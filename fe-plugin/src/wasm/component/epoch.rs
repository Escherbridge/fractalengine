//! One shared epoch ticker for every component engine; hook calls are interrupted at their deadline
//! (src/wasm/AGENTS.md §hooks, "wall-clock bound").

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wasmtime::{Engine, EngineWeak};

/// Wall time between epoch increments; an interrupt lands at most two ticks after its deadline.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);
/// Epoch delta for guest entries that keep only their fuel budget (ticks, routes, bus, lifecycle).
pub const NO_EPOCH_DEADLINE: u64 = u64::MAX / 2;

static ENGINES: OnceLock<Mutex<Vec<EngineWeak>>> = OnceLock::new();

/// Tick `engine`'s epoch from the one process-wide ticker thread (started on first use); dropped engines leave it.
pub(crate) fn register(engine: &Engine) {
    let engines = ENGINES.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("wasm-epoch".into()).spawn(tick_engines);
        if let Err(error) = spawned {
            tracing::error!(%error, "wasm epoch ticker did not start; hook calls keep only their fuel bound");
        }
        Mutex::new(Vec::new())
    });
    engines
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(engine.weak());
}

/// The ticker loop: bump every live engine's epoch each [`EPOCH_TICK`], forgetting dropped engines.
fn tick_engines() {
    loop {
        std::thread::sleep(EPOCH_TICK);
        if let Some(engines) = ENGINES.get() {
            let mut engines = engines.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            engines.retain(|weak| match weak.upgrade() {
                Some(engine) => {
                    engine.increment_epoch();
                    true
                }
                None => false,
            });
        }
    }
}

/// Epoch ticks that cover `deadline`, rounded up (the interrupt never lands early).
pub fn ticks_for(deadline: Duration) -> u64 {
    let tick = EPOCH_TICK.as_nanos().max(1);
    u64::try_from(deadline.as_nanos().div_ceil(tick)).unwrap_or(NO_EPOCH_DEADLINE).saturating_add(1)
}
