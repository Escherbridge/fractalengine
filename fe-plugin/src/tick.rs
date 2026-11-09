//! Tick scheduler and instance host for the plugin worker; see src/AGENTS.md §tick-scheduler, §bus-seat, §update.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bevy::prelude::Resource;
use crossbeam::channel::{Receiver, Sender, TryRecvError};
use fe_sdk::manifest_v2::{
    ExtensionManifest, HOST_CADENCE_FLOOR_MS, HOST_FUEL_CEILING, HOST_MAX_OPS_CEILING,
    HOST_MAX_READINGS_CEILING,
};
use fe_sdk::plugin_host::TransactionReceipt;
use serde::{Deserialize, Serialize};

use crate::context::PluginContext;
use crate::scene_host::PluginContexts;
use crate::transaction::{StagedFlush, StagedWindow};

/// Fuel for each hook call when the manifest names none.
pub const DEFAULT_TICK_FUEL: u64 = 1_000_000;
/// A flush with no answer after this long is reported `unknown` and never retried.
pub const FLUSH_TIMEOUT_MS: u64 = 10_000;
/// Real time `stop` waits for the in-flight flush and for a final flush to answer.
const FINAL_FLUSH_WAIT: Duration = Duration::from_millis(1_000);
/// Bus work units (call results, commands, event batches) one instance runs per worker turn.
pub const BUS_CALLS_PER_POLL: usize = 8;
/// Network deliveries (ingress frames, connection events, egress results) one instance runs per worker turn.
pub const NET_CALLS_PER_POLL: usize = 8;

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
    /// These reasons discard staged writes with no final flush. The commit already in flight still settles
    /// first, on the live lease; the host revokes the lease only after the stop (OD-E1).
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
    /// Run `on-deactivate`; anything staged gets one final flush unless the reason discards it.
    fn on_deactivate(&mut self, reason: DeactivateReason);
}

/// The hooks the host runs outside the tick loop (WIT `on-install`, `on-activate`, `on-uninstall`).
pub trait GuestLifecycle: Send {
    /// First activation of a binding; an error refuses activation.
    fn on_install(&mut self) -> Result<(), String>;
    /// Every activation, including after an update rollback; an error refuses activation.
    fn on_activate(&mut self) -> Result<(), String>;
    /// The binding is removed; whatever it stages gets one final flush.
    fn on_uninstall(&mut self);
    /// Why the guest refuses further hooks (a fault in any hook), if it does.
    fn degraded(&self) -> Option<String>;
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

/// Production sink: commits through a [`PluginContext`] on a short-lived thread.
pub struct ContextTickSink {
    context: PluginContext,
}

impl ContextTickSink {
    pub fn new(context: PluginContext) -> Self {
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
    /// `None` for a manifest without a `tick` block: it never ticks but still owns the stop (G1-C10).
    budget: Option<TickBudget>,
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
        Self::for_budget(Some(budget), clock, window, instance, sink)
    }

    /// A scheduler for a manifest without a `tick` block: it never ticks, yet seats and stops like any other.
    pub fn without_ticks(
        clock: Arc<dyn Clock>,
        window: SharedWindow,
        instance: Box<dyn TickInstance>,
        sink: Box<dyn TickSink>,
    ) -> Self {
        Self::for_budget(None, clock, window, instance, sink)
    }

    /// `budget` is the token's effective budget (`CapabilityToken::tick_budget().copied()`).
    pub fn for_budget(
        budget: Option<TickBudget>,
        clock: Arc<dyn Clock>,
        window: SharedWindow,
        instance: Box<dyn TickInstance>,
        sink: Box<dyn TickSink>,
    ) -> Self {
        let next_due_ms = clock.now_ms() + budget.map_or(0, |budget| budget.cadence_ms);
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

    /// Whether the instance runs (neither degraded nor stopped); a no-tick instance runs too.
    pub fn is_running(&self) -> bool {
        self.state == TickState::Running
    }

    /// The instant the next tick is due, while a ticking instance is running.
    pub fn next_due_ms(&self) -> Option<u64> {
        (self.is_running() && self.budget.is_some()).then_some(self.next_due_ms)
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
        let budget = self.budget?;
        let now = self.clock.now_ms();
        self.collect_reply(now);
        if now < self.next_due_ms {
            return None;
        }
        // Skip to the latest due instant; earlier ones are counted, never queued.
        let missed = (now - self.next_due_ms) / budget.cadence_ms;
        let scheduled_at = self.next_due_ms + missed * budget.cadence_ms;
        self.next_due_ms = scheduled_at + budget.cadence_ms;
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
        Some(self.run_tick(now, scheduled_at, budget.fuel))
    }

    fn run_tick(&mut self, now: u64, scheduled_at: u64, fuel: u64) -> TickEvent {
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
            .refuel(fuel)
            .map_err(TickFault::Trap)
            .and_then(|()| self.instance.on_tick(&info));
        if let Err(fault) = outcome {
            let reason = match fault {
                TickFault::FuelExhausted => "fuel exhausted".to_string(),
                TickFault::Trap(message) => format!("trap: {message}"),
            };
            self.degrade(reason.clone());
            return TickEvent::Degraded(reason);
        }
        TickEvent::Ran { tick, flushed: self.flush(tick) }
    }

    /// A fault: the window is discarded unflushed and the instance stops ticking.
    fn degrade(&mut self, reason: String) {
        if let Ok(mut window) = self.window.lock() {
            window.discard();
        }
        self.state = TickState::Degraded(reason.clone());
        self.update(|c| c.state = TickState::Degraded(reason));
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

    /// Flush whatever is staged now and wait (bounded) for its answer.
    fn flush_final(&mut self) {
        let tick = self.counters.lock().map_or(0, |c| c.ticks_run);
        if self.flush(tick) {
            self.await_in_flight();
        }
    }

    /// Stop (OD-E1): the commit in flight settles first, then `on-deactivate`, then one final flush unless the
    /// reason discards staged writes. The caller revokes the lease only after this returns.
    pub fn stop(&mut self, reason: DeactivateReason) {
        if self.state == TickState::Stopped {
            return;
        }
        // Drain before anything is discarded: an admitted commit lands whole with its true receipt.
        self.await_in_flight();
        let degraded = matches!(self.state, TickState::Degraded(_));
        if !degraded {
            self.instance.on_deactivate(reason);
        }
        if reason.lease_gone() || degraded {
            if let Ok(mut window) = self.window.lock() {
                window.discard();
            }
        } else {
            self.flush_final();
        }
        self.state = TickState::Stopped;
        self.update(|c| c.state = TickState::Stopped);
    }

    /// Restart a stopped scheduler (update rollback); `last_commit` keeps the final flush's outcome.
    fn resume(&mut self) {
        if self.state != TickState::Stopped {
            return;
        }
        self.next_due_ms = self.clock.now_ms() + self.budget.map_or(0, |budget| budget.cadence_ms);
        self.skipped_since_run = 0;
        self.state = TickState::Running;
        self.update(|c| c.state = TickState::Running);
    }

    /// Wait (bounded real time) for the outstanding flush so nothing is discarded or flushed over it.
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

/// The network broker an instance is mounted on, with the hooks its deliveries run through.
pub struct NetSeat {
    pub net: crate::net::Net,
    pub hooks: Box<dyn crate::net::NetHooks + Send>,
}

/// The brokers an instance is seated on beside its scheduler (src/AGENTS.md §bus-seat).
pub struct InstanceSeat {
    pub bus: crate::bus::Bus,
    /// The admitted instance; its key `(petal_id, package_id)` is shared by every build of the package.
    pub spec: crate::bus::InstanceSpec,
    pub bus_hooks: Box<dyn crate::bus::BusHooks + Send>,
    /// `None` on a host without a network broker (the lab).
    pub net: Option<NetSeat>,
}

/// One instance ready to start (`TickHost::launch` / `update`); nothing has run yet.
pub struct Launch {
    pub scheduler: TickScheduler,
    pub seat: Option<InstanceSeat>,
    /// The same guest the scheduler ticks (for a 2.0 component, a clone of its `InstanceHandle`).
    pub guest: Box<dyn GuestLifecycle>,
    /// Staged with `PluginContexts::stage` on this host's contexts; admitted only once the guest activated.
    pub context: PluginContext,
    /// Run `on-install` before `on-activate`.
    pub first_activation: bool,
}

/// Why an update did not end with the new build running (`TickHost::update`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateFailure {
    /// Nothing runs under this id, or it was not started through `launch` (no hooks to restart it).
    #[error("no running instance to update: {0}")]
    NotRunning(String),
    /// The new build did not start; the previous build was re-activated and runs again.
    #[error("update failed, previous version kept: {0}")]
    RolledBack(String),
    /// Neither build runs (the host is stopping, or the previous build could not restart).
    #[error("update failed and the previous version stopped: {0}")]
    PreviousStopped(String),
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

/// A seat plus its precomputed key (the worker dispatches it every turn) and whether it is registered now.
struct Seated {
    seat: InstanceSeat,
    key: crate::bus::InstanceKey,
    joined: bool,
}

impl Seated {
    fn new(seat: InstanceSeat) -> Self {
        let key = seat.spec.key();
        Self { seat, key, joined: false }
    }

    /// Bus first, then the network broker; a refused mount leaves the bus again.
    fn join(&mut self) -> Result<(), String> {
        if self.joined {
            return Ok(());
        }
        self.seat.bus.register_instance(self.seat.spec.clone()).map_err(|error| error.to_string())?;
        if let Some(net) = &self.seat.net {
            if let Err(error) = net.net.register_instance(&self.seat.spec) {
                self.seat.bus.unregister_instance(&self.key, crate::bus::EndReason::InstanceStopped);
                return Err(error.to_string());
            }
        }
        self.joined = true;
        Ok(())
    }

    /// Queued calls answer `not-available`, subscriptions end with the reason, endpoints unmount.
    fn leave(&mut self, reason: DeactivateReason) {
        if !std::mem::replace(&mut self.joined, false) {
            return;
        }
        self.seat.bus.unregister_instance(&self.key, end_reason(reason));
        if let Some(net) = &self.seat.net {
            net.net.unregister_instance(&self.key);
        }
    }

    /// Priority inside the bus `dispatch`: call results, then commands, then one event batch.
    fn dispatch(&mut self) {
        if !self.joined {
            return;
        }
        let seat = &mut self.seat;
        seat.bus.dispatch(&self.key, seat.bus_hooks.as_mut(), BUS_CALLS_PER_POLL);
        if let Some(net) = seat.net.as_mut() {
            net.net.dispatch(&self.key, net.hooks.as_mut(), NET_CALLS_PER_POLL);
        }
    }

    /// Drop the counters the brokers keep for a stopped installation (uninstall).
    fn forget(&self) {
        let installation_id = &self.seat.spec.installation_id;
        self.seat.bus.forget(installation_id);
        if let Some(net) = &self.seat.net {
            net.net.forget(installation_id);
        }
    }
}

/// One hosted instance: its scheduler, its seat and the hooks that start it again.
struct HostedInstance {
    scheduler: TickScheduler,
    seat: Option<Seated>,
    guest: Option<Box<dyn GuestLifecycle>>,
}

impl HostedInstance {
    fn new(scheduler: TickScheduler, seat: Option<InstanceSeat>, guest: Option<Box<dyn GuestLifecycle>>) -> Self {
        Self { scheduler, seat: seat.map(Seated::new), guest }
    }

    fn join(&mut self) -> Result<(), String> {
        self.seat.as_mut().map_or(Ok(()), Seated::join)
    }

    fn leave(&mut self, reason: DeactivateReason) {
        if let Some(seat) = self.seat.as_mut() {
            seat.leave(reason);
        }
    }

    /// Seat first (so `on-activate` can subscribe), then `on-install` on a first activation, then `on-activate`.
    fn activate(&mut self, first_activation: bool) -> Result<(), String> {
        self.join()?;
        let Some(guest) = self.guest.as_mut() else { return Ok(()) };
        if first_activation {
            guest.on_install().map_err(|reason| format!("on-install refused: {reason}"))?;
        }
        guest.on_activate().map_err(|reason| format!("on-activate refused: {reason}"))
    }

    /// Rollback: the previous build rejoins its seat, runs `on-activate` again, then ticks again.
    fn restart(&mut self) -> Result<(), String> {
        self.activate(false)?;
        self.scheduler.resume();
        Ok(())
    }

    /// A guest that faulted in a bus or network hook (it may never tick) is degraded here.
    fn guest_fault(&self) -> Option<String> {
        if !self.scheduler.is_running() {
            return None;
        }
        self.guest.as_ref().and_then(|guest| guest.degraded())
    }

    /// Fault path, in the OD-E1 stop order: the window is discarded, the seat leaves, the commit in flight settles.
    fn degrade(&mut self, reason: String) {
        if self.scheduler.is_running() {
            self.scheduler.degrade(reason);
        }
        self.leave(DeactivateReason::Degraded);
        self.scheduler.await_in_flight();
    }

    fn dispatch(&mut self) {
        if !self.scheduler.is_running() {
            return;
        }
        if let Some(seat) = self.seat.as_mut() {
            seat.dispatch();
        }
    }
}

#[derive(Default)]
struct TickHostInner {
    entries: HashMap<String, HostedInstance>,
    events: Vec<TickHostEvent>,
}

/// Every hosted instance on the plugin worker; polled from `plugin_thread_loop`, never a Bevy system.
#[derive(Clone, Resource)]
pub struct TickHost {
    inner: Arc<Mutex<TickHostInner>>,
    /// Status handles outside `inner`, so a status read never waits on a running hook (lock order: inner first).
    counters: Arc<Mutex<HashMap<String, Arc<Mutex<TickCounters>>>>>,
    clock: Arc<dyn Clock>,
    contexts: PluginContexts,
}

impl Default for TickHost {
    fn default() -> Self {
        Self::new(Arc::new(WallClock), PluginContexts::default())
    }
}

/// Refuse a second running instance under one id; a stopped or degraded one may be replaced.
fn refuse_running(inner: &TickHostInner, plugin_id: &str) -> Result<(), String> {
    if inner.entries.get(plugin_id).is_some_and(|hosted| hosted.scheduler.is_running()) {
        return Err("plugin instance is already scheduled".into());
    }
    Ok(())
}

impl TickHost {
    /// `clock` is the wall clock in production and the lab's `SimClock` adapter only in fe-sim.
    pub fn new(clock: Arc<dyn Clock>, contexts: PluginContexts) -> Self {
        Self { inner: Arc::default(), counters: Arc::default(), clock, contexts }
    }

    /// The same contexts the host stages, admits and unbinds.
    pub fn with_contexts(mut self, contexts: PluginContexts) -> Self {
        self.contexts = contexts;
        self
    }

    /// The contexts a `Launch` must be staged on.
    pub fn contexts(&self) -> &PluginContexts {
        &self.contexts
    }

    /// The injected clock, for building schedulers that share it.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    fn lock(&self) -> Result<MutexGuard<'_, TickHostInner>, String> {
        self.inner.lock().map_err(|_| "tick host lock poisoned".to_string())
    }

    /// Record the instance and publish its status handle (replacing a stopped or degraded one).
    fn insert(&self, inner: &mut TickHostInner, plugin_id: &str, hosted: HostedInstance) {
        if let Ok(mut counters) = self.counters.lock() {
            counters.insert(plugin_id.to_string(), hosted.scheduler.counters_handle());
        }
        inner.entries.insert(plugin_id.to_string(), hosted);
    }

    fn unbind(&self, plugin_id: &str, why: &str) {
        if let Err(error) = self.contexts.unbind(plugin_id) {
            tracing::warn!(plugin = %plugin_id, %error, "unbinding {why} instance failed");
        }
    }

    /// Start scheduling an instance whose context the caller bound; one running scheduler per plugin id.
    pub fn add(&self, plugin_id: &str, scheduler: TickScheduler) -> Result<(), String> {
        let mut inner = self.lock()?;
        refuse_running(&inner, plugin_id)?;
        self.insert(&mut inner, plugin_id, HostedInstance::new(scheduler, None, None));
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
        self.add_seated(plugin_id, scheduler, InstanceSeat { bus, spec, bus_hooks: hooks, net: None })
    }

    /// [`add`](Self::add) plus a seat on the bus and, when given, the network broker; a refusal schedules nothing.
    pub fn add_seated(&self, plugin_id: &str, scheduler: TickScheduler, seat: InstanceSeat) -> Result<(), String> {
        let mut inner = self.lock()?;
        refuse_running(&inner, plugin_id)?;
        let mut hosted = HostedInstance::new(scheduler, Some(seat), None);
        hosted.join()?;
        self.insert(&mut inner, plugin_id, hosted);
        Ok(())
    }

    /// Start an instance: seat it, run `on-install`/`on-activate` on its staged context, then admit the context
    /// and schedule it. A refusal leaves nothing seated and discards the context. Guest hooks run under the host
    /// lock, so a stop or poll waits for them (src/AGENTS.md §update).
    pub fn launch(&self, plugin_id: &str, launch: Launch) -> Result<(), String> {
        let Launch { scheduler, seat, guest, context, first_activation } = launch;
        let mut inner = match self.lock().and_then(|inner| refuse_running(&inner, plugin_id).map(|()| inner)) {
            Ok(inner) => inner,
            Err(error) => {
                self.contexts.discard(&context);
                return Err(error);
            }
        };
        let mut hosted = HostedInstance::new(scheduler, seat, Some(guest));
        let started = hosted.activate(first_activation).and_then(|()| self.contexts.admit(&context));
        if let Err(reason) = started {
            // Never activated as far as the host is concerned: no `on-deactivate`, nothing staged is flushed.
            hosted.leave(DeactivateReason::Unbound);
            self.contexts.discard(&context);
            return Err(reason);
        }
        self.insert(&mut inner, plugin_id, hosted);
        Ok(())
    }

    /// P4.3 update: the running build leaves its seat and drains (`updating`), keeping its admitted context; the new
    /// build starts on its staged context under the same key. Success revokes the previous lease and admits the new
    /// one; a refusing new build is discarded and the previous one re-activated (src/AGENTS.md §update).
    pub fn update(&self, plugin_id: &str, launch: Launch) -> Result<(), UpdateFailure> {
        let Launch { scheduler, seat, guest, context, first_activation } = launch;
        let Ok(mut inner) = self.inner.lock() else {
            self.contexts.discard(&context);
            return Err(UpdateFailure::NotRunning("tick host lock poisoned".into()));
        };
        let restartable = inner
            .entries
            .get(plugin_id)
            .is_some_and(|old| old.scheduler.is_running() && old.guest.is_some());
        // A stopped or hook-less entry stays where it is; only a restartable build is taken out to update.
        let old = if restartable { inner.entries.remove(plugin_id) } else { None };
        let Some(mut old) = old else {
            self.contexts.discard(&context);
            return Err(UpdateFailure::NotRunning(plugin_id.to_string()));
        };
        // 1. Suspend: leave the shared key, drain, `on-deactivate(updating)`, final flush on the still-live lease.
        old.leave(DeactivateReason::Updating);
        old.scheduler.stop(DeactivateReason::Updating);
        // 2. Start the new build beside the suspended one.
        let mut new = HostedInstance::new(scheduler, seat, Some(guest));
        if let Err(reason) = new.activate(first_activation) {
            new.leave(DeactivateReason::Unbound);
            self.contexts.discard(&context);
            return self.roll_back(&mut inner, plugin_id, old, reason);
        }
        // 3. Retire the previous build for good (only now is its lease revoked), then admit the new one.
        let retired = self.contexts.unbind(plugin_id).and_then(|()| self.contexts.admit(&context));
        if let Err(reason) = retired {
            new.leave(DeactivateReason::HostStopping);
            self.contexts.discard(&context);
            self.unbind(plugin_id, "a previous");
            inner.events.push(TickHostEvent {
                plugin_id: plugin_id.to_string(),
                kind: TickHostEventKind::Stopped(DeactivateReason::Updating),
            });
            inner.entries.insert(plugin_id.to_string(), old);
            return Err(UpdateFailure::PreviousStopped(reason));
        }
        self.insert(&mut inner, plugin_id, new);
        Ok(())
    }

    /// Re-activate the suspended previous build; if even that fails, nothing runs and the registry hears `stopped`.
    fn roll_back(
        &self,
        inner: &mut TickHostInner,
        plugin_id: &str,
        mut old: HostedInstance,
        reason: String,
    ) -> Result<(), UpdateFailure> {
        let outcome = match old.restart() {
            Ok(()) => Err(UpdateFailure::RolledBack(reason)),
            Err(restart) => {
                old.leave(DeactivateReason::Updating);
                self.unbind(plugin_id, "a previous");
                inner.events.push(TickHostEvent {
                    plugin_id: plugin_id.to_string(),
                    kind: TickHostEventKind::Stopped(DeactivateReason::Updating),
                });
                Err(UpdateFailure::PreviousStopped(format!("{reason}; the previous version did not restart: {restart}")))
            }
        };
        inner.entries.insert(plugin_id.to_string(), old);
        outcome
    }

    /// Counters for status reads; kept after the instance stops until `forget` or `uninstall`.
    pub fn counters(&self, plugin_id: &str) -> Option<TickCounters> {
        let handle = self.counters.lock().ok()?.get(plugin_id)?.clone();
        let counters = handle.lock().ok()?.clone();
        Some(counters)
    }

    /// Drop an instance everywhere: a running one stops `unbound` first, then the tick, bus and network counters go.
    pub fn forget(&self, plugin_id: &str) {
        self.retire(plugin_id, false);
    }

    /// Uninstall (G1-C10): stop `unbound`, run `on-uninstall` with one final flush on the live lease, then forget.
    pub fn uninstall(&self, plugin_id: &str) {
        self.retire(plugin_id, true);
    }

    fn retire(&self, plugin_id: &str, uninstall: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            let TickHostInner { entries, events } = &mut *inner;
            if let Some(mut hosted) = entries.remove(plugin_id) {
                if hosted.scheduler.is_running() {
                    hosted.leave(DeactivateReason::Unbound);
                    hosted.scheduler.stop(DeactivateReason::Unbound);
                    if let (true, Some(guest)) = (uninstall, hosted.guest.as_mut()) {
                        guest.on_uninstall();
                        hosted.scheduler.flush_final();
                    }
                    self.unbind(plugin_id, "an uninstalled");
                    events.push(TickHostEvent {
                        plugin_id: plugin_id.to_string(),
                        kind: TickHostEventKind::Stopped(DeactivateReason::Unbound),
                    });
                }
                if let Some(seat) = hosted.seat.as_ref() {
                    seat.forget();
                }
            }
        }
        if let Ok(mut counters) = self.counters.lock() {
            counters.remove(plugin_id);
        }
    }

    /// How long the worker may block on its command channel before a tick is due.
    pub fn next_wait(&self, cap: Duration) -> Duration {
        let now = self.clock.now_ms();
        let due = self
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.entries.values().filter_map(|hosted| hosted.scheduler.next_due_ms()).min());
        match due {
            Some(due) => Duration::from_millis(due.saturating_sub(now).max(1)).min(cap),
            None => cap,
        }
    }

    /// Poll every scheduler once, then run each running instance's bus and network work; a faulted instance
    /// leaves its seat, drains its in-flight commit, is unbound and reported.
    pub fn poll(&self) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let TickHostInner { entries, events } = &mut *inner;
        for (plugin_id, hosted) in entries.iter_mut() {
            let fault = match hosted.scheduler.poll() {
                Some(TickEvent::Degraded(reason)) => Some(reason),
                _ => hosted.guest_fault(),
            };
            if let Some(reason) = fault {
                hosted.degrade(reason.clone());
                self.unbind(plugin_id, "a degraded");
                events.push(TickHostEvent { plugin_id: plugin_id.clone(), kind: TickHostEventKind::Degraded(reason) });
            }
        }
        for hosted in entries.values_mut() {
            hosted.dispatch();
        }
    }

    /// Stop one instance (OD-E1 order): leave the seat first (queued calls answer `not-available`, no new work),
    /// drain the in-flight commit, `on-deactivate`, the final flush unless the reason discards it, then unbind.
    pub fn stop(&self, plugin_id: &str, reason: DeactivateReason) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let TickHostInner { entries, events } = &mut *inner;
        let Some(hosted) = entries.get_mut(plugin_id) else { return };
        hosted.leave(reason);
        hosted.scheduler.stop(reason);
        self.unbind(plugin_id, "a stopped");
        events.push(TickHostEvent { plugin_id: plugin_id.to_string(), kind: TickHostEventKind::Stopped(reason) });
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

    /// Stages one SetTransform per tick and records every `TickInfo` and stop; faults on `fail_on`.
    struct ScriptedInstance {
        window: SharedWindow,
        infos: Arc<Mutex<Vec<TickInfo>>>,
        fuel_refills: Arc<Mutex<Vec<u64>>>,
        deactivations: Arc<Mutex<Vec<DeactivateReason>>>,
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
        fn on_deactivate(&mut self, reason: DeactivateReason) {
            self.deactivations.lock().unwrap().push(reason);
        }
    }

    /// Activation hooks that log, refusing `on-activate` when asked (a broken update).
    struct Guest {
        log: Arc<Mutex<Vec<&'static str>>>,
        refuse: bool,
    }

    impl GuestLifecycle for Guest {
        fn on_install(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("install");
            Ok(())
        }
        fn on_activate(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("activate");
            if self.refuse { Err("guest refused".into()) } else { Ok(()) }
        }
        fn on_uninstall(&mut self) {
            self.log.lock().unwrap().push("uninstall");
        }
        fn degraded(&self) -> Option<String> {
            None
        }
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

    type PendingReplies = Arc<Mutex<Vec<(u64, Sender<FlushReply>, u32)>>>;

    /// Answers each flush `delay_ms` of manual time after it was submitted.
    struct SlowSink {
        clock: ManualClock,
        delay_ms: u64,
        pending: PendingReplies,
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
        deactivations: Arc<Mutex<Vec<DeactivateReason>>>,
        pending: PendingReplies,
        submitted: Arc<Mutex<Vec<StagedFlush>>>,
    }

    fn committed(ops: u32) -> FlushReply {
        FlushReply::Committed(TransactionReceipt { op_count: ops as usize, created_node_ids: vec![], reading_count: 0 })
    }

    fn rig(delay_ms: u64, fail_on: Option<u64>, fuel: u64) -> Rig {
        rig_for(Some(TickBudget { cadence_ms: 500, max_ops: 64, max_readings: 1_000, fuel }), delay_ms, fail_on)
    }

    fn rig_for(budget: Option<TickBudget>, delay_ms: u64, fail_on: Option<u64>) -> Rig {
        let clock = ManualClock::default();
        let window: SharedWindow = Arc::new(Mutex::new(StagedWindow::new(64, 1_000)));
        let infos = Arc::new(Mutex::new(Vec::new()));
        let fuel_refills = Arc::new(Mutex::new(Vec::new()));
        let deactivations = Arc::new(Mutex::new(Vec::new()));
        let pending = Arc::new(Mutex::new(Vec::new()));
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let instance = ScriptedInstance {
            window: window.clone(),
            infos: infos.clone(),
            fuel_refills: fuel_refills.clone(),
            deactivations: deactivations.clone(),
            fail_on,
        };
        let scheduler = TickScheduler::for_budget(
            budget,
            Arc::new(clock.clone()),
            window.clone(),
            Box::new(instance),
            Box::new(SlowSink { clock: clock.clone(), delay_ms, pending: pending.clone(), submitted: submitted.clone() }),
        );
        Rig { clock, scheduler, window, infos, fuel_refills, deactivations, pending, submitted }
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
                    let _ = reply.send(committed(ops));
                }
                self.scheduler.poll();
                assert!(self.pending.lock().unwrap().len() <= 1, "never more than one flush in flight");
            }
        }

        fn counters(&self) -> TickCounters {
            self.scheduler.counters_handle().lock().unwrap().clone()
        }
    }

    /// J1: skip-and-count under a slow sink, fuel exhaustion degrades without a flush, a revoke mid-flush drains
    /// (OD-E1), and the host seats, updates, rolls back and uninstalls instances (G1-C10, P4.3).
    #[test]
    fn tick_journey_skips_counts_and_degrades_without_flushing() {
        let mut slow = rig(750, None, 200_000);
        slow.run_until(10_000);
        let counters = slow.counters();
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
        assert!(matches!(fuel.counters().state, TickState::Degraded(_)));
        assert_eq!(fuel.submitted.lock().unwrap().len(), 1, "only tick 1 flushed");
        assert!(fuel.window.lock().unwrap().is_empty());
        assert_eq!(fuel.scheduler.next_due_ms(), None);

        // OD-E1 (E10W): a revocation mid-flush drains first. The admitted commit settles with its true receipt
        // while the lease is still live; what was staged since is discarded and nothing new is submitted.
        let mut revoked = rig(60_000, None, 200_000);
        revoked.run_until(500);
        assert!(revoked.counters().flush_in_flight, "tick 1's flush is in flight");
        let (_, reply, ops) = revoked.pending.lock().unwrap().pop().unwrap();
        let database = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let _ = reply.send(committed(ops));
        });
        revoked.window.lock().unwrap().stage_op(PendingOp::DeleteNode { node_id: "n1".into() }).unwrap();
        revoked.scheduler.stop(DeactivateReason::Revoked);
        database.join().unwrap();
        let counters = revoked.counters();
        assert_eq!((counters.commits, counters.flush_in_flight, counters.state), (1, false, TickState::Stopped));
        assert_eq!(revoked.submitted.lock().unwrap().len(), 1, "a revoked instance gets no final flush");
        assert!(revoked.window.lock().unwrap().is_empty(), "writes staged after the in-flight commit are discarded");
        assert_eq!(*revoked.deactivations.lock().unwrap(), vec![DeactivateReason::Revoked]);

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
        let idle_spec = crate::bus::InstanceSpec { installation_id: "inst-2".into(), petal_id: "p2".into(), ..spec.clone() };
        let bus = crate::bus::Bus::new(Arc::new(ManualClock::default()), crate::bus::BusSources::fail_closed(), LOCAL);
        let host = TickHost::new(Arc::new(ManualClock::default()), PluginContexts::default());
        host.add_on_bus("inst-1", rig(0, None, 200_000).scheduler, bus.clone(), spec.clone(), Box::new(NoHooks)).unwrap();
        assert!(bus.counters(&key).is_some(), "registered with its scheduler");
        host.stop("inst-1", DeactivateReason::Disabled);
        assert!(bus.counters(&key).is_none(), "a stopped instance leaves the bus");
        assert!(bus.counters_for_installation("inst-1").is_some(), "its counters stay until uninstall");
        // G1-C10: a manifest without a `tick` block is still seated, and sets no tick deadline.
        host.add_on_bus("inst-2", rig_for(None, 0, None).scheduler, bus.clone(), idle_spec.clone(), Box::new(NoHooks)).unwrap();
        assert!(bus.counters(&idle_spec.key()).is_some(), "a no-tick instance has a bus seat");
        assert_eq!(host.next_wait(Duration::from_millis(100)), Duration::from_millis(100));
        // Re-enabling a stopped instance replaces its scheduler; a second running one is refused.
        host.add_on_bus("inst-1", rig(0, None, 200_000).scheduler, bus.clone(), spec.clone(), Box::new(NoHooks)).unwrap();
        assert!(host.add("inst-1", rig(0, None, 200_000).scheduler).is_err());
        // Forgetting stops what still runs and drops the scheduler's and the bus's counters (G1-C10).
        host.forget("inst-1");
        host.forget("inst-2");
        assert!(host.counters("inst-1").is_none() && bus.counters_for_installation("inst-1").is_none());
        assert!(bus.counters(&idle_spec.key()).is_none() && bus.counters_for_installation("inst-2").is_none());

        // G4-A11 (P4.3): an update starts the new build on a staged context under the shared bus key. A refusing
        // build is discarded and the previous one re-activated on its own lease; an accepted one retires it.
        let contexts = PluginContexts::default();
        let host = TickHost::new(Arc::new(ManualClock::default()), contexts.clone());
        let bus = crate::bus::Bus::new(Arc::new(ManualClock::default()), crate::bus::BusSources::fail_closed(), LOCAL);
        let (commands, _queued) = crossbeam::channel::unbounded();
        let token = crate::capability::CapabilityToken::mint(
            "fleet",
            &crate::capability::CapabilityManifest::from_json(r#"{"id":"fleet"}"#).unwrap(),
        );
        let build = |refuse: bool, first_activation: bool| {
            let parts = rig(0, None, 200_000);
            let log = Arc::new(Mutex::new(Vec::new()));
            let context = contexts.stage("fleet".into(), "p1".into(), token.clone(), commands.clone()).unwrap();
            let lease = context.revocation_lease();
            let seat = InstanceSeat { bus: bus.clone(), spec: spec.clone(), bus_hooks: Box::new(NoHooks), net: None };
            let launch = Launch {
                scheduler: parts.scheduler,
                seat: Some(seat),
                guest: Box::new(Guest { log: log.clone(), refuse }),
                context,
                first_activation,
            };
            (launch, log, lease, parts.deactivations)
        };
        let (v1, v1_log, v1_lease, v1_stops) = build(false, true);
        host.launch("fleet", v1).unwrap();
        assert_eq!(*v1_log.lock().unwrap(), ["install", "activate"]);
        assert!(bus.counters(&key).is_some());

        let (v2, _, v2_lease, _) = build(true, false);
        assert!(matches!(host.update("fleet", v2), Err(UpdateFailure::RolledBack(_))));
        assert!(v1_lease.require_active().is_ok() && v2_lease.require_active().is_err(), "only the previous build holds a lease");
        assert_eq!(*v1_stops.lock().unwrap(), vec![DeactivateReason::Updating]);
        assert_eq!(*v1_log.lock().unwrap(), ["install", "activate", "activate"], "the previous build is re-activated");
        assert_eq!(host.counters("fleet").unwrap().state, TickState::Running);
        assert!(bus.counters(&key).is_some(), "and seated again");

        let (v3, v3_log, v3_lease, _) = build(false, false);
        host.update("fleet", v3).unwrap();
        assert!(v1_lease.require_active().is_err() && v3_lease.require_active().is_ok(), "the previous lease goes only on success");
        assert_eq!(*v3_log.lock().unwrap(), ["activate"]);
        assert!(host.take_events().is_empty(), "an update is not a stop: the registry keeps the installation active");

        // Uninstall: `on-deactivate(unbound)`, `on-uninstall`, the lease revoked, every counter forgotten.
        host.uninstall("fleet");
        assert_eq!(*v3_log.lock().unwrap(), ["activate", "uninstall"]);
        assert!(v3_lease.require_active().is_err());
        assert!(host.counters("fleet").is_none() && bus.counters_for_installation("inst-1").is_none());
        assert_eq!(host.take_events(), vec![TickHostEvent { plugin_id: "fleet".into(), kind: TickHostEventKind::Stopped(DeactivateReason::Unbound) }]);
    }
}
