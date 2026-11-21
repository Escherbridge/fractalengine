//! Fixture `commands[]`, `hooks[]` and stream capture over the real bus and scene hook gate.
//! Rationale (what is shared with the host, what is mirrored): tools/fe-ext/AGENTS.md §sandbox-calls.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use fe_plugin::bus::schema::validate_args;
use fe_plugin::bus::{self, Bus, BusHooks, BusRefusal, Caller, CallerKind, CommandError, CommandOutcome, GuestWork, InstanceKey};
use fe_plugin::host_bridge::scene_hook_reply;
use fe_plugin::transaction::StagedWindow;
use fe_plugin::wasm::component::bus_bridge::{instance_key, wit_host_error};
use fe_plugin::wasm::component::{Binding, ExtensionBroker, ExtensionInstance, HookRequest};
use fe_policy::RoleLevel;
use fe_runtime::scene_hook_gate::{
    AugmentOp, SceneBinding, SceneCaller, SceneCallerKind, SceneHook, SceneHookBackend, SceneHookCall, SceneHookGate,
    SceneHookInput, SceneHookReplySender, SceneVerdict,
};
use fe_sdk::manifest_v2::{ExtensionManifest, SceneWrite};
use fe_sdk::plugin_host::PendingOp;
use serde::Deserialize;
use serde_json::{json, Value};

/// When a fixture call runs: before tick n's `on-tick` (`at_tick`) or after tick n's flush (`after_tick`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Before(u32),
    After(u32),
}

impl When {
    /// Exactly one of `at_tick` / `after_tick`, within `1..=ticks`.
    fn parse(at_tick: Option<u32>, after_tick: Option<u32>, ticks: u32, what: &str) -> Result<Self> {
        let when = match (at_tick, after_tick) {
            (Some(tick), None) => Self::Before(tick),
            (None, Some(tick)) => Self::After(tick),
            _ => bail!("fixture {what}: give exactly one of at_tick or after_tick"),
        };
        let (Self::Before(tick) | Self::After(tick)) = when;
        if !(1..=ticks).contains(&tick) {
            bail!("fixture {what}: tick {tick} is outside 1..={ticks}");
        }
        Ok(when)
    }
}

/// One scripted bus command call (`commands[]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCommand {
    #[serde(default)]
    pub at_tick: Option<u32>,
    #[serde(default)]
    pub after_tick: Option<u32>,
    /// An id under the manifest's `bus.expose.commands`; anything else is the bus's own refusal.
    pub command: String,
    #[serde(default = "empty_object")]
    pub args: Value,
    /// The caller's role at the petal: `owner` (default), `manager`, `editor`, `viewer` or `none`.
    #[serde(default)]
    pub role: Option<String>,
    /// `ui` (default), `rest` or `mcp`.
    #[serde(default)]
    pub caller: Option<String>,
    /// Echoed into this call's trace record, and into the stream and fuel rows its guest call produced.
    #[serde(default)]
    pub label: Option<String>,
}

/// One scripted hook judgment (`hooks[]`): `input` is the hook input without `caller`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureHook {
    #[serde(default)]
    pub at_tick: Option<u32>,
    #[serde(default)]
    pub after_tick: Option<u32>,
    pub hook: String,
    pub input: Value,
    /// The binding's config, checked against the provided hook's `config_schema` (default `{}`).
    #[serde(default)]
    pub config: Option<Value>,
    /// `ui` (default), `rest`, `mcp` or `ws`.
    #[serde(default)]
    pub caller: Option<String>,
    /// A remote author's DID; absent = this host's identity.
    #[serde(default)]
    pub caller_did: Option<String>,
    /// Echoed into this call's trace record, and into the stream and fuel rows its guest call produced.
    #[serde(default)]
    pub label: Option<String>,
}

/// Add `label` to a trace record when the fixture entry carries one (absent otherwise, so old traces keep their bytes).
fn stamp_label(record: &mut Value, label: Option<&str>) {
    if let Some(label) = label {
        record["label"] = json!(label);
    }
}

fn empty_object() -> Value {
    json!({})
}

/// The `scene.transform` input as fixtures write it (bindings.md §6 minus `caller`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformInput {
    node: String,
    proposed: Pose,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pose {
    position: [f64; 3],
    rotation: [f64; 3],
    scale: [f64; 3],
}

/// The `scene.property` input as fixtures write it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PropertyInput {
    node: String,
    key: String,
    value: Value,
}

/// Live commands carry `f32` vectors the gate widens (fe-runtime `scene_write::widen`); a fixture value does the same trip.
fn through_f32(vector: [f64; 3]) -> [f64; 3] {
    vector.map(|value| f64::from(value as f32))
}

/// The gate's input for a hook the sandbox drives; add a hook here and in [`DRIVEN_HOOKS`].
fn scene_input(hook: &str, petal_id: &str, input: &Value) -> Result<SceneHookInput> {
    let petal_id = petal_id.to_string();
    Ok(match hook {
        "scene.transform" => {
            let TransformInput { node, proposed } = serde_json::from_value(input.clone())
                .map_err(|error| anyhow!("scene.transform input is {{node, proposed: {{position, rotation, scale}}}}: {error}"))?;
            SceneHookInput::Transform {
                petal_id,
                node_id: node,
                position: through_f32(proposed.position),
                rotation: through_f32(proposed.rotation),
                scale: through_f32(proposed.scale),
            }
        }
        "scene.property" => {
            let PropertyInput { node, key, value } = serde_json::from_value(input.clone())
                .map_err(|error| anyhow!("scene.property input is {{node, key, value}}: {error}"))?;
            SceneHookInput::Property { petal_id, node_id: node, key, value }
        }
        other => bail!("the sandbox drives {} hooks; {other} is not driven yet", DRIVEN_HOOKS.join(" and ")),
    })
}

/// Hooks [`scene_input`] can build.
const DRIVEN_HOOKS: [&str; 2] = ["scene.transform", "scene.property"];

fn role_ceiling(role: Option<&str>) -> Result<Option<RoleLevel>> {
    Ok(match role.unwrap_or("owner") {
        "owner" => None,
        "manager" => Some(RoleLevel::Manager),
        "editor" => Some(RoleLevel::Editor),
        "viewer" => Some(RoleLevel::Viewer),
        "none" => Some(RoleLevel::None),
        other => bail!("fixture command role {other:?} is not owner, manager, editor, viewer or none"),
    })
}

fn command_caller_kind(caller: Option<&str>) -> Result<CallerKind> {
    Ok(match caller.unwrap_or("ui") {
        "ui" => CallerKind::Ui,
        "rest" => CallerKind::Rest,
        "mcp" => CallerKind::Mcp,
        other => bail!("fixture command caller {other:?} is not ui, rest or mcp"),
    })
}

fn scene_caller(caller: Option<&str>, did: Option<&str>) -> Result<SceneCaller> {
    let kind = match caller.unwrap_or("ui") {
        "ui" if did.is_none() => return Ok(SceneCaller::ui()),
        "ui" => SceneCallerKind::Ui,
        "rest" => SceneCallerKind::Rest,
        "mcp" => SceneCallerKind::Mcp,
        "ws" => SceneCallerKind::Ws,
        other => bail!("fixture hook caller {other:?} is not ui, rest, mcp or ws"),
    };
    Ok(SceneCaller { kind, did: did.map(str::to_string) })
}

/// A checked fixture call, ready to run at its tick.
enum Planned {
    Command { when: When, entry: FixtureCommand, kind: CallerKind, ceiling: Option<RoleLevel> },
    Hook { when: When, entry: FixtureHook, input: SceneHookInput, binding: SceneBinding, caller: SceneCaller },
}

impl Planned {
    fn when(&self) -> When {
        match self {
            Self::Command { when, .. } | Self::Hook { when, .. } => *when,
        }
    }
}

/// What the guest is doing right now, for stream and fuel records.
#[derive(Debug, Clone, Default)]
struct Cursor {
    tick: u64,
    call: String,
    label: Option<String>,
}

/// Shared between the run loop and [`SandboxBroker`]: the cursor and the publish log.
#[derive(Default)]
struct Log {
    cursor: Cursor,
    streams: Vec<Value>,
}

type SharedLog = Arc<Mutex<Log>>;

fn lock(log: &SharedLog) -> std::sync::MutexGuard<'_, Log> {
    log.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The `bus` imports answered by the real [`Bus`]; every publish that reaches the bus is logged with its verdict.
struct SandboxBroker {
    bus: Bus,
    log: SharedLog,
}

impl ExtensionBroker for SandboxBroker {
    fn bus_subscribe(&self, binding: &Binding, source: &str, params: &str) -> Result<u64, fe_plugin::wasm::component::HostError> {
        self.bus.subscribe(&instance_key(binding), source, params).map_err(wit_host_error)
    }

    fn bus_unsubscribe(&self, binding: &Binding, subscription: u64) {
        self.bus.unsubscribe(&instance_key(binding), subscription);
    }

    fn bus_refresh(&self, binding: &Binding, subscription: u64) -> Result<(), fe_plugin::wasm::component::HostError> {
        self.bus.refresh(&instance_key(binding), subscription).map_err(wit_host_error)
    }

    fn bus_publish(&self, binding: &Binding, stream: &str, event: &str) -> Result<(), fe_plugin::wasm::component::HostError> {
        let outcome = self.bus.publish(&instance_key(binding), stream, event);
        let payload = serde_json::from_str::<Value>(event).unwrap_or_else(|_| Value::String(event.to_string()));
        let mut log = lock(&self.log);
        let mut record = json!({ "tick": log.cursor.tick, "call": log.cursor.call, "stream": stream, "payload": payload });
        stamp_label(&mut record, log.cursor.label.as_deref());
        match &outcome {
            Ok(()) => record["published"] = json!(true),
            Err(error) => record["refused"] = json!({ "code": error.code(), "detail": host_error_detail(error) }),
        }
        log.streams.push(record);
        drop(log);
        outcome.map_err(wit_host_error)
    }

    fn bus_call(&self, binding: &Binding, target: &str, args: &str) -> Result<u64, fe_plugin::wasm::component::HostError> {
        self.bus.call(&instance_key(binding), target, args).map_err(wit_host_error)
    }
}

fn host_error_detail(error: &bus::HostError) -> &str {
    match error {
        bus::HostError::CapabilityDenied(detail)
        | bus::HostError::PolicyDenied(detail)
        | bus::HostError::InvalidInput(detail)
        | bus::HostError::BudgetExceeded(detail)
        | bus::HostError::RateLimited(detail)
        | bus::HostError::Revoked(detail)
        | bus::HostError::NotAvailable(detail)
        | bus::HostError::Backend(detail)
        | bus::HostError::UnknownOutcome(detail) => detail,
    }
}

fn refusal_detail(refusal: &BusRefusal) -> Option<&str> {
    match refusal {
        BusRefusal::NotFound => None,
        BusRefusal::NotAvailable(detail)
        | BusRefusal::CapabilityDenied(detail)
        | BusRefusal::RateLimited(detail)
        | BusRefusal::InvalidInput(detail)
        | BusRefusal::LimitReached(detail) => Some(detail),
    }
}

fn command_error_detail(error: &CommandError) -> &str {
    match error {
        CommandError::InvalidInput(detail) | CommandError::Refused(detail) | CommandError::Failed(detail) => detail,
    }
}

/// The sandbox's one installed identity: Owner at its petal, no remote reach.
struct SandboxRoles {
    did: String,
}

impl bus::RoleSource for SandboxRoles {
    fn role_at(&self, did: &str, _petal_id: &str) -> RoleLevel {
        if did == self.did {
            RoleLevel::Owner
        } else {
            RoleLevel::None
        }
    }
}

struct NoRemoteReach;

impl bus::BusPolicy for NoRemoteReach {
    fn external_access_allowed(&self, _verse_id: &str) -> bool {
        false
    }
}

/// The gate's backend: one binding at a time, the dispatched call parked until the run loop answers it.
struct SandboxBackend {
    local_did: String,
    petal_id: String,
    binding: Mutex<Option<(SceneHook, SceneBinding)>>,
    parked: Mutex<Option<(SceneHookCall, SceneHookReplySender)>>,
    window: fe_plugin::tick::SharedWindow,
}

impl SceneHookBackend for SandboxBackend {
    fn local_did(&self) -> &str {
        &self.local_did
    }

    fn bound(&self, hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
        let binding = self.binding.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        binding
            .as_ref()
            .filter(|(bound, _)| *bound == hook && petal_id == self.petal_id)
            .map(|(_, binding)| binding.clone())
    }

    fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender) {
        *self.parked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((call, reply));
    }

    /// All or nothing into the instance window, as the binary's `LiveInstances::stage_augment` (scope was filtered by the gate).
    fn stage_augment(&self, _petal_id: &str, _package_id: &str, node_id: &str, ops: Vec<AugmentOp>) -> Result<(), String> {
        let mut window = self.window.lock().map_err(|_| "sandbox window lock poisoned".to_string())?;
        let mut staged: StagedWindow = window.clone();
        for op in ops {
            staged
                .stage_op(PendingOp::SetProperty { node_id: node_id.to_string(), key: op.key, value: op.value })
                .map_err(|error| error.to_string())?;
        }
        *window = staged;
        Ok(())
    }
}

/// The fixture's scripted calls plus the bus, gate and logs they run through.
pub struct Calls {
    bus: Bus,
    key: InstanceKey,
    planned: Vec<Planned>,
    gate: SceneHookGate,
    backend: Arc<SandboxBackend>,
    log: SharedLog,
    commands: Vec<Value>,
    hooks: Vec<Value>,
    fuel: Vec<Value>,
}

/// Most bus work units drained per fixture call (the worker's own per-poll cap is smaller; nothing here waits).
const DRAIN_LIMIT: usize = 64;

impl Calls {
    /// Check every fixture call against the manifest before anything runs; a hook the manifest does not provide is refused.
    pub fn plan(
        manifest: &ExtensionManifest,
        commands: &[FixtureCommand],
        hooks: &[FixtureHook],
        ticks: u32,
        petal_id: &str,
        petal_scope: &str,
        bus: Bus,
        key: InstanceKey,
        window: fe_plugin::tick::SharedWindow,
        local_did: &str,
    ) -> Result<Self> {
        let mut planned = Vec::new();
        for (index, entry) in commands.iter().enumerate() {
            let what = format!("commands[{index}]");
            let when = When::parse(entry.at_tick, entry.after_tick, ticks, &what)?;
            let kind = command_caller_kind(entry.caller.as_deref())?;
            let ceiling = role_ceiling(entry.role.as_deref())?;
            planned.push(Planned::Command { when, entry: entry.clone(), kind, ceiling });
        }
        for (index, entry) in hooks.iter().enumerate() {
            let what = format!("hooks[{index}]");
            let when = When::parse(entry.at_tick, entry.after_tick, ticks, &what)?;
            let provided = manifest
                .provides
                .iter()
                .find(|provided| provided.hook == entry.hook)
                .ok_or_else(|| anyhow!("fixture {what}: the manifest does not provide {} (add it under `provides`)", entry.hook))?;
            let input = scene_input(&entry.hook, petal_id, &entry.input).map_err(|error| anyhow!("fixture {what}: {error}"))?;
            let config = entry.config.clone().unwrap_or_else(empty_object);
            // The resolver leaves a binding whose config fails `config_schema` unresolved (fe-plugin bindings/resolver.rs).
            validate_args(provided.config_schema.as_ref(), &config)
                .map_err(|error| anyhow!("fixture {what}: config does not match config_schema: {error}"))?;
            let caller = scene_caller(entry.caller.as_deref(), entry.caller_did.as_deref())?;
            planned.push(Planned::Hook { when, entry: entry.clone(), binding: scene_binding(manifest, petal_scope, config), input, caller });
        }
        let backend = Arc::new(SandboxBackend {
            local_did: local_did.to_string(),
            petal_id: petal_id.to_string(),
            binding: Mutex::new(None),
            parked: Mutex::new(None),
            window,
        });
        Ok(Self {
            bus,
            key,
            planned,
            gate: SceneHookGate::new(backend.clone()),
            backend,
            log: SharedLog::default(),
            commands: Vec::new(),
            hooks: Vec::new(),
            fuel: Vec::new(),
        })
    }

    /// The broker the instance's `bus` imports go to.
    pub fn broker(&self) -> Arc<dyn ExtensionBroker> {
        Arc::new(SandboxBroker { bus: self.bus.clone(), log: self.log.clone() })
    }

    /// Name the guest call about to run (stream records carry it).
    pub fn enter(&self, tick: u64, call: &str) {
        self.enter_labeled(tick, call, None);
    }

    /// As [`Self::enter`], also carrying the fixture entry's `label` (the entry that caused this guest call).
    fn enter_labeled(&self, tick: u64, call: &str, label: Option<&str>) {
        lock(&self.log).cursor = Cursor { tick, call: call.to_string(), label: label.map(str::to_string) };
    }

    /// Record the fuel of the guest call that just returned.
    pub fn note_fuel(&mut self, instance: &ExtensionInstance) {
        if let Some(fuel) = instance.last_fuel() {
            let cursor = lock(&self.log).cursor.clone();
            let mut record = json!({ "tick": cursor.tick, "call": cursor.call, "limit": fuel.limit, "consumed": fuel.consumed });
            stamp_label(&mut record, cursor.label.as_deref());
            self.fuel.push(record);
        }
    }

    /// Run every call due at `when`, commands then hooks, each in fixture order.
    pub fn run_due(&mut self, when: When, tick: u64, instance: &mut ExtensionInstance) {
        let due: Vec<usize> = (0..self.planned.len()).filter(|index| self.planned[*index].when() == when).collect();
        let (commands, hooks): (Vec<usize>, Vec<usize>) =
            due.into_iter().partition(|index| matches!(self.planned[*index], Planned::Command { .. }));
        for index in commands.into_iter().chain(hooks) {
            match &self.planned[index] {
                Planned::Command { entry, kind, ceiling, .. } => {
                    let (entry, kind, ceiling) = (entry.clone(), *kind, *ceiling);
                    self.command(tick, &entry, kind, ceiling, instance);
                }
                Planned::Hook { entry, input, binding, caller, .. } => {
                    let (entry, input, binding, caller) = (entry.clone(), input.clone(), binding.clone(), caller.clone());
                    self.hook(tick, &entry, input, binding, caller, instance);
                }
            }
        }
    }

    /// One call through `Bus::call_command_local_client` (checks 1-8), then the guest, then the bus's answer.
    fn command(&mut self, tick: u64, entry: &FixtureCommand, kind: CallerKind, ceiling: Option<RoleLevel>, instance: &mut ExtensionInstance) {
        let caller = Caller { did: self.bus.local_did().to_string(), kind, package_id: None, local: true, role_ceiling: ceiling };
        let mut record = json!({
            "tick": tick,
            "command": entry.command,
            "args": entry.args,
            "role": entry.role.as_deref().unwrap_or("owner"),
        });
        stamp_label(&mut record, entry.label.as_deref());
        match self.bus.call_command_local_client(&caller, &self.key.petal_id, &self.key.package_id, &entry.command, &entry.args) {
            Err(refusal) => {
                record["host_refused"] = json!({ "code": refusal.code(), "status": refusal.http_status(), "detail": refusal_detail(&refusal) });
            }
            Ok(mut ticket) => {
                self.drain(tick, entry.label.as_deref(), instance);
                record["reply"] = match ticket.reply.try_recv() {
                    Ok(CommandOutcome::Ok(text)) => {
                        json!({ "ok": serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)) })
                    }
                    Ok(CommandOutcome::Guest(error)) => json!({
                        "guest_refused": { "code": error.code(), "status": error.http_status(), "reason": command_error_detail(&error) }
                    }),
                    Ok(CommandOutcome::Host(error)) => {
                        json!({ "host_refused": { "code": error.code(), "detail": host_error_detail(&error) } })
                    }
                    Err(_) => json!({ "host_refused": { "code": "unknown-outcome", "detail": "the call was not answered" } }),
                };
            }
        }
        self.commands.push(record);
    }

    /// Deliver queued bus work to the guest the way `Bus::dispatch` does, naming each call for the fuel log.
    fn drain(&mut self, tick: u64, label: Option<&str>, instance: &mut ExtensionInstance) {
        for _ in 0..DRAIN_LIMIT {
            let Some(work) = self.bus.next_work(&self.key) else { return };
            match work {
                GuestWork::Command { call, command, args, caller } => {
                    self.enter_labeled(tick, &format!("on-command {command}"), label);
                    let result = BusHooks::on_command(instance, &command, &args, &caller);
                    self.note_fuel(instance);
                    self.bus.complete_command(call, result);
                }
                GuestWork::Events(events) => {
                    self.enter(tick, "on-events");
                    BusHooks::on_events(instance, events);
                    self.note_fuel(instance);
                }
                GuestWork::CallResult { call, outcome } => {
                    self.enter(tick, "on-call-result");
                    BusHooks::on_call_result(instance, call, outcome);
                    self.note_fuel(instance);
                }
            }
        }
    }

    /// One write judged by the real `SceneHookGate`; the guest runs through `ExtensionInstance::call_hook`.
    fn hook(
        &mut self,
        tick: u64,
        entry: &FixtureHook,
        input: SceneHookInput,
        binding: SceneBinding,
        caller: SceneCaller,
        instance: &mut ExtensionInstance,
    ) {
        let hook = input.hook();
        *self.backend.binding.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((hook, binding));
        let mut ticket = self.gate.begin(input, caller);
        let parked = self.backend.parked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let mut record = json!({ "tick": tick, "hook": entry.hook, "input": entry.input });
        stamp_label(&mut record, entry.label.as_deref());
        if let Some((call, reply)) = parked {
            record["input"] = call.input.clone();
            // The request the binary's `scene_call` builds (fractalengine plugin_bridge/worker.rs).
            let request = HookRequest {
                hook: call.hook.id().to_string(),
                key: String::new(),
                binding_scope: call.binding_scope,
                config: call.config.to_string(),
                input: call.input.to_string(),
                body: Vec::new(),
            };
            self.enter_labeled(tick, &format!("hook {}", call.hook.id()), entry.label.as_deref());
            let outcome = instance.call_hook(&request, None);
            self.note_fuel(instance);
            let _ = reply.send(scene_hook_reply(outcome));
        }
        let verdict = ticket.poll(Instant::now());
        record["verdict"] = match verdict {
            Some(SceneVerdict::Proceed { augment: None }) => json!({ "allow": true }),
            Some(SceneVerdict::Proceed { augment: Some(augment) }) => {
                let ops: Vec<Value> = augment.ops().iter().map(|op| json!({ "key": op.key, "value": op.value })).collect();
                let staged = match augment.stage() {
                    Ok(()) => json!(true),
                    Err(error) => json!({ "dropped": error }),
                };
                json!({ "allow": true, "augment": ops, "staged": staged })
            }
            Some(SceneVerdict::Refused { reason, .. }) => json!({ "refused": "refused", "reason": reason }),
            Some(other) => json!({
                "refused": other.refusal_code(),
                "reason": other.refusal_message(),
            }),
            None => json!({ "refused": "hook-failed", "reason": "the gate did not settle" }),
        };
        *self.backend.binding.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.hooks.push(record);
    }

    /// The trace sections: commands, hooks, stream publishes, fuel per guest call.
    pub fn into_trace(self) -> (Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>) {
        let streams = std::mem::take(&mut lock(&self.log).streams);
        (self.commands, self.hooks, streams, self.fuel)
    }
}

/// The binding the binary's catalogue publishes for a resolved `scene.*` point (`catalog.rs` `scene_binding`).
fn scene_binding(manifest: &ExtensionManifest, petal_scope: &str, config: Value) -> SceneBinding {
    let scene = manifest.scene.as_ref();
    let writes_properties = scene
        .and_then(|scene| scene.write.as_ref())
        .is_some_and(|kinds| kinds.contains(&SceneWrite::SetProperty));
    SceneBinding {
        package_id: manifest.id.clone(),
        binding_scope: petal_scope.to_string(),
        config,
        property_scope: scene.and_then(|scene| scene.property_scope.clone()).unwrap_or_default(),
        may_augment: writes_properties && manifest.tick.is_some(),
    }
}

/// The sandbox's bus: the sandbox DID is the local identity and Owner at its petal; audits go to tracing.
pub fn sandbox_bus(clock: Arc<dyn fe_plugin::tick::Clock>, local_did: &str) -> Bus {
    let sources = bus::BusSources {
        query: None,
        scene: None,
        roles: Arc::new(SandboxRoles { did: local_did.to_string() }),
        policy: Arc::new(NoRemoteReach),
        audit: Arc::new(bus::sources::TracingAudit),
    };
    Bus::new(clock, sources, local_did)
}
