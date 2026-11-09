//! Send-side gate for the `scene.create` / `scene.transform` / `scene.property` binding hooks
//! (docs/extensions/bindings.md §6); rationale in `fe-runtime/src/AGENTS.md` §scene-hook-gate.
//!
//! Issuers (GUI commit, REST, MCP, WS) ask the gate before they send a node write; the DB thread,
//! `ApplyReplicatedRow` and `PluginCommit` never pass through it. Faults and timeouts fail open and
//! are counted (OQ-B4); a bound write whose hook input the caller sized over the cap is refused
//! (steer 2026-11-09 02:45); only `property_scope` keys reach the guest; augments stage in the
//! instance window through the binary's [`SceneHookBackend`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy::prelude::Resource;
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// Deadline used if the catalogue row is ever missing (it is 250 ms for every scene hook).
const FALLBACK_DEADLINE_MS: u64 = 250;
/// Largest hook input JSON handed to a guest (the `hooks` ABI cap, bindings.md §7.3).
pub const MAX_HOOK_INPUT_BYTES: usize = 64 * 1024;
/// Longest refusal reason shown to a caller, in bytes.
pub const MAX_REFUSAL_REASON_BYTES: usize = 512;
/// Most augment property sets one answer may stage (the tick window's op ceiling).
pub const MAX_AUGMENT_OPS: usize = 64;

static INSTALLED: OnceLock<SceneHookGate> = OnceLock::new();

/// The three scene hook points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SceneHook {
    Create,
    Transform,
    Property,
}

impl SceneHook {
    /// Catalogue id (`scene.create`, ...).
    pub fn id(self) -> &'static str {
        match self {
            Self::Create => "scene.create",
            Self::Transform => "scene.transform",
            Self::Property => "scene.property",
        }
    }

    /// The catalogue's wait deadline for this hook.
    pub fn deadline(self) -> Duration {
        let millis = crate::extension_binding::hook_spec(self.id())
            .and_then(|spec| spec.deadline_ms)
            .map_or(FALLBACK_DEADLINE_MS, u64::from);
        Duration::from_millis(millis)
    }
}

/// Where a gated write came from: the door that issued it (hook input `caller.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneCallerKind {
    Ui,
    Rest,
    Mcp,
    /// The `/ws` entity commands.
    Ws,
}

impl SceneCallerKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Rest => "rest",
            Self::Mcp => "mcp",
            Self::Ws => "ws",
        }
    }
}

/// The authenticated author of a gated write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneCaller {
    pub kind: SceneCallerKind,
    /// `None` = this host's local DID (the GUI user).
    pub did: Option<String>,
}

impl SceneCaller {
    /// The local GUI user.
    pub fn ui() -> Self {
        Self {
            kind: SceneCallerKind::Ui,
            did: None,
        }
    }

    /// An API principal (token `sub`).
    pub fn api(kind: SceneCallerKind, did: &str) -> Self {
        Self {
            kind,
            did: Some(did.to_owned()),
        }
    }

    fn to_json(&self, local_did: &str) -> Value {
        let did = self.did.as_deref().unwrap_or(local_did);
        json!({ "did": did, "kind": self.kind.as_str(), "local": did == local_did })
    }
}

/// One node write as the gate sees it; rotation is Euler XYZ radians, as stored.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneHookInput {
    Create {
        petal_id: String,
        name: String,
        position: [f32; 3],
    },
    Transform {
        petal_id: String,
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    Property {
        petal_id: String,
        node_id: String,
        key: String,
        value: Value,
    },
}

impl SceneHookInput {
    /// The hook this write triggers.
    pub fn hook(&self) -> SceneHook {
        match self {
            Self::Create { .. } => SceneHook::Create,
            Self::Transform { .. } => SceneHook::Transform,
            Self::Property { .. } => SceneHook::Property,
        }
    }

    /// The petal whose bindings decide.
    pub fn petal_id(&self) -> &str {
        match self {
            Self::Create { petal_id, .. }
            | Self::Transform { petal_id, .. }
            | Self::Property { petal_id, .. } => petal_id,
        }
    }

    /// The written node; `None` for a create (its id exists only after commit).
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Self::Create { .. } => None,
            Self::Transform { node_id, .. } | Self::Property { node_id, .. } => Some(node_id),
        }
    }

    /// The bindings.md §6 input JSON.
    fn to_json(&self, caller: Value) -> Value {
        match self {
            Self::Create {
                petal_id,
                name,
                position,
            } => json!({
                "petal_id": petal_id,
                "proposed": { "name": name, "position": position },
                "caller": caller,
            }),
            Self::Transform {
                node_id,
                position,
                rotation,
                scale,
                ..
            } => json!({
                "node": node_id,
                "proposed": { "position": position, "rotation": rotation, "scale": scale },
                "caller": caller,
            }),
            Self::Property {
                node_id,
                key,
                value,
                ..
            } => json!({
                "node": node_id,
                "key": key,
                "value": value,
                "caller": caller,
            }),
        }
    }
}

/// What the binary's resolution cache says serves one scene hook at one petal.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneBinding {
    pub package_id: String,
    /// Scope string of the binding row that selected the call (`hook-call.binding-scope`).
    pub binding_scope: String,
    /// The binding's config, validated by the resolver.
    pub config: Value,
    /// The build's `scene.property_scope` patterns (`*`, `prefix*` or an exact key).
    pub property_scope: Vec<String>,
    /// The build declares `scene.write` with `set_property` and a tick, so augments may stage.
    pub may_augment: bool,
}

/// One call for the backend to run on the bound instance (maps onto WIT `hook-call`, key `""`, no body).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneHookCall {
    pub hook: SceneHook,
    pub petal_id: String,
    pub package_id: String,
    pub binding_scope: String,
    pub config: Value,
    pub input: Value,
}

/// The backend's answer to one call.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneHookReply {
    /// `hook-output.output`.
    Output(Value),
    /// `hook-error::refused(reason)`.
    Refused(String),
    /// `hook-error::invalid-input` or `failed`: the built-in serves the use.
    Declined(String),
    /// Trap, fuel exhaustion, bad output, a missed deadline, or no instance could serve.
    Fault(String),
}

/// Where the backend sends its answer; `is_closed()` once the gate stopped waiting.
pub type SceneHookReplySender = oneshot::Sender<SceneHookReply>;

/// One `set_property` an allowing guest asked for on the hook's subject node.
#[derive(Debug, Clone, PartialEq)]
pub struct AugmentOp {
    pub key: String,
    pub value: Value,
}

/// What only the binary can do: resolve bindings, reach instances, stage into their windows.
pub trait SceneHookBackend: Send + Sync + 'static {
    /// This host's local DID.
    fn local_did(&self) -> &str;
    /// The binding that resolves for `hook` at `petal_id` on this peer, read from a cache; never blocks.
    fn bound(&self, hook: SceneHook, petal_id: &str) -> Option<SceneBinding>;
    /// Run `call` off the caller's thread (lazy-starting the instance) and answer on `reply`; never blocks.
    fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender);
    /// Stage `ops` as `set_property` on `node_id` in the instance's window; they flush with its next tick.
    fn stage_augment(
        &self,
        petal_id: &str,
        package_id: &str,
        node_id: &str,
        ops: Vec<AugmentOp>,
    ) -> Result<(), String>;
}

/// Gate outcomes per (package, hook), reported beside the instance's own `hooks.*` counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SceneGateCounters {
    /// Calls dispatched to the backend.
    pub calls: u64,
    pub allowed: u64,
    pub refused: u64,
    /// Guest `invalid-input` / `failed`: the write proceeded.
    pub declined: u64,
    /// Faults and bad output: the write proceeded (OQ-B4).
    pub faults: u64,
    /// No answer within the deadline: the write proceeded (OQ-B4).
    pub timeouts: u64,
    /// Writes that never reached the guest because their key is outside `property_scope`.
    pub skipped: u64,
    /// Writes refused because the caller-sized hook input exceeded [`MAX_HOOK_INPUT_BYTES`].
    pub too_large: u64,
    pub augments_staged: u64,
    pub augments_dropped: u64,
}

impl SceneGateCounters {
    fn rows(&self) -> [(&'static str, u64); 10] {
        [
            ("calls", self.calls),
            ("allowed", self.allowed),
            ("refused", self.refused),
            ("declined", self.declined),
            ("faults", self.faults),
            ("timeouts", self.timeouts),
            ("skipped", self.skipped),
            ("too_large", self.too_large),
            ("augments_staged", self.augments_staged),
            ("augments_dropped", self.augments_dropped),
        ]
    }
}

type CounterTable = BTreeMap<(String, SceneHook), SceneGateCounters>;

/// The host's scene hook gate; `Default` is disabled (every write proceeds untouched).
#[derive(Resource, Clone, Default)]
pub struct SceneHookGate {
    backend: Option<Arc<dyn SceneHookBackend>>,
    counters: Arc<Mutex<CounterTable>>,
}

impl SceneHookGate {
    /// A gate over the binary's backend.
    pub fn new(backend: Arc<dyn SceneHookBackend>) -> Self {
        Self {
            backend: Some(backend),
            counters: Arc::default(),
        }
    }

    /// Install the process gate once (before the API thread serves) and return it for the Bevy app.
    pub fn install(backend: Arc<dyn SceneHookBackend>) -> Result<Self, &'static str> {
        let gate = Self::new(backend);
        INSTALLED
            .set(gate.clone())
            .map_err(|_| "scene hook gate already installed")?;
        Ok(gate)
    }

    /// The installed process gate, for issuers outside the Bevy world (REST, MCP, WS).
    pub fn installed() -> Option<Self> {
        INSTALLED.get().cloned()
    }

    /// Start checking one write; never blocks. Poll or wait the ticket before sending the write.
    pub fn begin(&self, input: SceneHookInput, caller: SceneCaller) -> SceneGateTicket {
        let Some(backend) = &self.backend else {
            return SceneGateTicket::proceed();
        };
        let hook = input.hook();
        let Some(binding) = backend.bound(hook, input.petal_id()) else {
            return SceneGateTicket::proceed();
        };
        if let SceneHookInput::Property { key, .. } = &input {
            if !in_property_scope(&binding.property_scope, key) {
                self.count(&binding.package_id, hook, |c| c.skipped += 1);
                return SceneGateTicket::proceed();
            }
        }
        let input_json = input.to_json(caller.to_json(backend.local_did()));
        // The caller sized this input (names, values): over the cap it is refused, never waved through.
        if input_json.to_string().len() > MAX_HOOK_INPUT_BYTES {
            self.count(&binding.package_id, hook, |c| c.too_large += 1);
            return SceneGateTicket(TicketState::Ready(SceneVerdict::InputTooLarge {
                package_id: binding.package_id,
                limit_bytes: MAX_HOOK_INPUT_BYTES,
            }));
        }
        let call = SceneHookCall {
            hook,
            petal_id: input.petal_id().to_owned(),
            package_id: binding.package_id.clone(),
            binding_scope: binding.binding_scope.clone(),
            config: binding.config.clone(),
            input: input_json,
        };
        let (reply, answer) = oneshot::channel();
        self.count(&binding.package_id, hook, |c| c.calls += 1);
        let deadline = Instant::now() + hook.deadline();
        backend.dispatch(call, reply);
        SceneGateTicket(TicketState::Waiting(Box::new(Waiting {
            gate: self.clone(),
            hook,
            petal_id: input.petal_id().to_owned(),
            subject: input.node_id().map(str::to_owned),
            binding,
            answer,
            deadline,
        })))
    }

    /// Check one write from async code, waiting at most the hook's deadline.
    pub async fn check(&self, input: SceneHookInput, caller: SceneCaller) -> SceneVerdict {
        self.begin(input, caller).wait().await
    }

    /// One package's gate counters for one hook.
    pub fn counters(&self, package_id: &str, hook: SceneHook) -> SceneGateCounters {
        self.table()
            .get(&(package_id.to_owned(), hook))
            .copied()
            .unwrap_or_default()
    }

    /// Status rows `hooks.gate.<counter>.<hook>` for one package.
    pub fn status_rows(&self, package_id: &str) -> Vec<(String, u64)> {
        self.table()
            .iter()
            .filter(|((package, _), _)| package == package_id)
            .flat_map(|((_, hook), counters)| {
                counters
                    .rows()
                    .into_iter()
                    .map(move |(name, value)| (format!("hooks.gate.{name}.{}", hook.id()), value))
            })
            .collect()
    }

    fn table(&self) -> std::sync::MutexGuard<'_, CounterTable> {
        self.counters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn count(&self, package_id: &str, hook: SceneHook, bump: impl FnOnce(&mut SceneGateCounters)) {
        bump(
            self.table()
                .entry((package_id.to_owned(), hook))
                .or_default(),
        );
    }
}

/// The gate's answer for one write.
#[derive(Debug)]
pub enum SceneVerdict {
    /// Send the write; then stage the augment, if any.
    Proceed { augment: Option<SceneAugment> },
    /// The bound extension refused: do not send the write.
    Refused { package_id: String, reason: String },
    /// A bound write whose hook input is over `limit_bytes`: do not send it (REST answers 413).
    InputTooLarge { package_id: String, limit_bytes: usize },
}

impl SceneVerdict {
    fn proceed() -> Self {
        Self::Proceed { augment: None }
    }

    /// The client-facing refusal code (`refused`, `input-too-large`); `None` when the write proceeds.
    pub fn refusal_code(&self) -> Option<&'static str> {
        match self {
            Self::Refused { .. } => Some("refused"),
            Self::InputTooLarge { .. } => Some("input-too-large"),
            Self::Proceed { .. } => None,
        }
    }

    /// One line for an advisory or an error body; `None` when the write proceeds.
    pub fn refusal_message(&self) -> Option<String> {
        match self {
            Self::Refused { package_id, reason } => {
                Some(format!("Refused by {package_id}: {reason}"))
            }
            Self::InputTooLarge {
                package_id,
                limit_bytes,
            } => Some(format!(
                "Refused by {package_id}: input-too-large (the hook input exceeds {limit_bytes} bytes)"
            )),
            Self::Proceed { .. } => None,
        }
    }
}

/// Augment property sets from an allowing answer, already filtered to `property_scope`.
#[must_use = "stage the augment once the write is sent"]
pub struct SceneAugment {
    gate: SceneHookGate,
    hook: SceneHook,
    petal_id: String,
    package_id: String,
    subject: Option<String>,
    ops: Vec<AugmentOp>,
}

impl std::fmt::Debug for SceneAugment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneAugment")
            .field("hook", &self.hook)
            .field("package_id", &self.package_id)
            .field("subject", &self.subject)
            .field("ops", &self.ops)
            .finish()
    }
}

impl SceneAugment {
    /// The property sets that will stage.
    pub fn ops(&self) -> &[AugmentOp] {
        &self.ops
    }

    /// Stage on the written node (`scene.transform`, `scene.property`).
    pub fn stage(self) -> Result<(), String> {
        let subject = self
            .subject
            .clone()
            .ok_or_else(|| "a create augment needs the created node id".to_owned())?;
        self.stage_on(&subject)
    }

    /// Stage on the node a `scene.create` produced, once its id is known.
    pub fn stage_created(self, node_id: &str) -> Result<(), String> {
        self.stage_on(node_id)
    }

    fn stage_on(self, node_id: &str) -> Result<(), String> {
        let Self {
            gate,
            hook,
            petal_id,
            package_id,
            ops,
            ..
        } = self;
        let staged = ops.len() as u64;
        let Some(backend) = gate.backend.clone() else {
            return Err("scene hook gate is disabled".into());
        };
        let result = backend.stage_augment(&petal_id, &package_id, node_id, ops);
        match &result {
            Ok(()) => gate.count(&package_id, hook, |c| c.augments_staged += staged),
            Err(_) => gate.count(&package_id, hook, |c| c.augments_dropped += staged),
        }
        result
    }
}

/// A pending gate check; dropping it abandons the call (the backend sees the reply closed).
#[must_use = "poll or wait the ticket before sending the write"]
pub struct SceneGateTicket(TicketState);

enum TicketState {
    Ready(SceneVerdict),
    Waiting(Box<Waiting>),
    Spent,
}

struct Waiting {
    gate: SceneHookGate,
    hook: SceneHook,
    petal_id: String,
    subject: Option<String>,
    binding: SceneBinding,
    answer: oneshot::Receiver<SceneHookReply>,
    deadline: Instant,
}

impl SceneGateTicket {
    /// A settled ticket: the write proceeds untouched.
    pub fn proceed() -> Self {
        Self(TicketState::Ready(SceneVerdict::proceed()))
    }

    /// True while the guest's answer is outstanding.
    pub fn is_waiting(&self) -> bool {
        matches!(self.0, TicketState::Waiting(_))
    }

    /// Non-blocking (a frame loop): the verdict once answered or past the deadline, else `None`.
    pub fn poll(&mut self, now: Instant) -> Option<SceneVerdict> {
        match std::mem::replace(&mut self.0, TicketState::Spent) {
            TicketState::Ready(verdict) => Some(verdict),
            TicketState::Spent => None,
            TicketState::Waiting(mut waiting) => match waiting.answer.try_recv() {
                Ok(reply) => Some((*waiting).settle(Some(reply))),
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some((*waiting).settle(Some(dropped_reply())))
                }
                Err(oneshot::error::TryRecvError::Empty) if now >= waiting.deadline => {
                    Some((*waiting).settle(None))
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    self.0 = TicketState::Waiting(waiting);
                    None
                }
            },
        }
    }

    /// Async: the verdict, waiting at most the hook's deadline (needs a tokio time driver).
    pub async fn wait(self) -> SceneVerdict {
        match self.0 {
            TicketState::Ready(verdict) => verdict,
            TicketState::Spent => SceneVerdict::proceed(),
            TicketState::Waiting(mut waiting) => {
                let deadline = tokio::time::Instant::from_std(waiting.deadline);
                let reply = match tokio::time::timeout_at(deadline, &mut waiting.answer).await {
                    Ok(Ok(reply)) => Some(reply),
                    Ok(Err(_)) => Some(dropped_reply()),
                    Err(_) => None,
                };
                (*waiting).settle(reply)
            }
        }
    }
}

fn dropped_reply() -> SceneHookReply {
    SceneHookReply::Fault("the backend dropped the call".into())
}

impl Waiting {
    /// Map an answer (`None` = deadline passed) to a verdict, counting it; every non-answer fails open.
    fn settle(self, reply: Option<SceneHookReply>) -> SceneVerdict {
        let Self {
            gate,
            hook,
            petal_id,
            subject,
            binding,
            ..
        } = self;
        let package_id = binding.package_id.clone();
        let refuse = |reason: &str| {
            gate.count(&package_id, hook, |c| c.refused += 1);
            SceneVerdict::Refused {
                package_id: package_id.clone(),
                reason: clean_reason(reason),
            }
        };
        match reply {
            None => {
                gate.count(&package_id, hook, |c| c.timeouts += 1);
                tracing::warn!(
                    package_id,
                    hook = hook.id(),
                    "scene hook missed its deadline; the write proceeds"
                );
                SceneVerdict::proceed()
            }
            Some(SceneHookReply::Fault(detail)) => {
                gate.count(&package_id, hook, |c| c.faults += 1);
                tracing::warn!(package_id, hook = hook.id(), %detail, "scene hook faulted; the write proceeds");
                SceneVerdict::proceed()
            }
            Some(SceneHookReply::Declined(_)) => {
                gate.count(&package_id, hook, |c| c.declined += 1);
                SceneVerdict::proceed()
            }
            Some(SceneHookReply::Refused(reason)) => refuse(&reason),
            Some(SceneHookReply::Output(output)) => match parse_answer(&output) {
                Err(detail) => {
                    gate.count(&package_id, hook, |c| c.faults += 1);
                    tracing::warn!(package_id, hook = hook.id(), %detail, "scene hook bad output; the write proceeds");
                    SceneVerdict::proceed()
                }
                Ok(GuestAnswer::Refuse(reason)) => refuse(&reason),
                Ok(GuestAnswer::Allow(ops)) => {
                    let offered = ops.len();
                    let kept = admit_augment(&binding, ops);
                    let dropped = (offered - kept.len()) as u64;
                    gate.count(&package_id, hook, |c| {
                        c.allowed += 1;
                        c.augments_dropped += dropped;
                    });
                    let augment = (!kept.is_empty()).then(|| SceneAugment {
                        gate: gate.clone(),
                        hook,
                        petal_id,
                        package_id: package_id.clone(),
                        subject,
                        ops: kept,
                    });
                    SceneVerdict::Proceed { augment }
                }
            },
        }
    }
}

/// A well-formed guest answer.
#[derive(Debug, Clone, PartialEq)]
enum GuestAnswer {
    Allow(Vec<AugmentOp>),
    Refuse(String),
}

/// `{verdict: "allow" | "refuse", reason?, augment?: [{key, value}]}`; anything else is bad output.
fn parse_answer(output: &Value) -> Result<GuestAnswer, String> {
    match output.get("verdict").and_then(Value::as_str) {
        Some("refuse") => Ok(GuestAnswer::Refuse(
            output
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )),
        Some("allow") => {
            let ops = match output.get("augment") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(entries)) => entries
                    .iter()
                    .map(|entry| -> Result<AugmentOp, String> {
                        let key = entry
                            .get("key")
                            .and_then(Value::as_str)
                            .ok_or("augment entry without a key")?;
                        let value = entry
                            .get("value")
                            .cloned()
                            .ok_or("augment entry without a value")?;
                        Ok(AugmentOp {
                            key: key.to_owned(),
                            value,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                Some(_) => return Err("augment is not a list".into()),
            };
            Ok(GuestAnswer::Allow(ops))
        }
        Some(other) => Err(format!("unknown verdict {other:?}")),
        None => Err("missing verdict".into()),
    }
}

/// Keep the ops the build may stage: `may_augment`, inside `property_scope`, at most [`MAX_AUGMENT_OPS`].
fn admit_augment(binding: &SceneBinding, ops: Vec<AugmentOp>) -> Vec<AugmentOp> {
    if !binding.may_augment {
        return Vec::new();
    }
    ops.into_iter()
        .filter(|op| in_property_scope(&binding.property_scope, &op.key))
        .take(MAX_AUGMENT_OPS)
        .collect()
}

/// `*` matches every key, `prefix*` a prefix, anything else exactly (fe-plugin capability rule).
fn in_property_scope(patterns: &[String], key: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => key.starts_with(prefix),
            None => pattern == key,
        })
}

/// Guest text made safe to show: control characters become spaces, capped at a char boundary.
fn clean_reason(reason: &str) -> String {
    let cleaned: String = reason
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return "no reason given".into();
    }
    let mut end = trimmed.len().min(MAX_REFUSAL_REASON_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend with one binding at petal `p1`; `answer: None` holds the call unanswered.
    struct FakeBackend {
        binding: SceneBinding,
        answer: Option<SceneHookReply>,
        calls: Mutex<Vec<SceneHookCall>>,
        held: Mutex<Vec<SceneHookReplySender>>,
        staged: Mutex<Vec<(String, Vec<AugmentOp>)>>,
    }

    impl SceneHookBackend for FakeBackend {
        fn local_did(&self) -> &str {
            "did:key:local"
        }
        fn bound(&self, _hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
            (petal_id == "p1").then(|| self.binding.clone())
        }
        fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender) {
            self.calls.lock().unwrap().push(call);
            match &self.answer {
                Some(answer) => {
                    let _ = reply.send(answer.clone());
                }
                None => self.held.lock().unwrap().push(reply),
            }
        }
        fn stage_augment(
            &self,
            _petal: &str,
            _package: &str,
            node_id: &str,
            ops: Vec<AugmentOp>,
        ) -> Result<(), String> {
            self.staged.lock().unwrap().push((node_id.to_owned(), ops));
            Ok(())
        }
    }

    fn binding(may_augment: bool) -> SceneBinding {
        SceneBinding {
            package_id: "org.example.zoning".into(),
            binding_scope: "VERSE#v-FRACTAL#f".into(),
            config: json!({}),
            property_scope: vec!["zone*".into()],
            may_augment,
        }
    }

    fn gate_with(
        answer: Option<SceneHookReply>,
        may_augment: bool,
    ) -> (SceneHookGate, Arc<FakeBackend>) {
        let backend = Arc::new(FakeBackend {
            binding: binding(may_augment),
            answer,
            calls: Mutex::default(),
            held: Mutex::default(),
            staged: Mutex::default(),
        });
        (SceneHookGate::new(backend.clone()), backend)
    }

    fn transform(petal: &str) -> SceneHookInput {
        SceneHookInput::Transform {
            petal_id: petal.into(),
            node_id: "n1".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }

    fn property(key: &str) -> SceneHookInput {
        SceneHookInput::Property {
            petal_id: "p1".into(),
            node_id: "n1".into(),
            key: key.into(),
            value: json!(7),
        }
    }

    fn settle_now(gate: &SceneHookGate, input: SceneHookInput) -> SceneVerdict {
        gate.begin(input, SceneCaller::ui())
            .poll(Instant::now())
            .expect("an answered call settles at once")
    }

    /// Every guest answer maps to one verdict and one counter; only a refusal stops the write (OQ-B4).
    #[test]
    fn scene_gate_answers_map_to_verdicts_and_fail_open() {
        let long = "x".repeat(MAX_REFUSAL_REASON_BYTES + 40);
        let cases: Vec<(&str, SceneHookReply, Option<&str>, &str)> = vec![
            (
                "allow",
                SceneHookReply::Output(json!({"verdict": "allow"})),
                None,
                "allowed",
            ),
            (
                "refuse output",
                SceneHookReply::Output(json!({"verdict": "refuse", "reason": "zone\nlocked"})),
                Some("zone locked"),
                "refused",
            ),
            (
                "refuse without reason",
                SceneHookReply::Output(json!({"verdict": "refuse"})),
                Some("no reason given"),
                "refused",
            ),
            (
                "hook-error refused",
                SceneHookReply::Refused(long.clone()),
                Some(&long[..MAX_REFUSAL_REASON_BYTES]),
                "refused",
            ),
            (
                "declined",
                SceneHookReply::Declined("bad input".into()),
                None,
                "declined",
            ),
            (
                "fault",
                SceneHookReply::Fault("fuel exhausted".into()),
                None,
                "faults",
            ),
            (
                "unknown verdict",
                SceneHookReply::Output(json!({"verdict": "maybe"})),
                None,
                "faults",
            ),
            (
                "augment not a list",
                SceneHookReply::Output(json!({"verdict": "allow", "augment": {}})),
                None,
                "faults",
            ),
        ];
        let counter = |c: &SceneGateCounters, name: &str| {
            c.rows()
                .into_iter()
                .find(|(row, _)| *row == name)
                .map_or(0, |(_, v)| v)
        };
        for (label, answer, refusal, counted) in cases {
            let (gate, backend) = gate_with(Some(answer), true);
            let verdict = settle_now(&gate, transform("p1"));
            match (refusal, &verdict) {
                (Some(expected), SceneVerdict::Refused { package_id, reason }) => {
                    assert_eq!(package_id, "org.example.zoning", "{label}");
                    assert_eq!(reason, expected, "{label}");
                }
                (None, SceneVerdict::Proceed { augment: None }) => {}
                _ => panic!("{label}: unexpected verdict {verdict:?}"),
            }
            let counters = gate.counters("org.example.zoning", SceneHook::Transform);
            assert_eq!(
                (counters.calls, counter(&counters, counted)),
                (1, 1),
                "{label}"
            );
            assert_eq!(backend.calls.lock().unwrap().len(), 1, "{label}");
        }
        assert_eq!(clean_reason("  \t "), "no reason given");
        assert_eq!(
            clean_reason("é".repeat(400).as_str()).len(),
            MAX_REFUSAL_REASON_BYTES
        );
    }

    /// The commit flow: unbound petals and out-of-scope keys never reach the guest, the call carries the
    /// §6 input, a silent guest times out open, and an allowed augment stages only in-scope keys.
    #[test]
    fn scene_gate_journey_scope_deadline_and_augment() {
        // Disabled and unbound gates proceed without a call.
        assert!(matches!(
            settle_now(&SceneHookGate::default(), transform("p1")),
            SceneVerdict::Proceed { augment: None }
        ));
        let (gate, backend) = gate_with(
            Some(SceneHookReply::Output(json!({"verdict": "refuse"}))),
            true,
        );
        assert!(matches!(
            settle_now(&gate, transform("p2")),
            SceneVerdict::Proceed { augment: None }
        ));
        assert!(backend.calls.lock().unwrap().is_empty());

        // A key outside property_scope passes untouched and is counted as skipped.
        assert!(matches!(
            settle_now(&gate, property("owner")),
            SceneVerdict::Proceed { augment: None }
        ));
        assert!(backend.calls.lock().unwrap().is_empty());
        assert_eq!(
            gate.counters("org.example.zoning", SceneHook::Property)
                .skipped,
            1
        );

        // A caller-sized input over the cap is refused when bound (never waved through), without a call;
        // the same oversized value under an out-of-scope key still passes untouched.
        let oversized = |key: &str| SceneHookInput::Property {
            petal_id: "p1".into(),
            node_id: "n1".into(),
            key: key.into(),
            value: json!("x".repeat(MAX_HOOK_INPUT_BYTES)),
        };
        let too_large = settle_now(&gate, oversized("zone"));
        assert!(matches!(
            &too_large,
            SceneVerdict::InputTooLarge { package_id, limit_bytes: MAX_HOOK_INPUT_BYTES }
                if package_id == "org.example.zoning"
        ));
        assert_eq!(too_large.refusal_code(), Some("input-too-large"));
        assert!(matches!(
            settle_now(&gate, oversized("owner")),
            SceneVerdict::Proceed { augment: None }
        ));
        assert!(backend.calls.lock().unwrap().is_empty());
        let counters = gate.counters("org.example.zoning", SceneHook::Property);
        assert_eq!((counters.too_large, counters.skipped), (1, 2));

        // The transform call carries the §6 shape with a local UI caller.
        assert!(matches!(
            settle_now(&gate, transform("p1")),
            SceneVerdict::Refused { .. }
        ));
        let call = backend.calls.lock().unwrap()[0].clone();
        assert_eq!(
            (call.hook, call.binding_scope.as_str()),
            (SceneHook::Transform, "VERSE#v-FRACTAL#f")
        );
        assert_eq!(
            call.input,
            json!({
                "node": "n1",
                "proposed": {"position": [1.0, 2.0, 3.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                "caller": {"did": "did:key:local", "kind": "ui", "local": true},
            })
        );

        // A silent guest: pending until the deadline, then the write proceeds and the call is abandoned.
        let (gate, backend) = gate_with(None, true);
        let mut ticket = gate.begin(
            property("zone"),
            SceneCaller::api(SceneCallerKind::Ws, "did:key:remote"),
        );
        let past_deadline =
            Instant::now() + SceneHook::Property.deadline() + Duration::from_millis(1);
        assert!(ticket.poll(Instant::now()).is_none() && ticket.is_waiting());
        let verdict = ticket.poll(past_deadline);
        assert!(matches!(
            verdict,
            Some(SceneVerdict::Proceed { augment: None })
        ));
        assert_eq!(
            gate.counters("org.example.zoning", SceneHook::Property)
                .timeouts,
            1
        );
        assert!(
            backend.held.lock().unwrap()[0].is_closed(),
            "the backend can skip an abandoned call"
        );
        assert_eq!(
            backend.calls.lock().unwrap()[0].input["caller"],
            json!({"did": "did:key:remote", "kind": "ws", "local": false})
        );

        // An allowed transform stages only in-scope augment keys, on the written node.
        let augment = json!({"verdict": "allow", "augment": [{"key": "zone", "value": "A"}, {"key": "owner", "value": "x"}]});
        let (gate, backend) = gate_with(Some(SceneHookReply::Output(augment.clone())), true);
        let SceneVerdict::Proceed {
            augment: Some(staged),
        } = settle_now(&gate, transform("p1"))
        else {
            panic!("augment expected")
        };
        assert_eq!(
            staged.ops(),
            [AugmentOp {
                key: "zone".into(),
                value: json!("A")
            }]
        );
        staged.stage().unwrap();
        assert_eq!(backend.staged.lock().unwrap()[0].0, "n1");
        let counters = gate.counters("org.example.zoning", SceneHook::Transform);
        assert_eq!(
            (counters.augments_staged, counters.augments_dropped),
            (1, 1)
        );
        assert!(gate
            .status_rows("org.example.zoning")
            .contains(&("hooks.gate.augments_staged.scene.transform".into(), 1)));

        // A create augment waits for the created id; a build without set_property stages nothing.
        let create = SceneHookInput::Create {
            petal_id: "p1".into(),
            name: "kiosk".into(),
            position: [0.0; 3],
        };
        let SceneVerdict::Proceed {
            augment: Some(staged),
        } = settle_now(&gate, create.clone())
        else {
            panic!("augment expected")
        };
        staged.stage_created("n9").unwrap();
        assert_eq!(backend.staged.lock().unwrap()[1].0, "n9");
        let (gate, _) = gate_with(Some(SceneHookReply::Output(augment)), false);
        assert!(matches!(
            settle_now(&gate, create),
            SceneVerdict::Proceed { augment: None }
        ));

        // The async path (REST) settles the same way.
        let (gate, _) = gate_with(
            Some(SceneHookReply::Refused("closed for works".into())),
            true,
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let verdict = runtime.block_on(gate.check(
            transform("p1"),
            SceneCaller::api(SceneCallerKind::Rest, "did:key:remote"),
        ));
        assert_eq!(
            verdict.refusal_message().as_deref(),
            Some("Refused by org.example.zoning: closed for works")
        );
    }
}
