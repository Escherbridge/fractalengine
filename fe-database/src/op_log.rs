//! Hybrid Logical Clock (HLC) + operation-log writer (design:
//! fe-database/src/AGENTS.md §hlc).

use std::future::Future;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;

use crate::repo::Db;
use crate::repo::Repo;
use crate::schema::OpLog;
use crate::types::{OpLogEntry, PetalId};

// ---------------------------------------------------------------------------
// HLC state
// ---------------------------------------------------------------------------

/// Internal HLC state: wall-clock milliseconds + sub-ms counter.
struct HlcState {
    wall_ms: u64,
    counter: u64,
}

/// Module-level HLC.  `None` until [`init_hlc`] is called.
static HLC_STATE: Mutex<Option<HlcState>> = Mutex::new(None);

/// Pluggable wall-clock source (F8 sim lab): when installed, HLC wall time
/// comes from the injected source instead of the system clock, so a
/// simulated fleet stamps HLC from its virtual, accelerable `SimClock`
/// (every simulated peer reads time from it — see fe-sim
/// `src/AGENTS.md` §hlc-sim). A plain `fn` pointer (not a closure) keeps
/// the override `Sync`; the sim installs a shim reading its process-global
/// current clock. Uninstalled by default — production processes never touch
/// this and keep real wall time.
static WALL_CLOCK_SOURCE: Mutex<Option<fn() -> u64>> = Mutex::new(None);

/// Install a wall-clock source for HLC stamping (sim lab only). Passing
/// `None` restores the system clock.
pub fn set_wall_clock_source(source: Option<fn() -> u64>) {
    *WALL_CLOCK_SOURCE.lock().unwrap() = source;
}

/// Initialise the HLC past the highest persisted `lamport_clock` — called
/// once at DB startup, before any op-log write (see fe-database/src/AGENTS.md §hlc).
pub fn init_hlc(max_persisted: u64) {
    let now_ms = wall_now_ms();

    let (wall_ms, counter) = if max_persisted > 0 {
        // Decode the persisted packed value.
        let persisted_wall = max_persisted >> 16;
        let persisted_counter = max_persisted & 0xFFFF;
        if now_ms > persisted_wall {
            (now_ms, 0)
        } else {
            // Wall clock hasn't advanced past the persisted value — continue
            // from the persisted position to stay monotonic.
            (persisted_wall, persisted_counter + 1)
        }
    } else {
        (now_ms, 0)
    };

    *HLC_STATE.lock().unwrap() = Some(HlcState { wall_ms, counter });
    tracing::info!(
        "HLC initialised: wall_ms={wall_ms}, counter={counter}, max_persisted={max_persisted}"
    );
}

/// Opaque copy of the process HLC state (packed `wall<<16 | counter`; `None`
/// = uninitialised) — sim lab only, see [`snapshot_hlc`] / [`restore_hlc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlcSnapshot(Option<u64>);

impl HlcSnapshot {
    /// The packed state this snapshot holds (`None` = HLC was uninitialised).
    pub fn packed(&self) -> Option<u64> {
        self.0
    }
}

/// Snapshot the process HLC state before a sim session overrides it
/// (fe-database/src/AGENTS.md §hlc "sim-session snapshot/restore").
pub fn snapshot_hlc() -> HlcSnapshot {
    let guard = HLC_STATE.lock().unwrap_or_else(|e| e.into_inner());
    HlcSnapshot(
        guard
            .as_ref()
            .map(|s| (s.wall_ms << 16) | (s.counter & 0xFFFF)),
    )
}

/// Restore the HLC to `max(snapshot, real-now state)` after a sim session —
/// reads the SYSTEM clock (never an installed source); see AGENTS.md §hlc.
pub fn restore_hlc(snapshot: HlcSnapshot) {
    let real = (system_now_ms(), 0u64);
    let (wall_ms, counter) = match snapshot.0 {
        Some(packed) => real.max((packed >> 16, packed & 0xFFFF)),
        None => real,
    };
    *HLC_STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(HlcState { wall_ms, counter });
    tracing::info!("HLC restored after sim session: wall_ms={wall_ms}, counter={counter}");
}

/// Next HLC timestamp as `(packed_u64, human_string)`; panics if
/// [`init_hlc`] has not been called (format: fe-database/src/AGENTS.md §hlc).
pub fn next_hlc_timestamp() -> (u64, String) {
    let mut guard = HLC_STATE.lock().unwrap();
    let state = guard
        .as_mut()
        .expect("HLC not initialised — call init_hlc() during DB startup");

    let now_ms = wall_now_ms();

    if now_ms > state.wall_ms {
        state.wall_ms = now_ms;
        state.counter = 0;
    } else {
        state.counter += 1;
        // Guard against pathological bursts that would overflow 16 bits.
        // In practice 65 536 ops per millisecond is unreachable for a
        // single-threaded DB, but saturate rather than wrap.
        if state.counter > 0xFFFF {
            state.wall_ms += 1;
            state.counter = 0;
        }
    }

    let packed: u64 = (state.wall_ms << 16) | (state.counter & 0xFFFF);
    let ts_string = format!("{}:{:04X}", state.wall_ms, state.counter);

    (packed, ts_string)
}

// ---------------------------------------------------------------------------
// Op-log persistence
// ---------------------------------------------------------------------------

/// Append an operation before materializing its local state.
///
/// Until the verified materializer lands, a materialization failure can leave
/// an appended operation pending replay; an append failure never invokes the
/// materializer. See `src/AGENTS.md` §log-first-commit.
pub async fn commit_operation<T, F, Fut>(
    db: &Db,
    mut entry: OpLogEntry,
    materialize: F,
) -> anyhow::Result<T>
where
    F: FnOnce(u64) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let (lamport, hlc_ts) = next_hlc_timestamp();
    entry.lamport_clock = lamport;
    entry.hlc_timestamp = hlc_ts;

    append_operation_log(db, entry)
        .await
        .context("operation log append failed; local materialization was not attempted")?;

    materialize(lamport).await.map_err(|error| {
        // Preserve the full cause for DbCommand's outer error rendering; see AGENTS.md §log-first-commit.
        let cause = format!("{error:#}");
        error.context(format!(
            "operation was appended but local materialization failed; recovery must replay the operation; cause: {cause}"
        ))
    })
}

/// Persist a pre-stamped operation entry. Only [`commit_operation`] may call this.
async fn append_operation_log(db: &Db, entry: OpLogEntry) -> anyhow::Result<()> {
    let val = serde_json::to_value(entry)?;
    Repo::<OpLog>::create_raw(db, val).await?;
    Ok(())
}

pub async fn query_petal_at_time(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    petal_id: &PetalId,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<serde_json::Value> {
    let petal_record = format!("petal:{}", petal_id.0);
    let ts = timestamp.to_rfc3339();
    let mut result: surrealdb::IndexedResults = db
        .query(format!("SELECT * FROM $record VERSION d'{ts}'"))
        .bind(("record", petal_record))
        .await?;
    let value: Option<serde_json::Value> = result.take(0)?;
    Ok(value.unwrap_or(serde_json::Value::Null))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current wall-clock time in milliseconds since the Unix epoch — from the
/// installed sim-lab source when one is present, else the system clock.
fn wall_now_ms() -> u64 {
    if let Some(source) = *WALL_CLOCK_SOURCE.lock().unwrap() {
        return source();
    }
    system_now_ms()
}

/// The system clock in epoch milliseconds — never the installed source.
fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// All HLC tests share global state, so we must serialise them.
    /// Each test holds this lock for its entire duration.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_and_reset() -> MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *HLC_STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        guard
    }

    #[test]
    fn init_from_zero_uses_wall_clock() {
        let _g = lock_and_reset();
        init_hlc(0);
        let (packed, ts) = next_hlc_timestamp();
        assert!(packed > 0, "packed timestamp should be non-zero");
        assert!(ts.contains(':'), "human string should contain ':'");
    }

    #[test]
    fn monotonically_increasing() {
        let _g = lock_and_reset();
        init_hlc(0);
        let (a, _) = next_hlc_timestamp();
        let (b, _) = next_hlc_timestamp();
        let (c, _) = next_hlc_timestamp();
        assert!(b > a, "b={b} should be > a={a}");
        assert!(c > b, "c={c} should be > b={b}");
    }

    #[test]
    fn survives_restart_past_persisted() {
        let _g = lock_and_reset();
        // Simulate a persisted value far in the future (wall_ms=999999999, counter=5)
        let fake_persisted: u64 = (999_999_999u64 << 16) | 5;
        init_hlc(fake_persisted);
        let (packed, _) = next_hlc_timestamp();
        assert!(
            packed > fake_persisted,
            "first timestamp after restart ({packed}) must exceed persisted ({fake_persisted})"
        );
    }

    #[test]
    fn packed_encoding_roundtrip() {
        let _g = lock_and_reset();
        init_hlc(0);
        let (packed, ts) = next_hlc_timestamp();
        let wall = packed >> 16;
        let counter = packed & 0xFFFF;
        let expected = format!("{wall}:{counter:04X}");
        assert_eq!(ts, expected);
    }

    #[test]
    fn counter_overflow_advances_wall() {
        let _g = lock_and_reset();
        // Set wall_ms far in the future so wall clock never catches up.
        let future_wall: u64 = wall_now_ms() + 1_000_000;
        let near_overflow: u64 = (future_wall << 16) | 0xFFFD;
        init_hlc(near_overflow);

        // First tick: counter = 0xFFFF
        let (a, _) = next_hlc_timestamp();
        assert_eq!(a & 0xFFFF, 0xFFFF);

        // Second tick: counter would be 0x10000 — should advance wall_ms instead.
        let (b, _) = next_hlc_timestamp();
        assert_eq!(b & 0xFFFF, 0, "counter should reset to 0 after overflow");
        assert!(b > a, "b should still be monotonically greater");
    }

    /// A fixed fake clock source for the override tests (a real test would
    /// race the wall clock; a fake pins the values under assertion).
    fn fake_wall_now() -> u64 {
        4_102_444_800_000 // 2100-01-01T00:00:00Z, deliberately far from now
    }

    #[test]
    fn wall_clock_source_override_stamps_hlc_from_the_source() {
        let _g = lock_and_reset();
        set_wall_clock_source(Some(fake_wall_now));
        init_hlc(0);
        let (packed, _) = next_hlc_timestamp();
        let (second, _) = next_hlc_timestamp();
        set_wall_clock_source(None);
        // Restore real-clock state for the other tests in this binary.
        *HLC_STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        init_hlc(0);

        assert_eq!(
            packed >> 16,
            fake_wall_now(),
            "wall bits must come from the override"
        );
        assert!(second > packed, "monotonicity holds under a frozen source");
        let (real, _) = next_hlc_timestamp();
        assert!(
            real >> 16 < fake_wall_now(),
            "clearing restores real wall time"
        );
    }

    /// A past-dated sim source for the restore tests (2020-01-01T00:00:00Z).
    fn past_sim_now() -> u64 {
        1_577_836_800_000
    }

    /// A past-time sim session resets the HLC backwards (`init_hlc(0)` under
    /// the source); `restore_hlc` must bring it back to at least the
    /// pre-session state, so the next production stamp exceeds every stamp
    /// issued before the session — even when that state ran AHEAD of the
    /// wall clock (persisted stamps after clock skew).
    #[test]
    fn restore_after_a_past_sim_session_keeps_production_stamps_monotonic() {
        let _g = lock_and_reset();
        let ahead_wall = system_now_ms() + 60_000;
        init_hlc((ahead_wall << 16) | 7);
        let (before, _) = next_hlc_timestamp();
        let snapshot = snapshot_hlc();
        assert_eq!(snapshot.packed(), Some(before));

        // The sim session: past source + per-peer init_hlc(0).
        set_wall_clock_source(Some(past_sim_now));
        init_hlc(0);
        let (sim_stamp, _) = next_hlc_timestamp();
        set_wall_clock_source(None);
        assert_eq!(
            sim_stamp >> 16,
            past_sim_now(),
            "the session stamped sim time"
        );

        restore_hlc(snapshot);
        let (after, _) = next_hlc_timestamp();
        assert!(
            after > before,
            "post-session stamp {after} must exceed the pre-session stamp {before}"
        );
    }

    /// Restoring an uninitialised snapshot initialises at real now (never
    /// leaves the sim's state behind, never panics a later stamp), and a
    /// snapshot behind real time restores to real time.
    #[test]
    fn restore_from_uninitialised_or_stale_snapshot_uses_real_now() {
        let _g = lock_and_reset();
        let snapshot = snapshot_hlc();
        assert_eq!(snapshot.packed(), None);
        set_wall_clock_source(Some(fake_wall_now));
        init_hlc(0);
        set_wall_clock_source(None);
        let floor = system_now_ms();
        restore_hlc(snapshot);
        let (stamp, _) = next_hlc_timestamp();
        assert!(stamp >> 16 >= floor, "restored to real now");
        assert!(
            stamp >> 16 < fake_wall_now(),
            "the sim's future state is discarded"
        );
    }
}
