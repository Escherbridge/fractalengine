//! Tick scheduler for the plugin worker; see src/AGENTS.md §tick-scheduler.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::Resource;
use crossbeam::channel::{Receiver, Sender, TryRecvError};
use fe_sdk::manifest_v2::{
    ExtensionManifest, HOST_CADENCE_FLOOR_MS, HOST_FUEL_CEILING, HOST_MAX_OPS_CEILING,
    HOST_MAX_READINGS_CEILING,
};
use fe_sdk::plugin_host::TransactionReceipt;
use serde::{Deserialize, Serialize};

use crate::scene_host::PluginContexts;
use crate::transaction::{StagedFlush, StagedWindow};

/// Fuel for each hook call when the manifest names none.
pub const DEFAULT_TICK_FUEL: u64 = 1_000_000;
/// A flush with no answer after this long is reported `unknown` and never retried.
pub const FLUSH_TIMEOUT_MS: u64 = 10_000;
/// Real time `stop` waits for a final flush to answer.
const FINAL_FLUSH_WAIT: Duration = Duration::from_millis(1_000);
/// Bus work units (call results, commands, event batches) one instance runs per worker turn.
pub const BUS_CALLS_PER_POLL: usize = 8;

/// The scheduler's time source: the wall clock in production, a `SimClock` adapter in the lab.
pub trait Clock: Send + Sync {
    /// Current time in epoch milliseconds.
    fn now_ms(&self) -> u64;
}

/// Production clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct WallClock;

impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    }
}

/// A clock moved by hand; clones share one time.
#[derive(Debug, Default, Clone)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// Set the current time.
    pub fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// The effective cadence budget: the manifest's `tick` block with defaults and host ceilings applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickBudget {
    /// `on-tick` period.
    pub cadence_ms: u64,
    /// Scene ops per flush.
    pub max_ops: u32,
    /// Readings per flush.
    pub max_readings: u32,
    /// Fuel for each hook call.
    pub fuel: u64,
}

impl TickBudget {
    /// Apply the manifest defaults (R4) and clamp to the host ceilings.
    pub fn from_manifest(tick: &fe_sdk::manifest_v2::TickBudget, manifest: &ExtensionManifest) -> Self {
        let writes = manifest
            .scene
            .as_ref()
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| !kinds.is_empty());
        let default_ops = if writes { HOST_MAX_OPS_CEILING } else { 0 };
        let default_readings = if manifest.readings.is_some() { HOST_MAX_READINGS_CEILING } else { 0 };
        Self {
            cadence_ms: tick.cadence_ms.max(HOST_CADENCE_FLOOR_MS),
            max_ops: tick.max_ops.unwrap_or(default_ops).min(HOST_MAX_OPS_CEILING),
            max_readings: tick.max_readings.unwrap_or(default_readings).min(HOST_MAX_READINGS_CEILING),
            fuel: tick.fuel.unwrap_or(DEFAULT_TICK_FUEL).min(HOST_FUEL_CEILING),
        }
    }
}

/// Why an instance stops (mirrors the WIT `deactivate-reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeactivateReason {
    Disabled,
    Unbound,
    Updating,
    HostStopping,
    RoleLost,
    PolicyDenied,
    Revoked,
    Degraded,
}

impl DeactivateReason {
    /// These reasons arrive with the lease already gone, so no final flush is attempted.
    pub const fn lease_gone(self) -> bool {
        matches!(self, Self::RoleLost | Self::PolicyDenied | Self::Revoked | Self::Degraded)
    }
}

/// The outcome of the previous flush, reported in the next `tick-info`.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitOutcome {
    /// Nothing was flushed.
    None,
    /// Committed whole.
    Committed { tick: u64, ops: u32, readings: u32, created_node_ids: Vec<String> },
    /// Refused whole; nothing landed.
    Refused(String),
    /// Transport timeout: may or may not have landed; never retried.
    Unknown,
}

/// One scheduled tick as the guest sees it (mirrors the WIT `tick-info`).
#[derive(Debug, Clone, PartialEq)]
pub struct TickInfo {
    pub tick: u64,
    pub now_ms: u64,
    pub scheduled_at_ms: u64,
    pub delta_ms: u32,
    pub skipped: u32,
    pub last_commit: CommitOutcome,
}

/// Why a hook call failed; either way the instance degrades and its window is discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickFault {
    FuelExhausted,
    Trap(String),
}

/// A guest instance the scheduler drives; the wasm component host implements it.
pub trait TickInstance: Send {
    /// Refill fuel before the hook (`WasmEngine::refuel_for_tick` for a core store).
    fn refuel(&mut self, fuel: u64) -> Result<(), String>;
    /// Run `on-tick`; writes land in the shared window the host imports stage into.
    fn on_tick(&mut self, info: &TickInfo) -> Result<(), TickFault>;
    /// Run `on-deactivate`; anything staged gets one final flush unless the lease is gone.
    fn on_deactivate(&mut self, reason: DeactivateReason);
}

/// The window the instance's host imports stage into and the scheduler flushes.
pub type SharedWindow = Arc<Mutex<StagedWindow>>;

/// The answer to one flush.
#[derive(Debug, Clone)]
pub enum FlushReply {
    Committed(TransactionReceipt),
    Refused(String),
    Unknown,
}

/// Where flushes go; must not block the worker and must answer each submit exactly once.
pub trait TickSink: Send {
    /// Start committing `flush` (tick `tick`) and send the answer on `reply`.
    fn submit(&mut self, tick: u64, flush: StagedFlush, reply: Sender<FlushReply>);
}

/// Production sink: commits through a [`PluginContext`](crate::context::PluginContext) on a short-lived thread.
pub struct ContextTickSink {
    context: crate::context::PluginContext,
}

impl ContextTickSink {
    pub fn new(context: crate::context::PluginContext) -> Self {
        Self { context }
    }
}

impl TickSink for ContextTickSink {
    fn submit(&mut self, _tick: u64, flush: StagedFlush, reply: Sender<FlushReply>) {
        let context = self.context.clone();
        let spawned = std::thread::Builder::new().name("fe-plugin-flush".into()).spawn({
            let reply = reply.clone();
            move || {
                let answer = match context.commit_staged(&flush) {
                    Ok(receipt) => FlushReply::Committed(receipt),
                    Err(message) if is_unknown_outcome(&message) => FlushReply::Unknown,
                    Err(message) => FlushReply::Refused(message),
                };
                let _ = reply.send(answer);
            }
        });
        if spawned.is_err() {
            let _ = reply.send(FlushReply::Refused("flush thread could not start".into()));
        }
    }
}

/// Sink errors that mean the commit may have landed (receipt mismatch, transport timeout).
fn is_unknown_outcome(message: &str) -> bool {
    message.contains("unknown outcome") || message.contains("requires reconciliation")
}

/// Lifecycle of one scheduled instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickState {
    Running,
    Degraded(String),
    Stopped,
}

/// Per-instance counters behind `GET /api/v1/extensions/{installation}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickCounters {
    pub ticks_run: u64,
    pub ticks_skipped: u64,
    pub flushes: u64,
    pub commits: u64,
    pub refusals: u64,
    pub unknown_outcomes: u64,
    pub flush_in_flight: bool,
    pub state: TickState,
}

impl Default for TickCounters {
    fn default() -> Self {
        Self {
            ticks_run: 0,
            ticks_skipped: 0,
            flushes: 0,
            commits: 0,
            refusals: 0,
            unknown_outcomes: 0,
            flush_in_flight: false,
            state: TickState::Running,
        }
    }
}

/// What one `poll` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickEvent {
    /// A tick ran; `flushed` says whether a window went out.
    Ran { tick: u64, flushed: bool },
    /// The due instant was dropped because the previous flush had not answered.
    Skipped { count: u32 },
    /// The hook faulted; the window was discarded and the instance stopped ticking.
    Degraded(String),
}

struct InFlight {
    started_ms: u64,
    reply: Receiver<FlushReply>,
}

/// Deadline scheduler for one instance (R4): skip-and-count, one flush in flight, injected clock.
pub struct TickScheduler {
    budget: TickBudget,
    clock: Arc<dyn Clock>,
    next_due_ms: u64,
    last_run_ms: Option<u64>,
    skipped_since_run: u32,
    last_commit: CommitOutcome,
    in_flight: Option<InFlight>,
    /// (tick, ops, readings) of the flush in flight, echoed in its `Committed` outcome.
    pending_shape: (u64, u32, u32),
    window: SharedWindow,
    instance: Box<dyn TickInstance>,
    sink: Box<dyn TickSink>,
    counters: Arc<Mutex<TickCounters>>,
    state: TickState,
}

impl TickScheduler {
    /// The first tick is due one cadence after `clock.now_ms()`.
    pub fn new(
        budget: TickBudget,
        clock: Arc<dyn Clock>,
        window: SharedWindow,
        instance: Box<dyn TickInstance>,
        sink: Box<dyn TickSink>,
    ) -> Self {
        let next_due_ms = clock.now_ms() + budget.cadence_ms;
        Self {
            budget,
            clock,
            next_due_ms,
            last_run_ms: None,
            skipped_since_run: 0,
            last_commit: CommitOutcome::None,
            in_flight: None,
            pending_shape: (0, 0, 0),
            window,
            instance,
            sink,
            counters: Arc::new(Mutex::new(TickCounters::default())),
            state: TickState::Running,
        }
    }

    /// A shared handle for status reads that never waits on a running hook.
    pub fn counters_handle(&self) -> Arc<Mutex<TickCounters>> {
        self.counters.clone()
    }

    /// The instant the next tick is due, while the instance is running.
    pub fn next_due_ms(&self) -> Option<u64> {
        (self.state == TickState::Running).then_some(self.next_due_ms)
    }

    fn update(&self, change: impl FnOnce(&mut TickCounters)) {
        if let Ok(mut counters) = self.counters.lock() {
            change(&mut counters);
        }
    }

    fn settle(&mut self, reply: FlushReply, tick: u64, ops: u32, readings: u32) {
        self.in_flight = None;
        self.last_commit = match reply {
            FlushReply::Committed(receipt) => {
                self.update(|c| c.commits += 1);
                CommitOutcome::Committed { tick, ops, readings, created_node_ids: receipt.created_node_ids }
            }
            FlushReply::Refused(message) => {
                self.update(|c| c.refusals += 1);
                CommitOutcome::Refused(message)
            }
            FlushReply::Unknown => {
                self.update(|c| c.unknown_outcomes += 1);
                CommitOutcome::Unknown
            }
        };
        self.update(|c| c.flush_in_flight = false);
    }

    /// Collect a finished flush answer, or time a silent one out as `unknown`.
    fn collect_reply(&mut self, now_ms: u64) {
        let Some(in_flight) = &self.in_flight else { return };
        let answer = match in_flight.reply.try_recv() {
            Ok(reply) => Some(reply),
            Err(TryRecvError::Disconnected) => Some(FlushReply::Unknown),
            Err(TryRecvError::Empty) if now_ms.saturating_sub(in_flight.started_ms) >= FLUSH_TIMEOUT_MS => {
                Some(FlushReply::Unknown)
            }
            Err(TryRecvError::Empty) => None,
        };
        if let Some(reply) = answer {
            let (tick, ops, readings) = self.pending_shape;
            self.settle(reply, tick, ops, readings);
        }
    }

    /// Run whatever is due now: at most one tick, after settling any finished flush.
    pub fn poll(&mut self) -> Option<TickEvent> {
        if self.state != TickState::Running {
            return None;
        }
        let now = self.clock.now_ms();
        self.collect_reply(now);
        if now < self.next_due_ms {
            return None;
        }
        // Skip to the latest due instant; earlier ones are counted, never queued.
        let missed = (now - self.next_due_ms) / self.budget.cadence_ms;
        let scheduled_at = self.next_due_ms + missed * self.budget.cadence_ms;
        self.next_due_ms = scheduled_at + self.budget.cadence_ms;
        if self.in_flight.is_some() {
            let count = (missed + 1) as u32;
            self.skipped_since_run += count;
            self.update(|c| c.ticks_skipped += u64::from(count));
            return Some(TickEvent::Skipped { count });
        }
        if missed > 0 {
            self.skipped_since_run += missed as u32;
            self.update(|c| c.ticks_skipped += missed);
        }
        Some(self.run_tick(now, scheduled_at))
    }

    fn run_tick(&mut self, now: u64, scheduled_at: u64) -> TickEvent {
        let tick = self.counters.lock().map_or(0, |c| c.ticks_run) + 1;
        let info = TickInfo {
            tick,
            now_ms: now,
            scheduled_at_ms: scheduled_at,
            delta_ms: self.last_run_ms.map_or(0, |previous| now.saturating_sub(previous).min(u64::from(u32::MAX)) as u32),
            skipped: self.skipped_since_run,
            last_commit: std::mem::replace(&mut self.last_commit, CommitOutcome::None),
        };
        self.update(|c| c.ticks_run += 1);
        self.last_run_ms = Some(now);
        self.skipped_since_run = 0;
        let outcome = self
            .instance
            .refuel(self.budget.fuel)
            .map_err(TickFault::Trap)
            .and_then(|()| self.instance.on_tick(&info));
        if let Err(fault) = outcome {
            let reason = match fault {
                TickFault::FuelExhausted => "fuel exhausted".to_string(),
                TickFault::Trap(message) => format!("trap: {message}"),
            };
            if let Ok(mut window) = self.window.lock() {
                window.discard();
            }
            self.state = TickState::Degraded(reason.clone());
            self.update(|c| c.state = TickState::Degraded(reason.clone()));
            return TickEvent::Degraded(reason);
        }
        TickEvent::Ran { tick, flushed: self.flush(tick) }
    }

    /// Take the window and hand it to the sink; false when nothing was staged.
    fn flush(&mut self, tick: u64) -> bool {
        let flush = match self.window.lock() {
            Ok(mut window) if !window.is_empty() => window.take(),
            _ => return false,
        };
        self.pending_shape = (tick, flush.ops.len() as u32, flush.readings.len() as u32);
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.in_flight = Some(InFlight { started_ms: self.clock.now_ms(), reply: receiver });
        self.update(|c| {
            c.flushes += 1;
            c.flush_in_flight = true;
        });
        self.sink.submit(tick, flush, reply);
        true
    }

    /// Stop the instance: run `on-deactivate`, give staged writes one final flush unless the lease is gone.
    pub fn stop(&mut self, reason: DeactivateReason) {
        if self.state == TickState::Stopped {
            return;
        }
        if !matches!(self.state, TickState::Degraded(_)) {
            self.instance.on_deactivate(reason);
        }
        if reason.lease_gone() || matches!(self.state, TickState::Degraded(_)) {
            if let Ok(mut window) = self.window.lock() {
                window.discard();
            }
        } else {
            self.await_in_flight();
            let tick = self.counters.lock().map_or(0, |c| c.ticks_run);
            if self.flush(tick) {
                self.await_in_flight();
            }
        }
        self.state = TickState::Stopped;
        self.update(|c| c.state = TickState::Stopped);
    }

    /// Wait (bounded real time) for the outstanding flush so the final one respects one-in-flight.
    fn await_in_flight(&mut self) {
        let Some(in_flight) = &self.in_flight else { return };
        let reply = in_flight.reply.recv_timeout(FINAL_FLUSH_WAIT).unwrap_or(FlushReply::Unknown);
        let (tick, ops, readings) = self.pending_shape;
        self.settle(reply, tick, ops, readings);
    }
}

/// A scheduler-level event the Bevy side applies to the plugin registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickHostEvent {
    pub plugin_id: String,
    pub kind: TickHostEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickHostEventKind {
    /// The instance faulted and was unbound.
    Degraded(String),
    /// The host stopped the instance for this reason.
    Stopped(DeactivateReason),
}

/// An instance's seat on the integration bus: registered with its scheduler, dispatched while it runs,
/// unregistered when it stops or degrades (src/AGENTS.md §bus-seat).
struct BusSeat {
    bus: crate::bus::Bus,
    key: crate::bus::InstanceKey,
    hooks: Box<dyn crate::bus::BusHooks + Send>,
}

#[derive(Default)]
struct TickHostInner {
    entries: HashMap<String, TickScheduler>,
    counters: HashMap<String, Arc<Mutex<TickCounters>>>,
    seats: HashMap<String, BusSeat>,
    events: Vec<TickHostEvent>,
}

/// The bus `ended` reason a stop shows subscribers.
fn end_reason(reason: DeactivateReason) -> crate::bus::EndReason {
    use crate::bus::EndReason;
    match reason {
        DeactivateReason::Revoked => EndReason::Revoked,
        DeactivateReason::PolicyDenied => EndReason::PolicyDenied,
        DeactivateReason::RoleLost => EndReason::RoleLost,
        _ => EndReason::InstanceStopped,
    }
}

/// Every scheduled instance on the plugin worker; polled from `plugin_thread_loop`, never a Bevy system.
#[derive(Clone, Resource)]
pub struct TickHost {
    inner: Arc<Mutex<TickHostInner>>,
    clock: Arc<dyn Clock>,
    contexts: PluginContexts,
}

impl Default for TickHost {
    fn default() -> Self {
        Self::new(Arc::new(WallClock), PluginContexts::default())
    }
}

impl TickHost {
    /// `clock` is the wall clock in production and the lab's `SimClock` adapter only in fe-sim.
    pub fn new(clock: Arc<dyn Clock>, contexts: PluginContexts) -> Self {
        Self { inner: Arc::default(), clock, contexts }
    }

    /// The same contexts the host unbinds on degrade or stop.
    pub fn with_contexts(mut self, contexts: PluginContexts) -> Self {
        self.contexts = contexts;
        self
    }

    /// The injected clock, for building schedulers that share it.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    /// Start scheduling an instance; one scheduler per plugin id.
    pub fn add(&self, plugin_id: &str, scheduler: TickScheduler) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "tick host lock poisoned")?;
        if inner.entries.contains_key(plugin_id) {
            return Err("plugin instance is already scheduled".into());
        }
        inner.counters.insert(plugin_id.to_string(), scheduler.counters_handle());
        inner.entries.insert(plugin_id.to_string(), scheduler);
        Ok(())
    }

    /// [`add`](Self::add) plus the instance's bus surface: `register_instance` first, then the scheduler.
    pub fn add_on_bus(
        &self,
        plugin_id: &str,
        scheduler: TickScheduler,
        bus: crate::bus::Bus,
        spec: crate::bus::InstanceSpec,
        hooks: Box<dyn crate::bus::BusHooks + Send>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "tick host lock poisoned")?;
        if inner.entries.contains_key(plugin_id) {
            return Err("plugin instance is already scheduled".into());
        }
        let key = bus.register_instance(spec).map_err(|error| error.to_string())?;
        inner.counters.insert(plugin_id.to_string(), scheduler.counters_handle());
        inner.entries.insert(plugin_id.to_string(), scheduler);
        inner.seats.insert(plugin_id.to_string(), BusSeat { bus, key, hooks });
        Ok(())
    }

    /// Counters for status reads; kept after the instance stops until `forget`.
    pub fn counters(&self, plugin_id: &str) -> Option<TickCounters> {
        let handle = self.inner.lock().ok()?.counters.get(plugin_id)?.clone();
        let counters = handle.lock().ok()?.clone();
        Some(counters)
    }

    /// Drop a stopped instance's counters (uninstall).
    pub fn forget(&self, plugin_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(seat) = inner.seats.remove(plugin_id) {
                seat.bus.unregister_instance(&seat.key, crate::bus::EndReason::InstanceStopped);
            }
            inner.entries.remove(plugin_id);
            inner.counters.remove(plugin_id);
        }
    }

    /// How long the worker may block on its command channel before a tick is due.
    pub fn next_wait(&self, cap: Duration) -> Duration {
        let now = self.clock.now_ms();
        let due = self
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.entries.values().filter_map(TickScheduler::next_due_ms).min());
        match due {
            Some(due) => Duration::from_millis(due.saturating_sub(now).max(1)).min(cap),
            None => cap,
        }
    }

    /// Poll every scheduler once, then run each running instance's bus work; a faulted instance leaves
    /// the bus, is unbound and reported.
    pub fn poll(&self) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let mut degraded = Vec::new();
        for (plugin_id, scheduler) in inner.entries.iter_mut() {
            if let Some(TickEvent::Degraded(reason)) = scheduler.poll() {
                degraded.push((plugin_id.clone(), reason));
            }
        }
        for (plugin_id, reason) in degraded {
            if let Some(seat) = inner.seats.remove(&plugin_id) {
                seat.bus.unregister_instance(&seat.key, crate::bus::EndReason::InstanceStopped);
            }
            if let Err(error) = self.contexts.unbind(&plugin_id) {
                tracing::warn!(plugin = %plugin_id, %error, "unbinding a degraded instance failed");
            }
            inner.events.push(TickHostEvent { plugin_id, kind: TickHostEventKind::Degraded(reason) });
        }
        // Priority inside `dispatch`: call results, then commands, then one event batch.
        let TickHostInner { entries, seats, .. } = &mut *inner;
        for (plugin_id, seat) in seats.iter_mut() {
            if entries.get(plugin_id).is_some_and(|scheduler| scheduler.next_due_ms().is_some()) {
                seat.bus.dispatch(&seat.key, seat.hooks.as_mut(), BUS_CALLS_PER_POLL);
            }
        }
    }

    /// Stop one instance: leave the bus first (queued calls answer `not-available`, no new work), then
    /// run the final flush unless the lease is gone (OD-E1: drain in-flight writes), then revoke its context.
    pub fn stop(&self, plugin_id: &str, reason: DeactivateReason) {
        let Ok(mut inner) = self.inner.lock() else { return };
        if let Some(seat) = inner.seats.remove(plugin_id) {
            seat.bus.unregister_instance(&seat.key, end_reason(reason));
        }
        let known = inner.entries.get_mut(plugin_id).map(|scheduler| scheduler.stop(reason)).is_some();
        if known {
            if let Err(error) = self.contexts.unbind(plugin_id) {
                tracing::warn!(plugin = %plugin_id, %error, "unbinding a stopped instance failed");
            }
            inner.events.push(TickHostEvent { plugin_id: plugin_id.to_string(), kind: TickHostEventKind::Stopped(reason) });
        }
    }

    /// Stop everything (worker exit).
    pub fn stop_all(&self, reason: DeactivateReason) {
        let ids: Vec<String> = self.inner.lock().map(|inner| inner.entries.keys().cloned().collect()).unwrap_or_default();
        for plugin_id in ids {
            self.stop(&plugin_id, reason);
        }
    }

    /// Events since the last call, for the registry-applying system.
    pub fn take_events(&self) -> Vec<TickHostEvent> {
        self.inner.lock().map(|mut inner| std::mem::take(&mut inner.events)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::plugin_host::{PendingOp, StagedReading};

    /// Stages one SetTransform per tick and records every `TickInfo`; faults on `fail_on`.
    struct ScriptedInstance {
        window: SharedWindow,
        infos: Arc<Mutex<Vec<TickInfo>>>,
        fuel_refills: Arc<Mutex<Vec<u64>>>,
        fail_on: Option<u64>,
    }

    impl TickInstance for ScriptedInstance {
        fn refuel(&mut self, fuel: u64) -> Result<(), String> {
            self.fuel_refills.lock().unwrap().push(fuel);
            Ok(())
        }
        fn on_tick(&mut self, info: &TickInfo) -> Result<(), TickFault> {
            self.infos.lock().unwrap().push(info.clone());
            if self.fail_on == Some(info.tick) {
                return Err(TickFault::FuelExhausted);
            }
            self.window
                .lock()
                .unwrap()
                .stage_op(PendingOp::SetTransform { node_id: "n1".into(), position: Some([info.tick as f64, 0.0, 0.0]), rotation: None, scale: None })
                .map_err(|error| TickFault::Trap(error.to_string()))
        }
        fn on_deactivate(&mut self, _reason: DeactivateReason) {}
    }

    /// Bus hooks that answer nothing; J1 checks the seat lifecycle, the bus journey checks delivery.
    struct NoHooks;

    impl crate::bus::BusHooks for NoHooks {
        fn on_events(&mut self, _events: Vec<crate::bus::BusEvent>) {}
        fn on_command(&mut self, _command: &str, _args: &str, _caller: &crate::bus::Caller) -> Result<String, crate::bus::CommandError> {
            Ok("{}".into())
        }
        fn on_call_result(&mut self, _call: u64, _outcome: Result<String, crate::bus::HostError>) {}
    }

    /// Answers each flush `delay_ms` of manual time after it was submitted.
    struct SlowSink {
        clock: ManualClock,
        delay_ms: u64,
        pending: Arc<Mutex<Vec<(u64, Sender<FlushReply>, u32)>>>,
        submitted: Arc<Mutex<Vec<StagedFlush>>>,
    }

    impl TickSink for SlowSink {
        fn submit(&mut self, _tick: u64, flush: StagedFlush, reply: Sender<FlushReply>) {
            let release_at = self.clock.now_ms() + self.delay_ms;
            self.pending.lock().unwrap().push((release_at, reply, flush.ops.len() as u32));
            self.submitted.lock().unwrap().push(flush);
        }
    }

    struct Rig {
        clock: ManualClock,
        scheduler: TickScheduler,
        window: SharedWindow,
        infos: Arc<Mutex<Vec<TickInfo>>>,
        fuel_refills: Arc<Mutex<Vec<u64>>>,
        pending: Arc<Mutex<Vec<(u64, Sender<FlushReply>, u32)>>>,
        submitted: Arc<Mutex<Vec<StagedFlush>>>,
    }

    fn rig(delay_ms: u64, fail_on: Option<u64>, fuel: u64) -> Rig {
        let clock = ManualClock::default();
        let window: SharedWindow = Arc::new(Mutex::new(StagedWindow::new(64, 1_000)));
        let infos = Arc::new(Mutex::new(Vec::new()));
        let fuel_refills = Arc::new(Mutex::new(Vec::new()));
        let pending = Arc::new(Mutex::new(Vec::new()));
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let scheduler = TickScheduler::new(
            TickBudget { cadence_ms: 500, max_ops: 64, max_readings: 1_000, fuel },
            Arc::new(clock.clone()),
            window.clone(),
            Box::new(ScriptedInstance { window: window.clone(), infos: infos.clone(), fuel_refills: fuel_refills.clone(), fail_on }),
            Box::new(SlowSink { clock: clock.clone(), delay_ms, pending: pending.clone(), submitted: submitted.clone() }),
        );
        Rig { clock, scheduler, window, infos, fuel_refills, pending, submitted }
    }

    impl Rig {
        /// Advance manual time in 50 ms steps, answering due flushes and polling the scheduler.
        fn run_until(&mut self, end_ms: u64) {
            let mut now = self.clock.now_ms();
            while now < end_ms {
                now += 50;
                self.clock.set(now);
                let due: Vec<_> = {
                    let mut pending = self.pending.lock().unwrap();
                    let (due, rest): (Vec<_>, Vec<_>) = pending.drain(..).partition(|(release_at, _, _)| *release_at <= now);
                    *pending = rest;
                    due
                };
                for (_, reply, ops) in due {
                    let _ = reply.send(FlushReply::Committed(TransactionReceipt { op_count: ops as usize, created_node_ids: vec![] }));
                }
                self.scheduler.poll();
                assert!(self.pending.lock().unwrap().len() <= 1, "never more than one flush in flight");
            }
        }
    }

    /// J1: skip-and-count under a slow sink, then fuel exhaustion degrades without a flush.
    #[test]
    fn tick_journey_skips_counts_and_degrades_without_flushing() {
        let mut slow = rig(750, None, 200_000);
        slow.run_until(10_000);
        let counters = slow.scheduler.counters_handle().lock().unwrap().clone();
        assert_eq!(counters.ticks_run + counters.ticks_skipped, 20, "every due instant is run or skipped, none queued");
        assert_eq!((counters.ticks_run, counters.ticks_skipped), (10, 10));
        let submitted = slow.submitted.lock().unwrap();
        assert_eq!(submitted.len(), 10);
        assert!(submitted.iter().all(|flush| flush.ops.len() == 1 && flush.readings.is_empty()), "each flush carries exactly the staged op");
        let infos = slow.infos.lock().unwrap();
        assert_eq!((infos[0].delta_ms, infos[0].skipped), (0, 0));
        assert!(infos[1..].iter().all(|info| info.delta_ms == 1_000 && info.skipped == 1), "true delta-ms and the skipped due instant");
        assert!(matches!(infos[1].last_commit, CommitOutcome::Committed { ops: 1, readings: 0, .. }));
        assert_eq!(*slow.fuel_refills.lock().unwrap(), vec![200_000; 10], "every run tick is refuelled first");
        drop((submitted, infos));

        // Staged-window bounds hold at the seam the host imports use.
        let mut small = StagedWindow::new(1, 1);
        let reading = StagedReading { node_id: "n1".into(), metric: "temperature_c".into(), value: 1.0, units: String::new(), recorded_at: None };
        small.stage_op(PendingOp::DeleteNode { node_id: "n1".into() }).unwrap();
        assert!(small.stage_op(PendingOp::DeleteNode { node_id: "n2".into() }).is_err());
        small.stage_reading(reading.clone()).unwrap();
        assert!(small.stage_reading(reading).is_err());
        assert_eq!((small.info().ops, small.info().readings), (1, 1));

        // Fuel exhaustion on tick 2: state degraded, window discarded, no new flush.
        let mut fuel = rig(0, Some(2), 200_000);
        fuel.run_until(2_000);
        let counters = fuel.scheduler.counters_handle().lock().unwrap().clone();
        assert!(matches!(counters.state, TickState::Degraded(_)));
        assert_eq!(fuel.submitted.lock().unwrap().len(), 1, "only tick 1 flushed");
        assert!(fuel.window.lock().unwrap().is_empty());
        assert_eq!(fuel.scheduler.next_due_ms(), None);

        // G1: a seated instance joins the bus with its scheduler and leaves it when stopped; counters outlive it.
        const LOCAL: &str = "did:key:z6MkLocalHost";
        let fleet = include_str!("../../fe-sdk/schema/examples/fleet-sim.extension.json");
        let manifest = fe_sdk::manifest_v2::parse_and_validate(fleet.as_bytes()).unwrap().manifest;
        let spec = crate::bus::InstanceSpec {
            installation_id: "inst-1".into(),
            content_hash: "ab".repeat(32),
            verse_id: "v1".into(),
            petal_id: "p1".into(),
            identity_did: LOCAL.into(),
            manifest: Arc::new(manifest),
        };
        let key = spec.key();
        let bus = crate::bus::Bus::new(Arc::new(ManualClock::default()), crate::bus::BusSources::fail_closed(), LOCAL);
        let host = TickHost::new(Arc::new(ManualClock::default()), PluginContexts::default());
        host.add_on_bus("inst-1", rig(0, None, 200_000).scheduler, bus.clone(), spec, Box::new(NoHooks)).unwrap();
        assert!(bus.counters(&key).is_some(), "registered with its scheduler");
        host.stop("inst-1", DeactivateReason::Disabled);
        assert!(bus.counters(&key).is_none(), "a stopped instance leaves the bus");
        assert!(bus.counters_for_installation("inst-1").is_some(), "its counters stay until uninstall");
    }
}
