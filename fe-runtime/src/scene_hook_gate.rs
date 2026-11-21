//! Send-side gate for the `scene.create` / `scene.transform` / `scene.property` binding hooks
//! (docs/extensions/bindings.md §6); rationale in `fe-runtime/src/AGENTS.md` §scene-hook-gate.
//!
//! Issuers (GUI commit, REST, MCP, WS) reach the gate through the one door, [`crate::scene_write`]; the DB thread,
//! `ApplyReplicatedRow` and `PluginCommit` never pass through it. Every bound call fails closed: a missed deadline,
//! fault, decline or bad output refuses the write (owner decision 2026-11-14, reversing OQ-B4); a petal import
//! provisions the new petal first (§scene-hook-import); a bound write whose hook input the caller sized over the
//! cap is refused (steer 2026-11-09 02:45); a caller over its in-flight allowance is refused busy; only
//! `property_scope` keys reach the guest; augments stage in the instance window through the binary's
//! [`SceneHookBackend`].

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy::prelude::Resource;
use fe_sdk::manifest_v2::ScopeAccess;
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// Deadline used if the catalogue row is ever missing (it is 250 ms for every scene hook).
const FALLBACK_DEADLINE_MS: u64 = 250;
/// Largest hook input JSON handed to a guest: one full node value plus the envelope (bindings.md §7.3).
pub const MAX_HOOK_INPUT_BYTES: usize = fe_sdk::plugin_host::MAX_NODE_PAYLOAD_BYTES;
/// Live calls one caller may hold at one binding (petal, package): one full write's worth.
pub const MAX_HOOK_CALLS_IN_FLIGHT: usize = crate::editor_batch::MAX_EDITOR_BATCH_TARGETS;

/// A call's deadline for an input of `input_bytes`: `base`, growing linearly to twice `base` at
/// [`MAX_HOOK_INPUT_BYTES`]. Shared by the gate's wait, the instance's timeout and its epoch interrupt.
pub fn input_deadline(base: Duration, input_bytes: usize) -> Duration {
    let span = MAX_HOOK_INPUT_BYTES as u32;
    let bytes = input_bytes.min(MAX_HOOK_INPUT_BYTES) as u32;
    base + base * bytes / span
}
/// Longest refusal reason shown to a caller, in bytes.
pub const MAX_REFUSAL_REASON_BYTES: usize = 512;
/// Most augment property sets one answer may stage (the tick window's op ceiling).
pub const MAX_AUGMENT_OPS: usize = 64;

static INSTALLED: OnceLock<SceneHookGate> = OnceLock::new();
/// Source of [`ProvisionalPetal::hold`] ids (one per provision, paired with its release).
static NEXT_HOLD: AtomicU64 = AtomicU64::new(1);

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

/// One node write as the gate sees it; rotation is Euler XYZ radians; numbers are the values the DB stores.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneHookInput {
    Create {
        petal_id: String,
        name: String,
        position: [f64; 3],
    },
    Transform {
        petal_id: String,
        node_id: String,
        position: [f64; 3],
        rotation: [f64; 3],
        scale: [f64; 3],
    },
    /// `value` is `null` for a deleted key.
    Property {
        petal_id: String,
        node_id: String,
        key: String,
        value: Value,
    },
    /// A create whose name and position the DB derives from `source_node_id` (duplicate, stamp promotion).
    CreateFrom {
        petal_id: String,
        source_node_id: String,
    },
    /// One initial property of a node being created (`node: null` in the hook input).
    InitialProperty {
        petal_id: String,
        key: String,
        value: Value,
    },
}

impl SceneHookInput {
    /// The hook this write triggers.
    pub fn hook(&self) -> SceneHook {
        match self {
            Self::Create { .. } | Self::CreateFrom { .. } => SceneHook::Create,
            Self::Transform { .. } => SceneHook::Transform,
            Self::Property { .. } | Self::InitialProperty { .. } => SceneHook::Property,
        }
    }

    /// The petal whose bindings decide.
    pub fn petal_id(&self) -> &str {
        match self {
            Self::Create { petal_id, .. }
            | Self::Transform { petal_id, .. }
            | Self::Property { petal_id, .. }
            | Self::CreateFrom { petal_id, .. }
            | Self::InitialProperty { petal_id, .. } => petal_id,
        }
    }

    /// The written node; `None` for a create (its id exists only after commit).
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Self::Create { .. } | Self::CreateFrom { .. } | Self::InitialProperty { .. } => None,
            Self::Transform { node_id, .. } | Self::Property { node_id, .. } => Some(node_id),
        }
    }

    /// The property key a `scene.property` input carries.
    pub fn property_key(&self) -> Option<&str> {
        match self {
            Self::Property { key, .. } | Self::InitialProperty { key, .. } => Some(key),
            _ => None,
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
            Self::CreateFrom {
                petal_id,
                source_node_id,
            } => json!({
                "petal_id": petal_id,
                "proposed": { "source": source_node_id },
                "caller": caller,
            }),
            Self::InitialProperty { key, value, .. } => json!({
                "node": Value::Null,
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
    /// `hook-error::invalid-input` or `failed`: no judgment, so the write is refused.
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

/// A petal an import is about to create, addressed where it will live; `hold` pairs one provision with its release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalPetal {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
    pub hold: u64,
}

/// Why the backend cannot answer for a provisional petal; every case refuses the import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionFailure {
    /// Bindings could not be read or resolved now (host gone, snapshot unreadable, busy): REST 503.
    Unavailable(String),
    /// A scene hook in effect there cannot run on this host (unresolved here, or placed on another peer): REST 409.
    Unjudgeable { hook: String, reason: String },
    /// The id already names a petal at another address, whose hooks must not see this import: REST 409.
    Exists,
}

/// Where the backend answers a provision.
pub type ProvisionReplySender = oneshot::Sender<Result<(), ProvisionFailure>>;

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
    /// Resolve bindings at a petal that does not exist yet, so `bound`/`dispatch` answer for its id until `release`;
    /// never blocks. The default cannot, so an import is refused (fail closed).
    fn provision(&self, _petal: ProvisionalPetal, reply: ProvisionReplySender) {
        let _ = reply.send(Err(ProvisionFailure::Unavailable(
            "this host cannot resolve scene hooks for a new petal".into(),
        )));
    }
    /// End one `provision` (called exactly once per provision, whatever it answered); never blocks.
    fn release(&self, _petal: &ProvisionalPetal) {}
}

/// A provisioned petal; dropping it releases the provision.
#[must_use = "hold the lease until every hook call of the import settled"]
pub struct ProvisionLease {
    backend: Arc<dyn SceneHookBackend>,
    petal: ProvisionalPetal,
}

impl Drop for ProvisionLease {
    fn drop(&mut self) {
        self.backend.release(&self.petal);
    }
}

/// A bound call that gave no usable answer; live writes and imports both refuse on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnansweredCall {
    /// No answer within the hook's deadline.
    Deadline,
    /// Trap, fuel exhaustion, bad output, a dropped call or no serving instance.
    Fault(String),
    /// Guest `invalid-input` / `failed`.
    Declined(String),
}

impl UnansweredCall {
    /// The client-facing kind: `deadline`, `fault` or `declined`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Deadline => "deadline",
            Self::Fault(_) => "fault",
            Self::Declined(_) => "declined",
        }
    }

    /// Why the extension gave no answer, for a refusal line.
    pub fn describe(&self) -> String {
        match self {
            Self::Deadline => "it did not answer in time".into(),
            Self::Fault(detail) => format!("it crashed ({})", clean_reason(detail)),
            Self::Declined(detail) => format!("it declined to judge ({})", clean_reason(detail)),
        }
    }
}

/// A strictly settled call (imports): an answer maps as live, a non-answer never passes.
#[derive(Debug)]
pub enum StrictVerdict {
    Answered(SceneVerdict),
    Unanswered {
        package_id: String,
        failure: UnansweredCall,
    },
}

/// Gate outcomes per (package, hook), reported beside the instance's own `hooks.*` counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SceneGateCounters {
    /// Calls dispatched to the backend.
    pub calls: u64,
    pub allowed: u64,
    pub refused: u64,
    /// Guest `invalid-input` / `failed`: the write was refused.
    pub declined: u64,
    /// Faults and bad output: the write was refused.
    pub faults: u64,
    /// No answer within the deadline: the write was refused.
    pub timeouts: u64,
    /// Writes that never reached the guest because their key is outside `property_scope`.
    pub skipped: u64,
    /// Writes refused because the caller-sized hook input exceeded [`MAX_HOOK_INPUT_BYTES`].
    pub too_large: u64,
    /// Writes refused before dispatch because their caller held [`MAX_HOOK_CALLS_IN_FLIGHT`] live calls.
    pub busy: u64,
    pub augments_staged: u64,
    pub augments_dropped: u64,
}

impl SceneGateCounters {
    fn rows(&self) -> [(&'static str, u64); 11] {
        [
            ("calls", self.calls),
            ("allowed", self.allowed),
            ("refused", self.refused),
            ("declined", self.declined),
            ("faults", self.faults),
            ("timeouts", self.timeouts),
            ("skipped", self.skipped),
            ("too_large", self.too_large),
            ("busy", self.busy),
            ("augments_staged", self.augments_staged),
            ("augments_dropped", self.augments_dropped),
        ]
    }
}

type CounterTable = BTreeMap<(String, SceneHook), SceneGateCounters>;
/// Live calls per (petal, package, caller DID).
type InFlightTable = HashMap<(String, String, String), usize>;

/// One live call held against its caller's allowance; dropping it (settled or abandoned) frees the slot.
struct InFlight {
    table: Arc<Mutex<InFlightTable>>,
    key: (String, String, String),
}

impl InFlight {
    /// A slot when the caller holds fewer than [`MAX_HOOK_CALLS_IN_FLIGHT`] live calls at this binding.
    fn take(table: &Arc<Mutex<InFlightTable>>, key: (String, String, String)) -> Option<Self> {
        let mut held = table.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = held.entry(key.clone()).or_default();
        if *count >= MAX_HOOK_CALLS_IN_FLIGHT {
            return None;
        }
        *count += 1;
        Some(Self {
            table: Arc::clone(table),
            key,
        })
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut held = self.table.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = held.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}

/// The host's scene hook gate; `Default` is disabled (every write proceeds untouched).
#[derive(Resource, Clone, Default)]
pub struct SceneHookGate {
    backend: Option<Arc<dyn SceneHookBackend>>,
    counters: Arc<Mutex<CounterTable>>,
    in_flight: Arc<Mutex<InFlightTable>>,
}

impl SceneHookGate {
    /// A gate over the binary's backend.
    pub fn new(backend: Arc<dyn SceneHookBackend>) -> Self {
        Self {
            backend: Some(backend),
            counters: Arc::default(),
            in_flight: Arc::default(),
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
        if let Some(key) = input.property_key() {
            if !in_property_scope(&binding.property_scope, key, ScopeAccess::Read) {
                self.count(&binding.package_id, hook, |c| c.skipped += 1);
                return SceneGateTicket::proceed();
            }
        }
        let input_json = input.to_json(caller.to_json(backend.local_did()));
        let input_bytes = input_json.to_string().len();
        // The caller sized this input (names, values): over the cap it is refused, never waved through.
        if input_bytes > MAX_HOOK_INPUT_BYTES {
            self.count(&binding.package_id, hook, |c| c.too_large += 1);
            return SceneGateTicket(TicketState::Ready(SceneVerdict::InputTooLarge {
                package_id: binding.package_id,
                limit_bytes: MAX_HOOK_INPUT_BYTES,
            }));
        }
        // A caller holding a full write's worth of live calls here is refused busy, before the worker sees it.
        let caller_did = caller.did.clone().unwrap_or_else(|| backend.local_did().to_owned());
        let slot_key = (input.petal_id().to_owned(), binding.package_id.clone(), caller_did);
        let Some(slot) = InFlight::take(&self.in_flight, slot_key) else {
            self.count(&binding.package_id, hook, |c| c.busy += 1);
            return SceneGateTicket(TicketState::Ready(SceneVerdict::Busy {
                retry_after_ms: u64::try_from(input_deadline(hook.deadline(), MAX_HOOK_INPUT_BYTES).as_millis())
                    .unwrap_or(u64::MAX),
                package_id: binding.package_id,
            }));
        };
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
        let deadline = Instant::now() + input_deadline(hook.deadline(), input_bytes);
        backend.dispatch(call, reply);
        SceneGateTicket(TicketState::Waiting(Box::new(Waiting {
            gate: self.clone(),
            hook,
            petal_id: input.petal_id().to_owned(),
            subject: input.node_id().map(str::to_owned),
            binding,
            answer,
            deadline,
            _slot: slot,
        })))
    }

    /// Check one write from async code, waiting at most the hook's deadline.
    pub async fn check(&self, input: SceneHookInput, caller: SceneCaller) -> SceneVerdict {
        self.begin(input, caller).wait().await
    }

    /// True when a backend is installed; a disabled gate judges nothing, live or imported.
    pub fn is_enabled(&self) -> bool {
        self.backend.is_some()
    }

    /// Resolve bindings for `petal_id`, about to be created in `fractal_id`, waiting until `deadline`.
    /// `Ok(None)` = disabled gate; the lease keeps the provision until dropped, and every failure path releases it too.
    pub async fn provision(
        &self,
        verse_id: &str,
        fractal_id: &str,
        petal_id: &str,
        deadline: tokio::time::Instant,
    ) -> Result<Option<ProvisionLease>, ProvisionFailure> {
        let Some(backend) = &self.backend else {
            return Ok(None);
        };
        let petal = ProvisionalPetal {
            verse_id: verse_id.to_owned(),
            fractal_id: fractal_id.to_owned(),
            petal_id: petal_id.to_owned(),
            hold: NEXT_HOLD.fetch_add(1, Ordering::Relaxed),
        };
        let (reply, answer) = oneshot::channel();
        backend.provision(petal.clone(), reply);
        // Built before waiting: a late answer after a timeout is still released, in order behind the provision.
        let lease = ProvisionLease {
            backend: Arc::clone(backend),
            petal,
        };
        match tokio::time::timeout_at(deadline, answer).await {
            Ok(Ok(Ok(()))) => Ok(Some(lease)),
            Ok(Ok(Err(failure))) => Err(failure),
            Ok(Err(_)) => Err(ProvisionFailure::Unavailable(
                "the extension host dropped the request".into(),
            )),
            Err(_) => Err(ProvisionFailure::Unavailable(
                "the extension host did not resolve bindings in time".into(),
            )),
        }
    }

    /// The package `input` would reach (bound, and in `property_scope` for a property); never blocks.
    pub fn dispatch_target(&self, input: &SceneHookInput) -> Option<String> {
        let binding = self.backend.as_ref()?.bound(input.hook(), input.petal_id())?;
        let in_scope = input
            .property_key()
            .is_none_or(|key| in_property_scope(&binding.property_scope, key, ScopeAccess::Read));
        in_scope.then_some(binding.package_id)
    }

    /// Serialized size of the call `input` would dispatch (0 with no backend), as `begin` measures it.
    pub(crate) fn input_bytes(&self, input: &SceneHookInput, caller: &SceneCaller) -> usize {
        self.backend.as_ref().map_or(0, |backend| {
            input.to_json(caller.to_json(backend.local_did())).to_string().len()
        })
    }

    /// Count one write refused before dispatch because its hook input was caller-sized over a cap.
    pub(crate) fn count_too_large(&self, package_id: &str, hook: SceneHook) {
        self.count(package_id, hook, |c| c.too_large += 1);
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
    /// The bound extension gave no answer (deadline, crash, decline, bad output): do not send the write.
    Unanswered {
        package_id: String,
        failure: UnansweredCall,
    },
    /// The caller already holds its live-call allowance at this binding: do not send; retry later.
    Busy { package_id: String, retry_after_ms: u64 },
}

impl SceneVerdict {
    fn proceed() -> Self {
        Self::Proceed { augment: None }
    }

    /// The client-facing refusal code (`refused`, `input-too-large`, `hook-failed`, `hook-busy`); `None` when the write proceeds.
    pub fn refusal_code(&self) -> Option<&'static str> {
        match self {
            Self::Refused { .. } => Some("refused"),
            Self::InputTooLarge { .. } => Some("input-too-large"),
            Self::Unanswered { .. } => Some("hook-failed"),
            Self::Busy { .. } => Some("hook-busy"),
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
            Self::Unanswered { package_id, failure } => Some(unanswered_message(package_id, failure)),
            Self::Busy {
                package_id,
                retry_after_ms,
            } => Some(busy_message(package_id, *retry_after_ms)),
            Self::Proceed { .. } => None,
        }
    }
}

/// The refusal line for a bound hook that gave no answer.
pub fn unanswered_message(package_id: &str, failure: &UnansweredCall) -> String {
    format!("Refused by {package_id}: {}; the edit was not applied", failure.describe())
}

/// The refusal line for a caller over its live-call allowance.
pub fn busy_message(package_id: &str, retry_after_ms: u64) -> String {
    format!("Refused by {package_id}: the extension is busy; retry in {retry_after_ms} ms")
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

    /// The package whose answer asked for the augment.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// The written node, `None` while a create's node id is unknown.
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
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
    /// The caller's live-call slot, freed when the ticket settles or is dropped.
    _slot: InFlight,
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

    /// Async, for imports: like [`wait`](Self::wait), but a deadline, fault, decline or bad output is `Unanswered`.
    pub async fn wait_strict(self) -> StrictVerdict {
        match self.0 {
            TicketState::Ready(verdict) => StrictVerdict::Answered(verdict),
            TicketState::Spent => StrictVerdict::Unanswered {
                package_id: String::new(),
                failure: UnansweredCall::Fault("the ticket was already settled".into()),
            },
            TicketState::Waiting(mut waiting) => {
                let deadline = tokio::time::Instant::from_std(waiting.deadline);
                let reply = match tokio::time::timeout_at(deadline, &mut waiting.answer).await {
                    Ok(Ok(reply)) => Some(reply),
                    Ok(Err(_)) => Some(dropped_reply()),
                    Err(_) => None,
                };
                (*waiting).settle_strict(reply)
            }
        }
    }
}

fn dropped_reply() -> SceneHookReply {
    SceneHookReply::Fault("the backend dropped the call".into())
}

impl Waiting {
    /// Strict mapping (imports): every non-answer is `Unanswered` and counted; an answer settles as live.
    fn settle_strict(self, reply: Option<SceneHookReply>) -> StrictVerdict {
        let package_id = self.binding.package_id.clone();
        let hook = self.hook;
        let failure = match &reply {
            None => {
                self.gate.count(&package_id, hook, |c| c.timeouts += 1);
                Some(UnansweredCall::Deadline)
            }
            Some(SceneHookReply::Fault(detail)) => {
                self.gate.count(&package_id, hook, |c| c.faults += 1);
                Some(UnansweredCall::Fault(detail.clone()))
            }
            Some(SceneHookReply::Declined(detail)) => {
                self.gate.count(&package_id, hook, |c| c.declined += 1);
                Some(UnansweredCall::Declined(detail.clone()))
            }
            Some(SceneHookReply::Output(output)) => match parse_answer(output) {
                Err(detail) => {
                    self.gate.count(&package_id, hook, |c| c.faults += 1);
                    Some(UnansweredCall::Fault(detail))
                }
                Ok(_) => None,
            },
            Some(SceneHookReply::Refused(_)) => None,
        };
        match failure {
            Some(failure) => {
                tracing::warn!(package_id, hook = hook.id(), ?failure, "scene hook gave no answer; the write is refused");
                StrictVerdict::Unanswered {
                    package_id,
                    failure,
                }
            }
            None => StrictVerdict::Answered(self.answered(reply)),
        }
    }

    /// Map an answer (`None` = deadline passed) to a verdict, counting it; every non-answer refuses (fail closed).
    fn settle(self, reply: Option<SceneHookReply>) -> SceneVerdict {
        let gate = self.gate.clone();
        let hook = self.hook;
        match self.settle_strict(reply) {
            StrictVerdict::Answered(verdict) => verdict,
            StrictVerdict::Unanswered { package_id, failure } => {
                gate.count(&package_id, hook, |c| c.refused += 1);
                SceneVerdict::Unanswered { package_id, failure }
            }
        }
    }

    /// Map a usable answer (allow or refuse) to a verdict, counting it; `settle_strict` filtered the rest.
    fn answered(self, reply: Option<SceneHookReply>) -> SceneVerdict {
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
        // Non-answers were counted and filtered by `settle_strict`; they can only refuse here.
        let unanswered = |failure: UnansweredCall| SceneVerdict::Unanswered {
            package_id: package_id.clone(),
            failure,
        };
        match reply {
            None => unanswered(UnansweredCall::Deadline),
            Some(SceneHookReply::Fault(detail)) => unanswered(UnansweredCall::Fault(detail)),
            Some(SceneHookReply::Declined(detail)) => unanswered(UnansweredCall::Declined(detail)),
            Some(SceneHookReply::Refused(reason)) => refuse(&reason),
            Some(SceneHookReply::Output(output)) => match parse_answer(&output) {
                Err(detail) => unanswered(UnansweredCall::Fault(detail)),
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
        .filter(|op| in_property_scope(&binding.property_scope, &op.key, ScopeAccess::Write))
        .take(MAX_AUGMENT_OPS)
        .collect()
}

/// The shared `property_scope` rule: judging a write is a read of its key, staging an augment op is a write.
fn in_property_scope(patterns: &[String], key: &str, access: ScopeAccess) -> bool {
    fe_sdk::manifest_v2::scope_allows(patterns, key, access)
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
pub(crate) mod tests {
    use super::*;

    /// A backend with one binding at petal `p1`; `answer: None` holds the call unanswered; `transform_answer`
    /// overrides it for `scene.transform`.
    pub(crate) struct FakeBackend {
        pub(crate) binding: SceneBinding,
        pub(crate) answer: Option<SceneHookReply>,
        pub(crate) transform_answer: Option<SceneHookReply>,
        pub(crate) calls: Mutex<Vec<SceneHookCall>>,
        pub(crate) held: Mutex<Vec<SceneHookReplySender>>,
        pub(crate) staged: Mutex<Vec<(String, Vec<AugmentOp>)>>,
    }

    impl SceneHookBackend for FakeBackend {
        fn local_did(&self) -> &str {
            "did:key:local"
        }
        fn bound(&self, _hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
            (petal_id == "p1").then(|| self.binding.clone())
        }
        fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender) {
            let answer = match (&self.transform_answer, call.hook) {
                (Some(answer), SceneHook::Transform) => Some(answer),
                _ => self.answer.as_ref(),
            };
            self.calls.lock().unwrap().push(call);
            match answer {
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

    pub(crate) fn gate_with(
        answer: Option<SceneHookReply>,
        may_augment: bool,
    ) -> (SceneHookGate, Arc<FakeBackend>) {
        fake_gate(answer, None, may_augment)
    }

    /// [`gate_with`] whose `scene.transform` calls get `transform_answer` instead.
    pub(crate) fn gate_with_transform(
        answer: Option<SceneHookReply>,
        transform_answer: Option<SceneHookReply>,
    ) -> (SceneHookGate, Arc<FakeBackend>) {
        fake_gate(answer, transform_answer, true)
    }

    fn fake_gate(
        answer: Option<SceneHookReply>,
        transform_answer: Option<SceneHookReply>,
        may_augment: bool,
    ) -> (SceneHookGate, Arc<FakeBackend>) {
        let backend = Arc::new(FakeBackend {
            binding: binding(may_augment),
            answer,
            transform_answer,
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

    /// Every guest answer maps to one verdict and one counter; only an allow lets the write through
    /// (fail closed, owner decision 2026-11-14): a decline, fault or bad output is `hook-failed`.
    #[test]
    fn scene_gate_answers_map_to_verdicts_and_fail_closed() {
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
                Some("!declined"),
                "declined",
            ),
            (
                "fault",
                SceneHookReply::Fault("fuel exhausted".into()),
                Some("!fault"),
                "faults",
            ),
            (
                "unknown verdict",
                SceneHookReply::Output(json!({"verdict": "maybe"})),
                Some("!fault"),
                "faults",
            ),
            (
                "augment not a list",
                SceneHookReply::Output(json!({"verdict": "allow", "augment": {}})),
                Some("!fault"),
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
            // `!kind` = no answer: refused as `hook-failed`, naming the extension and why.
            match (refusal, &verdict) {
                (Some(kind), SceneVerdict::Unanswered { package_id, failure }) if kind.starts_with('!') => {
                    assert_eq!((package_id.as_str(), failure.kind()), ("org.example.zoning", &kind[1..]), "{label}");
                    assert_eq!(verdict.refusal_code(), Some("hook-failed"), "{label}");
                    assert!(verdict.refusal_message().unwrap().starts_with("Refused by org.example.zoning: it "), "{label}");
                }
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
    /// §6 input, a silent guest times out closed, a flooding caller is refused busy, and an allowed augment stages
    /// only in-scope keys.
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

        // A silent guest on a small input: pending until the deadline, then the write is REFUSED `hook-failed`
        // naming the extension (fail closed; this used to proceed) and the call is abandoned.
        let (gate, backend) = gate_with(None, true);
        let mut ticket = gate.begin(
            property("zone"),
            SceneCaller::api(SceneCallerKind::Ws, "did:key:remote"),
        );
        let past_deadline =
            Instant::now() + SceneHook::Property.deadline() + Duration::from_millis(1);
        assert!(ticket.poll(Instant::now()).is_none() && ticket.is_waiting());
        let verdict = ticket.poll(past_deadline).expect("settled at the deadline");
        assert!(matches!(
            &verdict,
            SceneVerdict::Unanswered { package_id, failure: UnansweredCall::Deadline }
                if package_id == "org.example.zoning"
        ));
        assert_eq!(
            verdict.refusal_message().as_deref(),
            Some("Refused by org.example.zoning: it did not answer in time; the edit was not applied")
        );
        let counters = gate.counters("org.example.zoning", SceneHook::Property);
        assert_eq!((counters.timeouts, counters.refused), (1, 1));
        assert!(
            backend.held.lock().unwrap()[0].is_closed(),
            "the backend can skip an abandoned call"
        );
        assert_eq!(
            backend.calls.lock().unwrap()[0].input["caller"],
            json!({"did": "did:key:remote", "kind": "ws", "local": false})
        );

        // Backlog: one caller holding a full write's worth of live calls at this binding is refused busy before
        // dispatch (retryable); another caller still reaches the guest, and a settled call frees its slot.
        let (gate, backend) = gate_with(None, true);
        let flooder = SceneCaller::api(SceneCallerKind::Rest, "did:key:flooder");
        let mut held: Vec<_> = (0..MAX_HOOK_CALLS_IN_FLIGHT)
            .map(|_| gate.begin(property("zone"), flooder.clone()))
            .collect();
        assert!(
            gate.begin(property("zone"), SceneCaller::ui()).is_waiting(),
            "the local GUI caller has its own allowance"
        );
        let mut over = gate.begin(property("zone"), flooder.clone());
        let busy = over.poll(Instant::now()).expect("refused at once");
        assert!(matches!(
            &busy,
            SceneVerdict::Busy { package_id, retry_after_ms } if package_id == "org.example.zoning" && *retry_after_ms > 0
        ));
        assert_eq!(busy.refusal_code(), Some("hook-busy"));
        assert_eq!(backend.calls.lock().unwrap().len(), MAX_HOOK_CALLS_IN_FLIGHT + 1, "the busy call never dispatched");
        drop(held.pop());
        assert!(gate.begin(property("zone"), flooder).is_waiting(), "a freed slot admits the next call");
        assert_eq!(gate.counters("org.example.zoning", SceneHook::Property).busy, 1);

        // Full-size values (src/AGENTS.md §scene-hook-gate bounds): an input exactly at the cap reaches the
        // guest, which judges it (here: refuses); one byte more is refused before any call.
        let sized = |len: usize| SceneHookInput::Property {
            petal_id: "p1".into(),
            node_id: "n1".into(),
            key: "zone".into(),
            value: json!("x".repeat(len)),
        };
        let envelope = sized(0)
            .to_json(SceneCaller::ui().to_json("did:key:local"))
            .to_string()
            .len();
        let full = MAX_HOOK_INPUT_BYTES - envelope;
        assert!(full >= fe_sdk::property_schema::MAX_PROPERTY_BYTES);
        let (gate, backend) = gate_with(
            Some(SceneHookReply::Output(
                json!({"verdict": "refuse", "reason": "zone too large"}),
            )),
            true,
        );
        assert!(matches!(
            settle_now(&gate, sized(full)),
            SceneVerdict::Refused { reason, .. } if reason == "zone too large"
        ));
        assert_eq!(
            backend.calls.lock().unwrap()[0].input["value"]
                .as_str()
                .map(str::len),
            Some(full),
            "the guest judged the whole value"
        );
        assert!(matches!(
            settle_now(&gate, sized(full + 1)),
            SceneVerdict::InputTooLarge { limit_bytes: MAX_HOOK_INPUT_BYTES, .. }
        ));
        assert_eq!(backend.calls.lock().unwrap().len(), 1);

        // Deadline scaling keeps honest hooks time to parse a large input; any size that gets no answer refuses.
        let base = SceneHook::Property.deadline();
        assert_eq!(input_deadline(base, 0), base);
        assert_eq!(input_deadline(base, MAX_HOOK_INPUT_BYTES), 2 * base);
        let (gate, _backend) = gate_with(None, true);
        let mut ticket = gate.begin(sized(full), SceneCaller::ui());
        assert!(
            ticket.poll(Instant::now() + base + Duration::from_millis(1)).is_none(),
            "a full-size input waits its doubled deadline"
        );
        assert!(matches!(
            ticket.poll(Instant::now() + 2 * base + Duration::from_millis(1)),
            Some(SceneVerdict::Unanswered { failure: UnansweredCall::Deadline, .. })
        ));
        // A crash refuses both a full-size and a small input (the small one used to proceed).
        let (gate, _backend) = gate_with(Some(SceneHookReply::Fault("fuel exhausted".into())), true);
        for input in [sized(full), property("zone")] {
            let verdict = settle_now(&gate, input);
            assert!(matches!(&verdict, SceneVerdict::Unanswered { failure: UnansweredCall::Fault(_), .. }));
            assert_eq!(
                verdict.refusal_message().as_deref(),
                Some("Refused by org.example.zoning: it crashed (fuel exhausted); the edit was not applied")
            );
        }
        let counters = gate.counters("org.example.zoning", SceneHook::Property);
        assert_eq!((counters.faults, counters.refused), (2, 2));

        // Judging a write reads its key (wildcards cover render.mesh); an augment op writes it (exact entry only).
        let wide = ["*".to_string()];
        assert!(in_property_scope(&wide, "render.mesh", ScopeAccess::Read));
        assert!(!in_property_scope(&wide, "render.mesh", ScopeAccess::Write));
        assert!(in_property_scope(&["render.mesh".to_string()], "render.mesh", ScopeAccess::Write));

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
