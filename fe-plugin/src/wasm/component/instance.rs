//! A live 2.0 instance: its store, hook calls, the `TickInstance` seam and the instance store;
//! see src/wasm/AGENTS.md §instance.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use wasmtime::Store;

use super::bindings::exports::fractalengine::plugin::bus_hooks::{BusEvent, CommandError};
use super::bindings::exports::fractalengine::plugin::ingress_hooks::{ConnectionEvent, IngressFrame, IngressReply};
use super::bindings::exports::fractalengine::plugin::lifecycle;
use super::bindings::fractalengine::plugin::egress::IncomingResponse;
use super::bindings::fractalengine::plugin::txn;
use super::bindings::fractalengine::plugin::types::{Binding, Caller, HostError};
use super::bindings::Extension;
use super::epoch::NO_EPOCH_DEADLINE;
use super::hooks::{HookCallError, HookReply, HookRequest, HooksGuest};
use super::state::{refusal_error, InstanceState};
use super::{start_store, Respawn};
use crate::bindings::decode::DecodeStamp;
use crate::capability::CapabilityToken;
use crate::tick::{self, Clock, SharedWindow, TickFault, TickInstance};
use crate::transaction::StagedWindow;

/// Why a hook call did not complete. Faults discard the window; a hook fault restarts the instance
/// (src/wasm/AGENTS.md §hooks, "crash recovery"), any other fault degrades it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookFault {
    #[error("fuel exhausted")]
    FuelExhausted,
    /// The guest was still running at the hook deadline (epoch interruption).
    #[error("interrupted at the hook deadline")]
    Interrupted,
    #[error("trap: {0}")]
    Trap(String),
    /// The instance degraded earlier; no guest code runs any more.
    #[error("instance degraded: {0}")]
    Degraded(String),
}

/// First wait after a hook crash before the restarted instance serves hooks again; doubles per crash in a window.
pub const HOOK_RESTART_BACKOFF_MS: u64 = 1_000;
/// Window over which hook-crash restarts are counted.
pub const HOOK_RESTART_WINDOW_MS: u64 = 60_000;
/// Most hook-crash restarts in one window; past it hooks wait until the oldest leaves the window.
pub const MAX_HOOK_RESTARTS_PER_WINDOW: usize = 5;

/// Per-instance counters beside the scheduler's `TickCounters` (status route, R11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceStats {
    pub hook_calls: u64,
    pub fuel_exhaustions: u64,
    pub traps: u64,
    /// Fresh instances started after a hook crash.
    pub restarts: u64,
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

/// A restarted instance waiting out its backoff after a hook crash: hook calls are refused until `retry_at_ms`.
#[derive(Debug, Clone)]
struct Recovering {
    reason: String,
    retry_at_ms: u64,
}

/// One instantiated component bound to one petal (OD-E3).
pub struct ExtensionInstance {
    store: Store<InstanceState>,
    bindings: Extension,
    clock: Arc<dyn Clock>,
    hook_fuel: u64,
    degraded: Option<String>,
    stats: InstanceStats,
    /// The optional `hooks` export, bound to this same instance (src/AGENTS.md §hook-calls).
    hooks: Option<HooksGuest>,
    /// The cached component a crashed instance restarts from; `None` degrades on a hook fault instead.
    respawn: Option<Respawn>,
    recovering: Option<Recovering>,
    /// Restart instants (clock ms) inside the current window.
    restarts: VecDeque<u64>,
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
            hooks: None,
            respawn: None,
            recovering: None,
            restarts: VecDeque::new(),
        }
    }

    /// Keep the bound `hooks` export (`None` for a plain 2.0 component).
    pub(crate) fn with_hooks(mut self, hooks: Option<HooksGuest>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Restart from `respawn` after a hook crash instead of degrading.
    pub(crate) fn with_respawn(mut self, respawn: Respawn) -> Self {
        self.respawn = Some(respawn);
        self
    }

    /// Per-hook counters of the `hooks` export, when the component has one.
    pub fn hook_stats(&self) -> Option<&super::hooks::HookStats> {
        self.hooks.as_ref().map(HooksGuest::stats)
    }

    /// One host-initiated hook call: refused while degraded or recovering; a fault restarts the instance from the
    /// cached component and refuses hooks for a backoff (degrades when it cannot restart). Readings staged during
    /// the call see `stamp` through `InstanceState::decode_stamp`.
    pub fn call_hook(&mut self, request: &HookRequest, stamp: Option<&DecodeStamp>) -> Result<HookReply, HookCallError> {
        if let Some(reason) = &self.degraded {
            return Err(HookCallError::Fault(HookFault::Degraded(reason.clone())));
        }
        let now = self.clock.now_ms();
        if let Some(recovering) = &self.recovering {
            if now < recovering.retry_at_ms {
                return Err(HookCallError::Fault(HookFault::Degraded(format!(
                    "{}; restarted, serving again in {} ms",
                    recovering.reason,
                    recovering.retry_at_ms - now
                ))));
            }
            self.recovering = None;
        }
        let Some(guest) = self.hooks.as_mut() else {
            return Err(HookCallError::NotServed(request.hook.clone()));
        };
        self.stats.hook_calls += 1;
        self.store.data_mut().decode_stamp = stamp.cloned();
        let outcome = guest.call(&mut self.store, request, now);
        self.store.data_mut().decode_stamp = None;
        if let Err(HookCallError::Fault(fault)) = &outcome {
            match fault {
                HookFault::FuelExhausted => self.stats.fuel_exhaustions += 1,
                _ => self.stats.traps += 1,
            }
            // `HooksGuest::call` already discarded the window; a trapped instance is not entered again.
            self.recover(fault.to_string(), now);
        }
        outcome
    }

    /// After a hook crash: start a fresh instance at once (ticks and bus keep a sound store) and refuse hooks until
    /// the backoff passes; at most [`MAX_HOOK_RESTARTS_PER_WINDOW`] restarts per window. Degrades when it cannot.
    fn recover(&mut self, reason: String, now: u64) {
        let Some(respawn) = self.respawn.clone() else {
            self.degraded = Some(reason);
            return;
        };
        while self
            .restarts
            .front()
            .is_some_and(|at| now.saturating_sub(*at) >= HOOK_RESTART_WINDOW_MS)
        {
            self.restarts.pop_front();
        }
        self.restarts.push_back(now);
        let in_window = self.restarts.len();
        let cap = HOOK_RESTART_WINDOW_MS / MAX_HOOK_RESTARTS_PER_WINDOW as u64;
        let shift = u32::try_from(in_window - 1).unwrap_or(u32::MAX).min(16);
        let mut retry_at_ms = now + (HOOK_RESTART_BACKOFF_MS << shift).min(cap);
        if in_window >= MAX_HOOK_RESTARTS_PER_WINDOW {
            // The next crash can only follow a served call, so this keeps restarts per window at the cap.
            retry_at_ms = retry_at_ms.max(self.restarts.front().copied().unwrap_or(now) + HOOK_RESTART_WINDOW_MS);
        }
        match self.restart(&respawn, now) {
            Ok(()) => {
                tracing::warn!(%reason, retry_at_ms, "extension hook crashed; instance restarted");
                self.recovering = Some(Recovering { reason, retry_at_ms });
            }
            Err(error) => {
                tracing::warn!(%reason, %error, "extension hook crashed and the instance could not restart");
                self.degraded = Some(format!("{reason}; restart failed: {error}"));
            }
        }
    }

    /// Replace the store with a fresh instance of the cached component (same binding, window, kv, readers), then
    /// run `on-activate` on it.
    fn restart(&mut self, respawn: &Respawn, now: u64) -> Result<(), String> {
        let engine = self.store.engine().clone();
        self.stats.restarts += 1;
        let seed = u64::MAX - self.stats.restarts;
        let state = self.store.data_mut().respawn(seed);
        let started =
            start_store(&engine, respawn, state, self.hook_fuel, now).map_err(|error| error.to_string())?;
        self.store = started.store;
        self.bindings = started.bindings;
        let previous = self.hooks.take();
        self.hooks = started.hooks.map(|mut guest| {
            if let Some(previous) = previous {
                guest.inherit_stats(previous);
            }
            guest
        });
        match self.on_activate() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(refused)) => Err(format!("on-activate refused: {refused}")),
            Err(fault) => Err(format!("on-activate faulted: {fault}")),
        }
    }

    /// The host-minted binding this instance runs under.
    pub fn binding(&self) -> &Binding {
        &self.store.data().binding
    }

    /// The degrade reason, once a fault could not be recovered (a hook crash restarts instead).
    pub fn degraded(&self) -> Option<&str> {
        self.degraded.as_deref()
    }

    /// The crash a restarted instance is backing off from, while hook calls are still refused.
    pub fn recovering(&self) -> Option<&str> {
        self.recovering
            .as_ref()
            .filter(|recovering| self.clock.now_ms() < recovering.retry_at_ms)
            .map(|recovering| recovering.reason.as_str())
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
        // Non-hook entries keep their fuel budget only; the epoch deadline is for hook calls.
        self.store.data_mut().hook_deadline = None;
        self.store.set_epoch_deadline(NO_EPOCH_DEADLINE);
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
        let fault = super::hooks::classify_fault(&self.store, &error);
        match fault {
            HookFault::FuelExhausted => self.stats.fuel_exhaustions += 1,
            _ => self.stats.traps += 1,
        }
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

    /// [`ExtensionInstance::call_hook`] under the instance lock (a poisoned lock is a fault).
    pub fn call_hook(&self, request: &HookRequest, stamp: Option<&DecodeStamp>) -> Result<HookReply, HookCallError> {
        let mut instance = self
            .lock()
            .map_err(|message| HookCallError::Fault(HookFault::Trap(message)))?;
        instance.call_hook(request, stamp)
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
            ("wasi:clocks/monotonic-clock@0.2.0", true, false),
            ("wasi:clocks/monotonic-clock@0.2.12", true, false),
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

        // Node-sized payloads, relative to the node limit (src/wasm/AGENTS.md §node-bounds): a full node reads,
        // a page cuts by bytes with a cursor, an over-bound node is skipped by id, and staging stops at the caps.
        use fe_sdk::plugin_host::MAX_NODE_PAYLOAD_BYTES;
        use fe_sdk::property_schema::MAX_PROPERTY_BYTES;
        let full = "x".repeat(MAX_PROPERTY_BYTES - 2);
        let huge = "x".repeat(MAX_NODE_PAYLOAD_BYTES);
        for (id, value) in [("b0", &full), ("b1", &full), ("b2", &huge), ("b3", &full), ("b4", &full), ("b5", &full)] {
            mock.lock().unwrap().insert_node(
                id,
                crate::NodeSnapshot {
                    node_id: id.into(),
                    petal_id: "p1".into(),
                    name: "big".into(),
                    position: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                    properties: HashMap::from([("fleet.blob".to_string(), PropertyValue::String(value.clone()))]),
                },
            );
        }
        state.reset_hook_budgets();
        let read = scene::Host::get_node(state, "b0".into()).unwrap().unwrap();
        assert!(read.properties.len() >= MAX_PROPERTY_BYTES, "a full-size node reads whole");
        assert!(matches!(scene::Host::get_node(state, "b2".into()), Err(HostError::BudgetExceeded(_))));
        let first = scene::Host::query_page(state, r#"{"name":"big","limit":200}"#.into()).unwrap();
        let ids = |page: &scene::NodePage| page.nodes.iter().map(|node| node.node_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&first), ["b0", "b1", "b3", "b4"], "four full nodes fill a page");
        assert_eq!((first.omitted.clone(), first.next.as_deref()), (vec!["b2".to_string()], Some("b4")));
        let rest = scene::Host::query_page(state, r#"{"name":"big","limit":200,"after":"b4"}"#.into()).unwrap();
        assert_eq!((ids(&rest), rest.next), (vec!["b5".to_string()], None));
        assert!(matches!(
            scene::Host::query_nodes(state, r#"{"name":"big","limit":200}"#.into()),
            Err(HostError::BudgetExceeded(_))
        ));
        window.lock().unwrap().take();
        let over = format!("\"{}\"", "x".repeat(MAX_PROPERTY_BYTES - 1));
        assert!(matches!(
            wit_txn::Host::stage_set_property(state, "b0".into(), "fleet.blob".into(), over),
            Err(HostError::InvalidInput(_))
        ));
        let value = format!("\"{full}\"");
        for _ in 0..4 {
            wit_txn::Host::stage_set_property(state, "b0".into(), "fleet.blob".into(), value.clone()).unwrap();
        }
        assert!(matches!(
            wit_txn::Host::stage_set_property(state, "b0".into(), "fleet.blob".into(), value),
            Err(HostError::BudgetExceeded(_))
        ));
        assert_eq!(wit_txn::Host::window(state).ops, 4, "the refused stage changed nothing");
        window.lock().unwrap().take();
        drop(instance);

        // Hook crash recovery and the wall-clock bound (src/wasm/AGENTS.md §hooks): a trap is a fault (the gate refuses
        // the write), the instance restarts at once from the cached component, hooks are refused until the backoff
        // passes, then it judges again; a guest still running at the deadline is interrupted the same way.
        let judge = host.compile(&test_guest::hooks_component()).unwrap();
        assert!(judge.exports_hooks());
        let hook_clock = ManualClock::default();
        let setup = fleet_setup(&mock, &hook_clock).with_hook_fuel(fe_sdk::manifest_v2::HOST_FUEL_CEILING);
        let mut judge = host.instantiate(&judge, setup).unwrap();
        let request = |input: &str| HookRequest {
            hook: "scene.transform".into(),
            key: String::new(),
            binding_scope: "VERSE#mock".into(),
            config: "{}".into(),
            input: input.into(),
            body: Vec::new(),
        };
        let honest = r#"{"node":"n1"}"#;
        assert_eq!(judge.call_hook(&request(honest), None).unwrap().output, r#"{"verdict":"allow"}"#);
        assert!(matches!(
            judge.call_hook(&request(r#"["trap"]"#), None),
            Err(HookCallError::Fault(HookFault::Trap(_)))
        ));
        assert_eq!((judge.degraded(), judge.stats().restarts), (None, 1), "a hook crash restarts, never degrades");
        assert!(matches!(
            judge.call_hook(&request(honest), None),
            Err(HookCallError::Fault(HookFault::Degraded(reason))) if reason.contains("restarted")
        ), "refused during the backoff");
        hook_clock.set(HOOK_RESTART_BACKOFF_MS);
        assert!(judge.call_hook(&request(honest), None).is_ok(), "the restarted instance judges again");
        // Spin: drive the epoch from here so the interrupt lands long before the 20 M fuel runs out.
        let engine = host.engine().clone();
        let ticking = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let ticker = {
            let ticking = ticking.clone();
            std::thread::spawn(move || {
                ticking.store(1, std::sync::atomic::Ordering::SeqCst);
                while ticking.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                    engine.increment_epoch();
                    std::thread::yield_now();
                }
            })
        };
        while ticking.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        let spun = judge.call_hook(&request(r#"["spin"]"#), None);
        ticking.store(2, std::sync::atomic::Ordering::SeqCst);
        ticker.join().unwrap();
        assert!(matches!(spun, Err(HookCallError::Fault(HookFault::Interrupted))), "{spun:?}");
        assert_eq!((judge.degraded(), judge.stats().restarts), (None, 2));
        assert!(judge.recovering().is_some(), "an interrupted guest backs off like a crashed one");
        // No read starts past the deadline, so host read time counts against it too.
        judge.store.data_mut().hook_deadline = Some(std::time::Instant::now());
        assert!(matches!(
            scene::Host::get_node(judge.store.data_mut(), "r1".into()),
            Err(HostError::BudgetExceeded(_))
        ));
    }

    #[test]
    fn shadowed_monotonic_clock_tracks_frozen_clock_and_subscribes_without_blocking() {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Waker};
        use wasmtime_wasi::p2::bindings::clocks::monotonic_clock::Host as MonoHost;
        use wasmtime_wasi::p2::Pollable;

        let host = ComponentHost::new().unwrap();
        let bytes = test_guest::component(test_guest::STAGE_ONE_TRANSFORM);
        let component = host.compile(&bytes).unwrap();
        let mock = Arc::new(Mutex::new(MockHostEnv::new()));
        let clock = ManualClock::default();
        clock.set(42_500);
        let mut instance = host.instantiate(&component, fleet_setup(&mock, &clock)).unwrap();
        instance.store.data().freeze_clock(42_500);

        let state = instance.store.data_mut();
        assert_eq!(MonoHost::now(state).unwrap(), 42_500_000_000);
        assert_eq!(MonoHost::resolution(state).unwrap(), 1_000_000);
        state.freeze_clock(41_000);
        assert_eq!(InstanceState::now(state), 41_000);
        assert_eq!(MonoHost::now(state).unwrap(), 42_500_000_000);
        state.freeze_clock(43_000);
        assert_eq!(MonoHost::now(state).unwrap(), 43_000_000_000);

        let dur_pollable = MonoHost::subscribe_duration(state, u64::MAX).unwrap();
        let inst_pollable = MonoHost::subscribe_instant(state, u64::MAX).unwrap();
        assert_ne!(dur_pollable.rep(), inst_pollable.rep());

        let mut ready = crate::wasm::component::wasi::ReadyPollable;
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(Pin::new(&mut ready.ready()).poll(&mut cx), Poll::Ready(())));
    }
}
