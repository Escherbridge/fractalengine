//! `SimClock` — the virtual, accelerable, deterministic clock every
//! simulated peer reads (D3): sensor timestamps, the network's delivery
//! schedule, and HLC stamping (through `fe_database::op_log::
//! set_wall_clock_source`) all derive from it, never from wall time.
//!
//! Two drive modes:
//! * **Stepped** (scenarios / CI): the runner calls [`SimClock::advance_ms`]
//!   per tick. Fully deterministic — nothing in a run observes real time
//!   except how long convergence takes, never what converges.
//! * **Auto-accelerated** (demos): [`SimClock::auto_pump`] spawns a driver
//!   thread advancing sim time at `speed`× real time (e.g. 60× = one
//!   simulated minute per real second).

use std::sync::Mutex;
use std::time::Duration;

/// Internal clock state — `now_ms` is simulated epoch milliseconds.
#[derive(Debug)]
struct ClockState {
    now_ms: u64,
    /// Simulated ms advanced per real ms (1.0 = real time). Manual stepping
    /// ignores the speed factor — acceleration only shapes the auto pump.
    speed: f64,
}

/// The process-global "current" clock the HLC source shim reads (see
/// [`install_hlc_source`]). One active scenario at a time — the scenario
/// runner serializes runs process-wide ([`SCENARIO_RUN_LOCK`]), because both
/// the HLC override and the wall-clock override are process-global state.
static CURRENT: Mutex<Option<std::sync::Arc<SimClock>>> = Mutex::new(None);

/// Serializes every HLC-source install window process-wide (scenario runs
/// and the clock tests): the override is process-global state, so two
/// concurrent installs would clobber each other — a run would stamp from
/// another run's clock, or from real time after a foreign uninstall. Cargo
/// test runs library tests on parallel threads, so this lock is what keeps
/// concurrent scenario tests from racing the global.
pub static SCENARIO_RUN_LOCK: Mutex<()> = Mutex::new(());

/// A virtual, accelerable, deterministic clock (F8/A19).
#[derive(Debug)]
pub struct SimClock {
    state: Mutex<ClockState>,
}

impl SimClock {
    /// New clock at simulated epoch `start_ms`.
    pub fn new(start_ms: u64) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: Mutex::new(ClockState {
                now_ms: start_ms,
                speed: 1.0,
            }),
        })
    }

    /// Current simulated epoch milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).now_ms
    }

    /// Advance simulated time by `delta_ms` and return the new now.
    /// Monotonic by construction (addition on u64).
    pub fn advance_ms(&self, delta_ms: u64) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.now_ms = state.now_ms.saturating_add(delta_ms);
        state.now_ms
    }

    /// Set the acceleration factor (simulated ms per real ms) used by the
    /// auto pump. 1.0 = real time.
    pub fn set_speed(&self, speed: f64) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).speed = speed.max(0.0);
    }

    /// The current acceleration factor.
    pub fn speed(&self) -> f64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).speed
    }

    /// Pure arithmetic of the auto pump: how much simulated time `real_elapsed`
    /// covers at `speed` (unit-tested separately from the thread timing).
    pub fn sim_delta(real_elapsed: Duration, speed: f64) -> u64 {
        (real_elapsed.as_millis() as f64 * speed.max(0.0)) as u64
    }

    /// Demo drive mode: spawn a thread advancing sim time at `speed`× real
    /// time until the returned guard is dropped. Scenario/CI runs do NOT
    /// use this — they step the clock deterministically (`advance_ms`) and
    /// keep `speed` as a recorded parameter.
    pub fn auto_pump(self: &std::sync::Arc<Self>) -> AutoPumpGuard {
        let clock = self.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut last = std::time::Instant::now();
            while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                let now = std::time::Instant::now();
                let delta = SimClock::sim_delta(now - last, clock.speed());
                if delta > 0 {
                    clock.advance_ms(delta);
                }
                last = now;
            }
        });
        AutoPumpGuard {
            stop,
            handle: Some(handle),
        }
    }
}

/// Stops the auto pump when dropped (joins the driver thread).
pub struct AutoPumpGuard {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AutoPumpGuard {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The HLC source shim: a plain `fn` (the override takes a fn pointer, not a
/// closure) reading the process-global current clock.
fn current_clock_now_ms() -> u64 {
    CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|c| c.now_ms())
        .unwrap_or(0)
}

/// Install `clock` as the process's HLC wall-clock source (see
/// `fe_database::op_log::set_wall_clock_source`): from here, every HLC
/// stamp in the process reads the SimClock. Paired with
/// [`uninstall_hlc_source`] by the scenario runner around each run.
pub fn install_hlc_source(clock: std::sync::Arc<SimClock>) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(clock);
    fe_database::op_log::set_wall_clock_source(Some(current_clock_now_ms));
}

/// Restore real wall-clock HLC stamping (shutdown / between runs).
pub fn uninstall_hlc_source() {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    fe_database::op_log::set_wall_clock_source(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_start_ms_and_advances_monotonically() {
        let clock = SimClock::new(1_000_000);
        assert_eq!(clock.now_ms(), 1_000_000);
        assert_eq!(clock.advance_ms(60_000), 1_060_000);
        assert_eq!(clock.advance_ms(1), 1_060_001);
        assert!(clock.now_ms() >= 1_060_001);
    }

    #[test]
    fn speed_factor_shapes_only_the_auto_pump_arithmetic() {
        let clock = SimClock::new(0);
        clock.set_speed(60.0);
        assert_eq!(clock.speed(), 60.0);
        // 100 real ms at 60x = 6000 simulated ms.
        assert_eq!(SimClock::sim_delta(Duration::from_millis(100), 60.0), 6_000);
        // Manual stepping is unaffected by the speed factor (determinism).
        assert_eq!(clock.advance_ms(10), 10);
        clock.set_speed(-5.0);
        assert_eq!(clock.speed(), 0.0, "negative speed clamps to zero");
    }

    #[test]
    fn hlc_source_shim_reads_the_current_clock() {
        // The override is process-global — serialize against concurrent
        // scenario runs (see SCENARIO_RUN_LOCK).
        let _run_lock = SCENARIO_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let clock = SimClock::new(4_000_000_000_000);
        install_hlc_source(clock.clone());
        // init_hlc resets HLC state; with the override active the wall bits
        // must come from the SimClock, not the system clock.
        fe_database::op_log::init_hlc(0);
        let (packed, _) = fe_database::op_log::next_hlc_timestamp();
        uninstall_hlc_source();
        assert_eq!(
            packed >> 16,
            4_000_000_000_000,
            "HLC wall bits must read the SimClock"
        );
    }
}
