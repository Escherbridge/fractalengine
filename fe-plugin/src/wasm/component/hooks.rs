//! Host call of the `hooks` export (world `extension-with-hooks`); see src/wasm/AGENTS.md §hooks.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use fe_runtime::extension_binding::{hook_spec, HookCallKind};
use fe_sdk::manifest_v2::KNOWN_HOOKS;
use wasmtime::component::Instance;
use wasmtime::Store;

use super::instance::{outside_async, HookFault};
use super::state::InstanceState;
use super::{ComponentError, ExtensionComponent};

#[allow(dead_code, unused_imports, clippy::all)]
mod world {
    wasmtime::component::bindgen!({
        world: "extension-with-hooks",
        path: "wit/fractal-plugin-2.wit",
    });
}

use world::exports::fractalengine::plugin::hooks as wit;

/// The export a component must carry when its manifest declares `provides` (bindings.md §7.3).
pub const HOOKS_EXPORT: &str = "fractalengine:plugin/hooks@2.0.0";
/// Largest JSON value in or out of one hook call.
pub const MAX_HOOK_JSON_BYTES: usize = 64 * 1024;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
/// Bound for a call whose catalogue row has no deadline: service `configure` calls.
const SERVICE_CALL_DEADLINE_MS: u64 = 5_000;
/// Bound for an after-commit call whose catalogue row has no deadline (`blob.mirror`).
const AFTER_COMMIT_DEADLINE_MS: u64 = 30_000;

/// Per-hook caps of bindings.md §6: the deadline and the byte bodies each way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookLimits {
    /// Wall-clock budget for one call; a longer call is a counted timeout.
    pub deadline: Duration,
    /// Largest `body` handed to the guest.
    pub max_body_in: usize,
    /// Largest `body` the guest may return.
    pub max_body_out: usize,
}

/// The §6 limits of a hook a manifest may provide (`KNOWN_HOOKS`, the B0 catalogue's deadline);
/// `None` for any other id, so this table can never serve a hook the validator refuses.
pub fn hook_limits(hook: &str) -> Option<HookLimits> {
    let spec = hook_spec(hook).filter(|_| KNOWN_HOOKS.contains(&hook))?;
    let (body_in, body_out) = match hook {
        "ingest.decode" => (MIB, 0),
        "asset.import" => (16 * MIB, 16 * MIB),
        "blob.mirror" => (8 * MIB, 0),
        "blob.source" | "sync.tileset-source" => (0, 16 * MIB),
        _ => (0, 0),
    };
    let millis = match spec.deadline_ms {
        Some(millis) => u64::from(millis),
        None if spec.call == HookCallKind::AfterCommit => AFTER_COMMIT_DEADLINE_MS,
        None => SERVICE_CALL_DEADLINE_MS,
    };
    Some(HookLimits {
        deadline: Duration::from_millis(millis),
        max_body_in: body_in,
        max_body_out: body_out,
    })
}

/// One hook call as the host builds it from a resolved binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRequest {
    /// Catalogue id, e.g. `petal.map`.
    pub hook: String,
    /// Binding key; empty for keyless hooks.
    pub key: String,
    /// Scope string of the binding row that selected this call.
    pub binding_scope: String,
    /// The binding's config, already validated against `config_schema`.
    pub config: String,
    /// Hook-specific input JSON (bindings.md §6).
    pub input: String,
    /// Bytes for `ingest.decode`, `asset.import`, `blob.mirror`; else empty.
    pub body: Vec<u8>,
}

impl HookRequest {
    /// The `configure` call a service hook gets at start and on every config change.
    pub fn configure(hook: &str, key: &str, binding_scope: &str, config: &str) -> Self {
        Self {
            hook: hook.into(),
            key: key.into(),
            binding_scope: binding_scope.into(),
            config: config.into(),
            input: r#"{"event":"configure"}"#.into(),
            body: Vec::new(),
        }
    }
}

/// A completed hook call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReply {
    /// Hook-specific output JSON.
    pub output: String,
    /// Bytes for `asset.import`, `blob.source`, `sync.tileset-source`; else empty.
    pub body: Vec<u8>,
}

/// The guest answered with an error: not a fault, the built-in serves the use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookRefusal {
    /// The guest declined (a `scene.*` refusal carries its reason to the caller).
    Refused(String),
    /// The guest rejected the input it was given.
    InvalidInput(String),
    /// The guest failed while serving.
    Failed(String),
}

/// Why a hook call produced no reply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookCallError {
    /// The id is not a hook this host serves; nothing ran and nothing was counted.
    #[error("hook {0} is not served by this host")]
    NotServed(String),
    /// The request or reply broke a cap; nothing ran or the reply was dropped.
    #[error("hook payload over cap: {0}")]
    TooLarge(&'static str),
    /// The guest returned its own error.
    #[error("hook declined: {0:?}")]
    Guest(HookRefusal),
    /// A trap or fuel exhaustion; the window is discarded and the caller degrades the instance.
    #[error("hook fault: {0}")]
    Fault(HookFault),
    /// The call returned after its deadline; the reply is dropped.
    #[error("hook missed its {0:?} deadline")]
    Timeout(Duration),
    /// The guest returned malformed or oversized JSON.
    #[error("hook returned bad output: {0}")]
    BadOutput(String),
}

/// Counters of one hook (extension status, R11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookCounters {
    /// Calls that reached the guest.
    pub calls: u64,
    /// Calls the guest refused or rejected.
    pub refused: u64,
    /// Traps, fuel exhaustion and bad output.
    pub faults: u64,
    /// Calls past their deadline.
    pub timeouts: u64,
}

/// Counters per hook id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookStats(BTreeMap<String, HookCounters>);

impl HookStats {
    /// One hook's counters.
    pub fn get(&self, hook: &str) -> HookCounters {
        self.0.get(hook).copied().unwrap_or_default()
    }

    /// `hooks.calls.<hook>`, `hooks.refused.<hook>`, `hooks.faults.<hook>`, `hooks.timeouts.<hook>`.
    pub fn status_rows(&self) -> Vec<(String, u64)> {
        let mut rows = Vec::with_capacity(self.0.len() * 4);
        for (hook, c) in &self.0 {
            rows.push((format!("hooks.calls.{hook}"), c.calls));
            rows.push((format!("hooks.refused.{hook}"), c.refused));
            rows.push((format!("hooks.faults.{hook}"), c.faults));
            rows.push((format!("hooks.timeouts.{hook}"), c.timeouts));
        }
        rows
    }

    fn entry(&mut self, hook: &str) -> &mut HookCounters {
        self.0.entry(hook.to_string()).or_default()
    }
}

/// `missing-export hooks`: a manifest that declares `provides` needs a component that exports `hooks`.
pub fn check_hooks_export(declares_provides: bool, component_exports_hooks: bool) -> Result<(), ComponentError> {
    if declares_provides && !component_exports_hooks {
        return Err(ComponentError::PackageInvalid(format!("missing-export {HOOKS_EXPORT}")));
    }
    Ok(())
}

/// True when the component type carries the `hooks` export.
pub fn component_exports_hooks(ty: &wasmtime::component::types::Component, engine: &wasmtime::Engine) -> bool {
    ty.get_export(engine, HOOKS_EXPORT).is_some()
}

/// The bound `hooks` export of one instance, with its own fuel refill and counters.
pub struct HooksGuest {
    bindings: world::ExtensionWithHooks,
    fuel: u64,
    stats: HookStats,
}

impl HooksGuest {
    /// Bind the export of an already-instantiated component (`fuel` is the manifest's `tick.fuel`).
    pub fn bind(store: &mut Store<InstanceState>, instance: &Instance, fuel: u64) -> Result<Self, ComponentError> {
        let bindings = world::ExtensionWithHooks::new(&mut *store, instance)
            .map_err(|error| ComponentError::PackageInvalid(format!("missing-export {HOOKS_EXPORT}: {error:#}")))?;
        Ok(Self { bindings, fuel, stats: HookStats::default() })
    }

    /// [`bind`](Self::bind) when `component` exports `hooks`; `None` for a plain 2.0 component.
    pub fn bind_if_exported(
        component: &ExtensionComponent,
        store: &mut Store<InstanceState>,
        instance: &Instance,
        fuel: u64,
    ) -> Result<Option<Self>, ComponentError> {
        if !component.exports_hooks() {
            return Ok(None);
        }
        Self::bind(store, instance, fuel).map(Some)
    }

    /// Counters for the status route.
    pub fn stats(&self) -> &HookStats {
        &self.stats
    }

    /// One call: caps in, own fuel refill, frozen clock, deadline, caps out. The caller holds the
    /// instance lock, refuses while degraded, and degrades on `Fault`.
    pub fn call(
        &mut self,
        store: &mut Store<InstanceState>,
        request: &HookRequest,
        now_ms: u64,
    ) -> Result<HookReply, HookCallError> {
        let limits = hook_limits(&request.hook).ok_or_else(|| HookCallError::NotServed(request.hook.clone()))?;
        for (what, json) in [("config", &request.config), ("input", &request.input)] {
            if json.len() > MAX_HOOK_JSON_BYTES {
                return Err(HookCallError::TooLarge(what));
            }
        }
        if request.body.len() > limits.max_body_in {
            return Err(HookCallError::TooLarge("body"));
        }
        let wire = wit::HookCall {
            hook: request.hook.clone(),
            key: request.key.clone(),
            binding_scope: request.binding_scope.clone(),
            config: request.config.clone(),
            input: request.input.clone(),
            body: request.body.clone(),
        };
        store
            .set_fuel(self.fuel)
            .map_err(|error| HookCallError::Fault(HookFault::Trap(format!("refuel failed: {error:#}"))))?;
        store.data().freeze_clock(now_ms);
        store.data().reset_hook_budgets();
        self.stats.entry(&request.hook).calls += 1;

        let started = Instant::now();
        let bindings = &self.bindings;
        let outcome = {
            let store = &mut *store;
            outside_async(move || bindings.fractalengine_plugin_hooks().call_on_hook(store, &wire))
        };
        let elapsed = started.elapsed();

        let result = match outcome {
            Err(error) => {
                let fault = classify_fault(store, &error);
                store.data_mut().discard_window();
                self.stats.entry(&request.hook).faults += 1;
                return Err(HookCallError::Fault(fault));
            }
            Ok(result) => result,
        };
        if elapsed > limits.deadline {
            self.stats.entry(&request.hook).timeouts += 1;
            return Err(HookCallError::Timeout(limits.deadline));
        }
        match result {
            Err(refusal) => {
                self.stats.entry(&request.hook).refused += 1;
                Err(HookCallError::Guest(match refusal {
                    wit::HookError::Refused(reason) => HookRefusal::Refused(reason),
                    wit::HookError::InvalidInput(reason) => HookRefusal::InvalidInput(reason),
                    wit::HookError::Failed(reason) => HookRefusal::Failed(reason),
                }))
            }
            Ok(reply) => match check_reply(&reply, &limits) {
                Ok(()) => Ok(HookReply { output: reply.output, body: reply.body }),
                Err(detail) => {
                    self.stats.entry(&request.hook).faults += 1;
                    Err(HookCallError::BadOutput(detail))
                }
            },
        }
    }
}

/// The reply caps: JSON ≤ 64 KiB and well-formed, body ≤ the hook's cap.
fn check_reply(reply: &wit::HookOutput, limits: &HookLimits) -> Result<(), String> {
    if reply.output.len() > MAX_HOOK_JSON_BYTES {
        return Err("output exceeds 64 KiB".into());
    }
    if serde_json::from_str::<serde_json::Value>(&reply.output).is_err() {
        return Err("output is not JSON".into());
    }
    if reply.body.len() > limits.max_body_out {
        return Err("body exceeds the hook's cap".into());
    }
    Ok(())
}

/// Fuel exhaustion, a guest exit or any other trap, as the instance's own fault classes.
fn classify_fault(store: &Store<InstanceState>, error: &wasmtime::Error) -> HookFault {
    let out_of_fuel = error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
        || store.get_fuel().is_ok_and(|fuel| fuel == 0);
    if out_of_fuel {
        HookFault::FuelExhausted
    } else if let Some(exit) = error.downcast_ref::<wasmtime_wasi::I32Exit>() {
        HookFault::Trap(format!("guest exited with status {}", exit.0))
    } else {
        HookFault::Trap(format!("{error:#}"))
    }
}
