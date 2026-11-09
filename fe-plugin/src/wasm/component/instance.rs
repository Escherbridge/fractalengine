//! A live 2.0 instance: its store, hook calls, the `TickInstance` seam and the instance store;
//! see src/wasm/AGENTS.md §instance.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use wasmtime::Store;

use super::bindings::exports::fractalengine::plugin::bus_hooks::{BusEvent, CommandError};
use super::bindings::exports::fractalengine::plugin::ingress_hooks::{ConnectionEvent, IngressFrame, IngressReply};
use super::bindings::exports::fractalengine::plugin::lifecycle;
use super::bindings::fractalengine::plugin::egress::IncomingResponse;
use super::bindings::fractalengine::plugin::txn;
use super::bindings::fractalengine::plugin::types::{Binding, Caller, HostError};
use super::bindings::Extension;
use super::state::{refusal_error, InstanceState};
use crate::capability::CapabilityToken;
use crate::tick::{self, Clock, SharedWindow, TickFault, TickInstance};
use crate::transaction::StagedWindow;

/// Why a hook call did not complete. Fuel and traps degrade the instance and discard its window.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookFault {
    #[error("fuel exhausted")]
    FuelExhausted,
    #[error("trap: {0}")]
    Trap(String),
    /// The instance degraded earlier; no guest code runs any more.
    #[error("instance degraded: {0}")]
    Degraded(String),
}

/// Per-instance counters beside the scheduler's `TickCounters` (status route, R11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceStats {
    pub hook_calls: u64,
    pub fuel_exhaustions: u64,
    pub traps: u64,
    /// Log messages over the 50/s cap (`log.write` and guest stdio).
    pub log_dropped: u64,
}

/// The write window a token's tick budget allows (empty limits without a `tick` block).
pub fn window_for_token(token: &CapabilityToken) -> SharedWindow {
    let (ops, readings) = token
        .tick_budget()
        .map_or((0, 0), |budget| (budget.max_ops as usize, budget.max_readings as usize));
    Arc::new(Mutex::new(StagedWindow::new(ops, readings)))
}

/// Run a guest call outside any async context, so WASI's sync shims may block on their own runtime.
pub(crate) fn outside_async<R: Send>(call: impl FnOnce() -> R + Send) -> R {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(call),
        // block_in_place panics on a current-thread runtime; a scoped thread has no runtime at all.
        Ok(_) => std::thread::scope(|scope| match scope.spawn(call).join() {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }),
        Err(_) => call(),
    }
}

/// One instantiated component bound to one petal (OD-E3).
pub struct ExtensionInstance {
    store: Store<InstanceState>,
    bindings: Extension,
    clock: Arc<dyn Clock>,
    hook_fuel: u64,
    degraded: Option<String>,
    stats: InstanceStats,
}

impl ExtensionInstance {
    pub(crate) fn new(store: Store<InstanceState>, bindings: Extension, clock: Arc<dyn Clock>, hook_fuel: u64) -> Self {
        Self {
            store,
            bindings,
            clock,
            hook_fuel,
            degraded: None,
            stats: InstanceStats::default(),
        }
    }

    /// The host-minted binding this instance runs under.
    pub fn binding(&self) -> &Binding {
        &self.store.data().binding
    }

    /// The degrade reason, once a hook faulted.
    pub fn degraded(&self) -> Option<&str> {
        self.degraded.as_deref()
    }

    /// Counters for the status route.
    pub fn stats(&self) -> InstanceStats {
        let log_dropped = self
            .store
            .data()
            .log
            .lock()
            .map_or(0, |log| log.dropped());
        InstanceStats { log_dropped, ..self.stats.clone() }
    }

    /// The window this instance stages into (shared with its `TickScheduler`).
    pub fn window(&self) -> SharedWindow {
        self.store.data().window.clone()
    }

    /// Forget a route's flattened geometry (a `host/node-changes` event for that node).
    pub fn invalidate_route(&mut self, node_id: &str) {
        self.store.data_mut().routes.invalidate(node_id);
    }

    /// Set the store's fuel (the scheduler calls this before `on-tick`).
    pub fn refuel(&mut self, fuel: u64) -> Result<(), String> {
        self.store
            .set_fuel(fuel.min(fe_sdk::manifest_v2::HOST_FUEL_CEILING))
            .map_err(|error| format!("{error:#}"))
    }

    /// Enter the guest once: refuel (unless the scheduler did), freeze the clock, map faults.
    fn call<R, F>(&mut self, refuel: bool, now_ms: Option<u64>, invoke: F) -> Result<R, HookFault>
    where
        R: Send,
        F: FnOnce(&Extension, &mut Store<InstanceState>) -> wasmtime::Result<R> + Send,
    {
        if let Some(reason) = &self.degraded {
            return Err(HookFault::Degraded(reason.clone()));
        }
        if refuel {
            self.store
                .set_fuel(self.hook_fuel)
                .map_err(|error| HookFault::Trap(format!("refuel failed: {error:#}")))?;
        }
        let now = now_ms.unwrap_or_else(|| self.clock.now_ms());
        self.store.data().freeze_clock(now);
        self.store.data().reset_hook_budgets();
        self.stats.hook_calls += 1;
        let bindings = &self.bindings;
        let store = &mut self.store;
        match outside_async(move || invoke(bindings, store)) {
            Ok(value) => Ok(value),
            Err(error) => Err(self.fault(error)),
        }
    }

    /// Degrade on any fault: count it, discard the window unflushed, refuse further hooks.
    fn fault(&mut self, error: wasmtime::Error) -> HookFault {
        let out_of_fuel = error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
            || self.store.get_fuel().is_ok_and(|fuel| fuel == 0);
        let fault = if out_of_fuel {
            self.stats.fuel_exhaustions += 1;
            HookFault::FuelExhausted
        } else if let Some(exit) = error.downcast_ref::<wasmtime_wasi::I32Exit>() {
            self.stats.traps += 1;
            HookFault::Trap(format!("guest exited with status {}", exit.0))
        } else {
            self.stats.traps += 1;
            HookFault::Trap(format!("{error:#}"))
        };
        self.degraded = Some(fault.to_string());
        self.store.data_mut().discard_window();
        fault
    }

    /// `on-install`: first activation of this binding; a guest error refuses activation.
    pub fn on_install(&mut self) -> Result<Result<(), String>, HookFault> {
        let binding = self.binding().clone();
        self.call(true, None, move |guest, store| {
            guest.fractalengine_plugin_lifecycle().call_on_install(store, &binding)
        })
    }

    /// `on-activate`: every activation; a guest error refuses activation.
    pub fn on_activate(&mut self) -> Result<Result<(), String>, HookFault> {
        let binding = self.binding().clone();
        self.call(true, None, move |guest, store| {
            guest.fractalengine_plugin_lifecycle().call_on_activate(store, &binding)
        })
    }

    /// `on-deactivate`; the scheduler decides whether the window gets a final flush.
    pub fn on_deactivate(&mut self, reason: tick::DeactivateReason) -> Result<(), HookFault> {
        let reason = wit_deactivate_reason(reason);
        self.call(true, None, move |guest, store| {
            guest.fractalengine_plugin_lifecycle().call_on_deactivate(store, reason)
        })
    }

    /// `on-uninstall`; one final flush follows (the caller's job).
    pub fn on_uninstall(&mut self) -> Result<(), HookFault> {
        let binding = self.binding().clone();
        self.call(true, None, move |guest, store| {
            guest.fractalengine_plugin_lifecycle().call_on_uninstall(store, &binding)
        })
    }

    /// `on-tick` at the scheduler's instant; fuel was already refilled by `refuel`.
    pub fn on_tick(&mut self, info: &tick::TickInfo) -> Result<(), HookFault> {
        let wit = wit_tick_info(info);
        self.call(false, Some(info.now_ms), move |guest, store| {
            guest.fractalengine_plugin_lifecycle().call_on_tick(store, &wit)
        })
    }

    /// `on-ingress` for one declared endpoint's frame (E5's ingress broker calls this).
    pub fn on_ingress(&mut self, frame: &IngressFrame) -> Result<Option<IngressReply>, HookFault> {
        let now = frame.received_at_ms;
        self.call(true, Some(now), move |guest, store| {
            guest.fractalengine_plugin_ingress_hooks().call_on_ingress(store, frame)
        })
    }

    /// `on-connection` for a websocket endpoint.
    pub fn on_connection(&mut self, endpoint: &str, connection: u64, event: ConnectionEvent) -> Result<(), HookFault> {
        let endpoint = endpoint.to_string();
        self.call(true, None, move |guest, store| {
            guest
                .fractalengine_plugin_ingress_hooks()
                .call_on_connection(store, &endpoint, connection, event)
        })
    }

    /// `on-egress-result` for a queued request.
    pub fn on_egress_result(
        &mut self,
        request: u64,
        outcome: Result<IncomingResponse, HostError>,
    ) -> Result<(), HookFault> {
        self.call(true, None, move |guest, store| {
            guest
                .fractalengine_plugin_egress_hooks()
                .call_on_egress_result(store, request, outcome.as_ref())
        })
    }

    /// `on-events`: one delivery batch (at most 64 events, 256 KiB; bus.md).
    pub fn on_events(&mut self, events: &[BusEvent]) -> Result<(), HookFault> {
        self.call(true, None, move |guest, store| {
            guest.fractalengine_plugin_bus_hooks().call_on_events(store, events)
        })
    }

    /// `on-command`: `args` already passed the declared schema and the caller its role floor.
    pub fn on_command(
        &mut self,
        command: &str,
        args: &str,
        caller: &Caller,
    ) -> Result<Result<String, CommandError>, HookFault> {
        let (command, args) = (command.to_string(), args.to_string());
        self.call(true, None, move |guest, store| {
            guest
                .fractalengine_plugin_bus_hooks()
                .call_on_command(store, &command, &args, caller)
        })
    }

    /// `on-call-result` for a queued `bus.call`.
    pub fn on_call_result(&mut self, call: u64, outcome: Result<String, HostError>) -> Result<(), HookFault> {
        self.call(true, None, move |guest, store| {
            guest
                .fractalengine_plugin_bus_hooks()
                .call_on_call_result(store, call, outcome.as_ref())
        })
    }
}

/// The scheduler's `TickInfo` as the WIT record.
fn wit_tick_info(info: &tick::TickInfo) -> lifecycle::TickInfo {
    lifecycle::TickInfo {
        tick: info.tick,
        now_ms: info.now_ms,
        scheduled_at_ms: info.scheduled_at_ms,
        delta_ms: info.delta_ms,
        skipped: info.skipped,
        last_commit: wit_commit_outcome(&info.last_commit),
    }
}

/// The previous flush's outcome as the WIT variant; a refusal keeps its host-error case.
fn wit_commit_outcome(outcome: &tick::CommitOutcome) -> txn::CommitOutcome {
    match outcome {
        tick::CommitOutcome::None => txn::CommitOutcome::None,
        tick::CommitOutcome::Committed { tick, ops, readings, created_node_ids } => {
            txn::CommitOutcome::Committed(txn::CommitReceipt {
                tick: *tick,
                ops: *ops,
                readings: *readings,
                created_node_ids: created_node_ids.clone(),
            })
        }
        tick::CommitOutcome::Refused(message) => txn::CommitOutcome::Refused(refusal_error(message)),
        tick::CommitOutcome::Unknown => txn::CommitOutcome::Unknown,
    }
}

fn wit_deactivate_reason(reason: tick::DeactivateReason) -> lifecycle::DeactivateReason {
    use lifecycle::DeactivateReason as Wit;
    match reason {
        tick::DeactivateReason::Disabled => Wit::Disabled,
        tick::DeactivateReason::Unbound => Wit::Unbound,
        tick::DeactivateReason::Updating => Wit::Updating,
        tick::DeactivateReason::HostStopping => Wit::HostStopping,
        tick::DeactivateReason::RoleLost => Wit::RoleLost,
        tick::DeactivateReason::PolicyDenied => Wit::PolicyDenied,
        tick::DeactivateReason::Revoked => Wit::Revoked,
        tick::DeactivateReason::Degraded => Wit::Degraded,
    }
}

/// A shared handle: the scheduler ticks it, the bus and ingress brokers call its other hooks.
/// All callers run on the plugin worker, so the lock is uncontended.
#[derive(Clone)]
pub struct InstanceHandle(Arc<Mutex<ExtensionInstance>>);

impl InstanceHandle {
    /// Share one instance.
    pub fn new(instance: ExtensionInstance) -> Self {
        Self(Arc::new(Mutex::new(instance)))
    }

    /// Lock for a hook call or a status read.
    pub fn lock(&self) -> Result<MutexGuard<'_, ExtensionInstance>, String> {
        self.0.lock().map_err(|_| "extension instance lock poisoned".to_string())
    }
}

impl TickInstance for InstanceHandle {
    fn refuel(&mut self, fuel: u64) -> Result<(), String> {
        self.lock()?.refuel(fuel)
    }

    fn on_tick(&mut self, info: &tick::TickInfo) -> Result<(), TickFault> {
        let mut instance = self.lock().map_err(TickFault::Trap)?;
        instance.on_tick(info).map_err(|fault| match fault {
            HookFault::FuelExhausted => TickFault::FuelExhausted,
            other => TickFault::Trap(other.to_string()),
        })
    }

    fn on_deactivate(&mut self, reason: tick::DeactivateReason) {
        let outcome = self
            .lock()
            .map_err(HookFault::Trap)
            .and_then(|mut instance| instance.on_deactivate(reason));
        if let Err(fault) = outcome {
            tracing::warn!(%fault, ?reason, "extension on-deactivate faulted");
        }
    }
}

/// Live instances on this host, keyed by installation id (never replicated).
#[derive(Clone, Default)]
pub struct InstanceStore {
    instances: Arc<Mutex<HashMap<String, InstanceHandle>>>,
}

impl InstanceStore {
    /// Track a new instance; one per installation.
    pub fn insert(&self, handle: InstanceHandle) -> Result<(), String> {
        let installation_id = handle.lock()?.binding().installation_id.clone();
        let mut instances = self
            .instances
            .lock()
            .map_err(|_| "instance store lock poisoned".to_string())?;
        if instances.contains_key(&installation_id) {
            return Err(format!("installation {installation_id} already has a live instance"));
        }
        instances.insert(installation_id, handle);
        Ok(())
    }

    /// The live instance of an installation.
    pub fn get(&self, installation_id: &str) -> Option<InstanceHandle> {
        self.instances.lock().ok()?.get(installation_id).cloned()
    }

    /// Stop tracking an installation's instance.
    pub fn remove(&self, installation_id: &str) -> Option<InstanceHandle> {
        self.instances.lock().ok()?.remove(installation_id)
    }

    /// Installation ids with a live instance, sorted.
    pub fn installation_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .instances
            .lock()
            .map(|instances| instances.keys().cloned().collect())
            .unwrap_or_default();
        ids.sort();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::super::bindings::fractalengine::plugin::types::Vec3;
    use super::super::bindings::fractalengine::plugin::{kv, readings, routes, scene, txn as wit_txn};
    use super::super::test_guest::{self, fleet_setup};
    use super::super::{wasi, ComponentError, ComponentHost};
    use super::*;
    use crate::tick::ManualClock;
    use fe_plugin_test::mock_host::MockHostEnv;
    use fe_sdk::plugin_host::PendingOp;
    use fe_sdk::property::PropertyValue;

    fn tick_info(tick: u64) -> tick::TickInfo {
        tick::TickInfo {
            tick,
            now_ms: tick * 500,
            scheduled_at_ms: tick * 500,
            delta_ms: if tick == 1 { 0 } else { 500 },
            skipped: 0,
            last_commit: tick::CommitOutcome::None,
        }
    }

    /// 2.0 journey: contract and import policy at compile, then instantiate a component that
    /// stages through the shared window on every tick, then the host imports it can reach.
    #[test]
    fn component_instantiates_ticks_and_stages_through_the_shared_window() {
        let host = ComponentHost::new().unwrap();

        // A 1.0 core module is the retired ABI; imports outside the closed list are refused.
        assert!(matches!(
            host.compile(&wat::parse_str("(module)").unwrap()),
            Err(ComponentError::UnsupportedContract(_))
        ));
        for (name, admitted, unsupported) in [
            ("fractalengine:plugin/txn@2.0.0", true, false),
            ("wasi:cli/stdout@0.2.3", true, false),
            ("fractalengine:plugin/node-api@1.0.0", false, true),
            ("fractalengine:plugin/txn@2.1.0", false, true),
            ("fractalengine:plugin/lifecycle@2.0.0", false, false),
            ("wasi:sockets/tcp@0.2.0", false, false),
            ("wasi:clocks/monotonic-clock@0.2.0", false, false),
            ("wasi:http/outgoing-handler@0.2.0", false, false),
        ] {
            let verdict = wasi::check_import(name);
            assert_eq!(verdict.is_ok(), admitted, "{name}");
            assert_eq!(matches!(verdict, Err(ComponentError::UnsupportedContract(_))), unsupported, "{name}");
        }

        // A kill-list entry for the component digest refuses it under any package id (steer 20:45).
        let bytes = test_guest::component(test_guest::STAGE_ONE_TRANSFORM);
        let kill = crate::trust_roots::RevocationSet::from_entries(&[crate::trust_roots::RevocationEntry {
            package_id: "org.example.renamed".into(),
            content_hash: crate::wasm::aot::AotCache::hash_bytes(&bytes),
            reason: "test".into(),
            revoked_at: "2026-11-08T00:00:00Z".into(),
        }]);
        assert!(matches!(
            host.compile_unrevoked(&bytes, &"0".repeat(64), &kill),
            Err(ComponentError::Revoked(_))
        ));

        // Instantiate the mover guest: each on-tick stages set-transform(n1, [tick, 0, 0]).
        let component = host
            .compile_unrevoked(&bytes, &"0".repeat(64), &crate::trust_roots::RevocationSet::default())
            .unwrap();
        let mock = Arc::new(Mutex::new(MockHostEnv::new()));
        let clock = ManualClock::default();
        let instance = host.instantiate(&component, fleet_setup(&mock, &clock)).unwrap();
        let window = instance.window();
        let mut handle = InstanceHandle::new(instance);
        let store = InstanceStore::default();
        store.insert(handle.clone()).unwrap();
        assert!(store.insert(handle.clone()).is_err(), "one live instance per installation");
        assert_eq!(handle.lock().unwrap().on_activate(), Ok(Ok(())));
        for tick in [1, 2] {
            TickInstance::refuel(&mut handle, 200_000).unwrap();
            TickInstance::on_tick(&mut handle, &tick_info(tick)).unwrap();
        }
        let flush = window.lock().unwrap().take();
        let positions: Vec<_> = flush
            .ops
            .iter()
            .map(|op| match op {
                PendingOp::SetTransform { node_id, position, rotation: None, scale: None } if node_id == "n1" => *position,
                other => panic!("unexpected staged op {other:?}"),
            })
            .collect();
        assert_eq!(positions, vec![Some([1.0, 0.0, 0.0]), Some([2.0, 0.0, 0.0])]);
        assert!(flush.readings.is_empty());
        assert_eq!(handle.lock().unwrap().stats().hook_calls, 3);

        // A refused flush reaches the guest with its host-error case intact.
        let refused = wit_commit_outcome(&tick::CommitOutcome::Refused("invalid-input: unknown anchor a9".into()));
        assert!(matches!(refused, txn::CommitOutcome::Refused(HostError::InvalidInput(detail)) if detail == "unknown anchor a9"));

        // The same imports, driven host-side: routes read geometry the property scope hides.
        mock.lock().unwrap().insert_node(
            "r1",
            crate::NodeSnapshot {
                node_id: "r1".into(),
                petal_id: "p1".into(),
                name: "route".into(),
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                properties: HashMap::from([(
                    "gpx_points".to_string(),
                    PropertyValue::Json(serde_json::json!([[0.0, 0.0, 0.0, 0.0], [10.0, 0.0, 0.0, 1.0]])),
                )]),
            },
        );
        let mut instance = handle.lock().unwrap();
        let state = instance.store.data_mut();
        let sample = routes::Host::sample(state, "r1".into(), 0.25).unwrap();
        assert_eq!((sample.position.x, sample.tangent.x, sample.distance_m, sample.segment_index), (2.5, 1.0, 2.5, 0));
        let snap = routes::Host::snap(state, "r1".into(), Vec3 { x: 5.0, y: 3.0, z: 0.0 }).unwrap();
        assert_eq!((snap.nearest.x, snap.deviation_m, snap.progress), (5.0, 3.0, 0.5));
        let seen = scene::Host::get_node(state, "r1".into()).unwrap().unwrap();
        assert_eq!(seen.properties, "{}", "gpx_points is outside property_scope fleet.*");
        // G1: scene reads are a per-hook budget; each is a DB round trip the fuel meter never sees.
        let budget = super::super::state::MAX_SCENE_READS_PER_HOOK;
        state.reads_this_hook.store(budget, std::sync::atomic::Ordering::Relaxed);
        assert!(matches!(scene::Host::get_node(state, "r1".into()), Err(HostError::BudgetExceeded(_))));
        state.reset_hook_budgets();

        // Scope, metric and budget refusals change nothing; create ordinals restart per window.
        let out_of_scope = wit_txn::Host::stage_set_property(state, "r1".into(), "other".into(), "1".into());
        assert!(matches!(out_of_scope, Err(HostError::CapabilityDenied(_))));
        wit_txn::Host::stage_set_property(state, "r1".into(), "fleet.speed".into(), "4".into()).unwrap();
        let reading = |metric: &str| readings::Reading {
            anchor: "r1".into(),
            metric: metric.into(),
            value: 1.0,
            units: "m".into(),
            recorded_at: None,
        };
        assert!(matches!(readings::Host::stage(state, vec![reading("pressure")]), Err(HostError::CapabilityDenied(_))));
        readings::Host::stage(state, vec![reading("altitude_m")]).unwrap();
        let ordinals = ["a", "b"].map(|name| wit_txn::Host::stage_create_node(state, name.into()).unwrap());
        assert_eq!(ordinals, [0, 1]);
        let info = wit_txn::Host::window(state);
        assert_eq!((info.ops, info.readings), (3, 1));
        window.lock().unwrap().take();
        assert_eq!(wit_txn::Host::stage_create_node(state, "c".into()).unwrap(), 0, "a flushed window restarts ordinals");
        kv::Host::set(state, "a".into(), "1".into()).unwrap();
        let too_big = format!("\"{}\"", "x".repeat(80));
        assert!(matches!(kv::Host::set(state, "b".into(), too_big), Err(HostError::BudgetExceeded(_))));
        assert_eq!(kv::Host::list_keys(state, "".into(), 10).unwrap(), vec!["a".to_string()]);
    }
}
