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

/// Current wall-clock time in milliseconds since the Unix epoch.
fn wall_now_ms() -> u64 {
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
}
