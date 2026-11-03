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

/// An admitted remote scene clock, bounded before it can affect local causality.
pub(crate) struct AdmittedSceneHlc(u64);

pub(crate) fn admit_scene_hlc(packed: u64) -> anyhow::Result<AdmittedSceneHlc> {
    let wall = packed >> 16;
    anyhow::ensure!(
        packed > 0
            && packed <= i64::MAX as u64 - 0x1_0000
            && wall <= wall_now_ms().saturating_add(300_000),
        "scene HLC outside the supported domain or five-minute future bound"
    );
    Ok(AdmittedSceneHlc(packed))
}

/// Observe only after the corresponding scene row and clock commit atomically.
pub(crate) fn observe_scene_hlc(clock: AdmittedSceneHlc) {
    let mut guard = HLC_STATE.lock().unwrap_or_else(|error| error.into_inner());
    let incoming = (clock.0 >> 16, clock.0 & 0xFFFF);
    let current = guard
        .as_ref()
        .map(|state| (state.wall_ms, state.counter))
        .unwrap_or((wall_now_ms(), 0));
    let (wall_ms, counter) = current.max(incoming);
    *guard = Some(HlcState { wall_ms, counter });
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

/// Binding-name prefix reserved for [`commit_operation_atomic`]'s own binds.
pub const RESERVED_OP_BINDING_PREFIX: &str = "__op_";

/// Log-first commit where the op-log row and `statements` share ONE SurrealDB
/// transaction: a failing statement rolls back the log row too. Returns the
/// stamped lamport clock and each executed caller statement's rows (the
/// op-log CREATE and BEGIN/COMMIT slots are offset out). See `src/AGENTS.md`
/// §log-first-commit.
pub async fn commit_operation_atomic(
    db: &Db,
    entry: OpLogEntry,
    statements: &[String],
    bindings: Vec<(String, serde_json::Value)>,
) -> anyhow::Result<(u64, Vec<Vec<serde_json::Value>>)> {
    let statements = statements.to_vec();
    commit_operation_atomic_with(db, entry, move |_| (statements, bindings)).await
}

/// [`commit_operation_atomic`] whose statements and bindings are built from the
/// stamped lamport clock (e.g. a tombstone marker carrying the op's HLC).
pub async fn commit_operation_atomic_with<F>(
    db: &Db,
    mut entry: OpLogEntry,
    build: F,
) -> anyhow::Result<(u64, Vec<Vec<serde_json::Value>>)>
where
    F: FnOnce(u64) -> (Vec<String>, Vec<(String, serde_json::Value)>),
{
    let (lamport, hlc_ts) = next_hlc_timestamp();
    let (statements, bindings) = build(lamport);
    if let Some((name, _)) = bindings
        .iter()
        .find(|(name, _)| name.starts_with(RESERVED_OP_BINDING_PREFIX))
    {
        anyhow::bail!("binding '{name}' uses the reserved '{RESERVED_OP_BINDING_PREFIX}' prefix");
    }
    entry.lamport_clock = lamport;
    entry.hlc_timestamp = hlc_ts;

    let mut script = String::from("BEGIN TRANSACTION;\nCREATE op_log CONTENT $__op_entry;\n");
    for (index, statement) in statements.iter().enumerate() {
        let trimmed = statement.trim().trim_end_matches(';').trim();
        if trimmed.is_empty() {
            anyhow::bail!("statement {index} is empty");
        }
        script.push_str(trimmed);
        script.push_str(";\n");
    }
    script.push_str("COMMIT TRANSACTION;");

    let mut query = db.query(script).bind(("__op_entry", op_entry_json(entry)?));
    for binding in bindings {
        query = query.bind(binding);
    }
    let mut response = query.await.context("atomic operation commit failed")?;
    let errors = response.take_errors();
    if !errors.is_empty() {
        anyhow::bail!("{}", most_specific_error(errors));
    }
    // Slots: [BEGIN, CREATE op_log, caller…, COMMIT]. A caller `RETURN`
    // skips the rest of the transaction's statements, so there may be fewer
    // caller slots than statements (src/AGENTS.md §log-first-commit).
    let num = response.num_statements();
    let mut rows = Vec::with_capacity(statements.len());
    for index in 2..num.saturating_sub(1) {
        rows.push(
            response
                .take::<Vec<serde_json::Value>>(index)
                .with_context(|| format!("reading statement {} result", index - 2))?,
        );
    }
    Ok((lamport, rows))
}

/// The error that explains a failed transaction: the first one that is not
/// SurrealDB's generic "not executed due to a failed transaction" echo.
pub(crate) fn most_specific_error(
    errors: std::collections::HashMap<usize, surrealdb::Error>,
) -> String {
    let mut errors: Vec<(usize, String)> = errors
        .into_iter()
        .map(|(index, error)| (index, error.to_string()))
        .collect();
    errors.sort_by_key(|(index, _)| *index);
    errors
        .iter()
        .find(|(_, text)| !text.contains("due to a failed transaction"))
        .or_else(|| errors.first())
        .map(|(_, text)| text.clone())
        .unwrap_or_default()
}

/// Persist a pre-stamped operation entry. Only [`commit_operation`] may call this.
async fn append_operation_log(db: &Db, entry: OpLogEntry) -> anyhow::Result<()> {
    Repo::<OpLog>::create_raw(db, op_entry_json(entry)?).await
}

/// The stored JSON shape of an op-log entry, shared by both commit seams.
fn op_entry_json(entry: OpLogEntry) -> anyhow::Result<serde_json::Value> {
    Ok(crate::repo::strip_json_nulls(serde_json::to_value(entry)?))
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

    async fn op_log_count(db: &Db) -> usize {
        let rows: Vec<serde_json::Value> = db
            .query("SELECT count() AS n FROM op_log GROUP ALL")
            .await
            .expect("count op_log")
            .take(0)
            .expect("count rows");
        rows.first().and_then(|row| row["n"].as_u64()).unwrap_or(0) as usize
    }

    async fn probe_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("op_log").use_db("op_log").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("apply schema");
        db.query("DEFINE TABLE IF NOT EXISTS atomicity_probe SCHEMALESS")
            .await
            .expect("probe table")
            .check()
            .expect("probe table ok");
        db
    }

    fn probe_entry() -> OpLogEntry {
        OpLogEntry {
            lamport_clock: 0,
            hlc_timestamp: String::new(),
            node_id: crate::types::NodeId("did:key:z6MkProbe".to_string()),
            op_type: crate::types::OpType::ElevatedQuery,
            payload: serde_json::json!({ "probe": true }),
            sig: "00".repeat(64),
        }
    }

    // The HLC lock is held across awaits on purpose: other tests reset the
    // process-global HLC to `None`, and the commit stamps it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn atomic_commit_logs_once_and_returns_each_statement_rows() {
        let _g = lock_and_reset();
        init_hlc(0);
        let db = probe_db().await;
        let (lamport, rows) = commit_operation_atomic(
            &db,
            probe_entry(),
            &[
                "CREATE atomicity_probe CONTENT { tag: $tag }".to_string(),
                "SELECT VALUE tag FROM atomicity_probe".to_string(),
            ],
            vec![("tag".to_string(), serde_json::json!("kept"))],
        )
        .await
        .expect("atomic commit");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec![serde_json::json!("kept")]);

        assert_eq!(op_log_count(&db).await, 1);
        let logged: Vec<serde_json::Value> = db
            .query("SELECT lamport_clock, op_type FROM op_log")
            .await
            .expect("read op_log")
            .take(0)
            .expect("op_log rows");
        assert_eq!(logged[0]["lamport_clock"].as_u64(), Some(lamport));
        assert_eq!(logged[0]["op_type"], "ElevatedQuery");
        // A `LET` slot (no rows) inside the transaction does not break slot reading.
        let (_, rows) = commit_operation_atomic(
            &db,
            probe_entry(),
            &[
                "LET $t = 'let-bound'".to_string(),
                "CREATE atomicity_probe CONTENT { tag: $t }".to_string(),
                "SELECT VALUE tag FROM atomicity_probe WHERE tag = 'let-bound'".to_string(),
            ],
            Vec::new(),
        )
        .await
        .expect("atomic commit with LET");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2], vec![serde_json::json!("let-bound")]);
        assert_eq!(op_log_count(&db).await, 2, "one entry per commit");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_failing_statement_rolls_back_the_op_log_row_too() {
        let _g = lock_and_reset();
        init_hlc(0);
        let db = probe_db().await;
        let error = commit_operation_atomic(
            &db,
            probe_entry(),
            &[
                "CREATE atomicity_probe CONTENT { tag: 'doomed' }".to_string(),
                "THROW 'induced failure'".to_string(),
            ],
            Vec::new(),
        )
        .await
        .expect_err("a throwing statement must fail the whole commit");
        assert!(error.to_string().contains("induced failure"), "{error}");
        assert_eq!(op_log_count(&db).await, 0, "the log row rolled back");
        let probes: Vec<serde_json::Value> = db
            .query("SELECT * FROM atomicity_probe")
            .await
            .expect("read probes")
            .take(0)
            .expect("probe rows");
        assert!(probes.is_empty(), "the write rolled back: {probes:?}");

        let refused = commit_operation_atomic(
            &db,
            probe_entry(),
            &["RETURN 1".to_string()],
            vec![("__op_entry".to_string(), serde_json::json!({}))],
        )
        .await
        .expect_err("a caller binding may not shadow the log entry");
        assert!(refused.to_string().contains("reserved"), "{refused}");
        assert_eq!(op_log_count(&db).await, 0);

        crate::schema::apply_all(&db).await.expect("journal schema");
        db.query("CREATE verse CONTENT {verse_id:'history-verse',name:'v',created_by:'did:qa',created_at:'2026-11-01'};
            CREATE fractal CONTENT {fractal_id:'history-fractal',verse_id:'history-verse',owner_did:'did:qa',name:'f',created_at:'2026-11-01'};
            CREATE petal CONTENT {petal_id:'history-petal',fractal_id:'history-fractal',name:'p',node_id:'p',created_at:'2026-11-01'};")
            .await.unwrap().check().unwrap();
        let (node_id, _) = crate::handlers::node_history::create_object(
            &db,
            "history-petal",
            "A",
            None,
            [0.0; 3],
            vec![0.0; 3],
            vec![1.0; 3],
            serde_json::Map::new(),
            "did:qa",
        )
        .await
        .unwrap();
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            &node_id,
            "calibration",
            &serde_json::json!({"gain":2}),
            "did:qa",
        )
        .await
        .unwrap();
        crate::handlers::transform::update_node_transform_as(
            &db,
            &node_id,
            [3.0, 0.0, 4.0],
            [0.0; 3],
            [1.0; 3],
            "did:qa",
        )
        .await
        .unwrap();
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            &node_id,
            "calibration",
            &serde_json::json!({"gain":3}),
            "did:qa",
        )
        .await
        .unwrap();
        crate::handlers::transform::update_node_transform_as(
            &db,
            &node_id,
            [6.0, 0.0, 8.0],
            [0.0; 3],
            [1.0; 3],
            "did:qa",
        )
        .await
        .unwrap();
        let before = crate::handlers::node_history::committed_receipt(&db, &node_id)
            .await
            .unwrap();
        assert_eq!(before["history_version"], 5);
        let before_count = op_log_count(&db).await;
        db.query("DEFINE EVENT fail_journal ON TABLE node_log WHEN $event = 'CREATE' AND $after.row_version = 6 THEN { THROW 'induced journal failure'; }")
            .await.unwrap().check().unwrap();
        let error = crate::handlers::entity_property::set_entity_property_as(
            &db,
            &node_id,
            "calibration",
            &serde_json::json!({"gain":99}),
            "did:qa",
        )
        .await
        .expect_err("journal insert must roll back mutation");
        assert!(
            error.to_string().contains("induced journal failure"),
            "{error}"
        );
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &node_id)
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            op_log_count(&db).await,
            before_count,
            "journal failure rolls back intention too"
        );
        let page = crate::handlers::node_history::read(&db, &node_id, 0, 200, None, "did:qa")
            .await
            .unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), 5);
        assert_eq!(page["current_version"], 5);
        let replay = crate::handlers::node_history::read(&db, &node_id, 0, 1, Some(3), "did:qa")
            .await
            .unwrap();
        assert_eq!(replay["availability"], "available");
        assert_eq!(replay["state"]["properties"]["calibration"]["gain"], 2);
        assert_eq!(
            replay["state"]["position"],
            serde_json::json!([3.0, 0.0, 4.0])
        );
        db.query("REMOVE EVENT fail_journal ON TABLE node_log")
            .await
            .unwrap()
            .check()
            .unwrap();
        crate::handlers::node_log::append_node_log(
            &db,
            &node_id,
            "unsupported_legacy_write",
            "did:qa",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
        let gap = crate::handlers::node_history::read(&db, &node_id, 0, 1, Some(6), "did:qa")
            .await
            .unwrap();
        assert_eq!(gap["availability"], "unavailable_scope");
        assert!(gap.get("state").is_none());
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            &node_id,
            "calibration",
            &serde_json::json!({"gain":4}),
            "did:qa",
        )
        .await
        .unwrap();
        let restarted_epoch =
            crate::handlers::node_history::read(&db, &node_id, 0, 1, Some(7), "did:qa")
                .await
                .unwrap();
        assert_eq!(restarted_epoch["availability"], "available");
        assert_eq!(restarted_epoch["baseline_version"], 7);
        let legacy_id = "legacy-history-node";
        db.query("CREATE node CONTENT {node_id:$id,petal_id:'history-petal',display_name:'legacy',position:<geometry<point>>[0.0,0.0],elevation:0.0,rotation:[0.0,0.0,0.0],scale:[1.0,1.0,1.0],interactive:false,created_at:'2026-11-01',properties:{}};
            CREATE node_log CONTENT {log_id:'legacy-fact-99',node_id:$id,hlc_timestamp:1,op:'legacy_partial',source_did:'did:qa',payload:{},row_version:99,created_at:'2026-11-01'}")
            .bind(("id",legacy_id)).await.unwrap().check().unwrap();
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            legacy_id,
            "calibration",
            &serde_json::json!({"gain":2}),
            "did:qa",
        )
        .await
        .unwrap();
        let unavailable =
            crate::handlers::node_history::read(&db, legacy_id, 0, 1, Some(99), "did:qa")
                .await
                .unwrap();
        assert_eq!(unavailable["coverage_start_version"], 100);
        assert_eq!(unavailable["availability"], "before_coverage");
        assert!(unavailable.get("state").is_none());

        use fe_runtime::node_history::NodeAccessGrant;
        let scope_a = crate::build_scope(
            "history-verse",
            Some("history-fractal"),
            Some("history-petal"),
        );
        let scope_b = crate::build_scope(
            "history-verse",
            Some("history-fractal"),
            Some("history-petal-b"),
        );
        let grant_a = NodeAccessGrant::Scoped(scope_a.clone());
        let grant_b = NodeAccessGrant::Scoped(scope_b);
        assert!(
            crate::handlers::node_history::read_granted(
                &db,
                &node_id,
                0,
                1,
                None,
                &NodeAccessGrant::Scoped("VERSE#history".into()),
                "did:qa"
            )
            .await
            .is_err(),
            "a hyphen-bearing verse id cannot be covered by an identifier prefix"
        );
        let (captured_fence, captured_bindings) =
            crate::handlers::node_history::scope_fence(&scope_a, Some(&node_id)).unwrap();
        db.query("CREATE petal CONTENT {petal_id:'history-petal-b',fractal_id:'history-fractal',name:'B',node_id:'b',created_at:'2026-11-01'};
            UPDATE node SET petal_id='history-petal-b' WHERE node_id=$id")
            .bind(("id",node_id.clone())).await.unwrap().check().unwrap();
        let state_after_reparent = crate::handlers::node_history::snapshot(&db, &node_id)
            .await
            .unwrap();
        let count_before_denial = op_log_count(&db).await;
        let denied = crate::handlers::entity_property::set_entity_property_granted(
            &db,
            &node_id,
            "calibration",
            &serde_json::json!({"gain":999}),
            "did:queued-editor",
            &grant_a,
        )
        .await
        .expect_err("queued old grant cannot write the new petal");
        assert!(denied.to_string().contains("not permitted"));
        assert_eq!(
            crate::handlers::node_history::snapshot(&db, &node_id)
                .await
                .unwrap(),
            state_after_reparent
        );
        assert_eq!(op_log_count(&db).await, count_before_denial);
        db.query("CREATE verse CONTENT {verse_id:'foreign-history-verse',name:'foreign',created_by:'did:qa',created_at:'2026-11-01'};
            CREATE fractal CONTENT {fractal_id:'foreign-history-fractal',verse_id:'foreign-history-verse',owner_did:'did:qa',name:'foreign',created_at:'2026-11-01'};
            CREATE petal CONTENT {petal_id:'queued-create-petal',fractal_id:'history-fractal',name:'queued',node_id:'queued',created_at:'2026-11-01'}")
            .await.unwrap().check().unwrap();
        let prechecked_create_scope = crate::handlers::preconditions::require_petal_scope(
            &db,
            "queued-create-petal",
            "precheck",
        )
        .await
        .unwrap();
        assert_eq!(
            prechecked_create_scope,
            crate::build_scope(
                "history-verse",
                Some("history-fractal"),
                Some("queued-create-petal")
            )
        );
        let queued_create_grant = NodeAccessGrant::Scoped("VERSE#history-verse".into());
        db.query("UPDATE petal SET fractal_id='foreign-history-fractal' WHERE petal_id='queued-create-petal'").await.unwrap().check().unwrap();
        crate::handlers::node_history::create_object_granted(
            &db,
            "queued-create-petal",
            "queued create must fail",
            None,
            [0.0; 3],
            vec![0.0; 3],
            vec![1.0; 3],
            serde_json::Map::new(),
            "did:queued-editor",
            &queued_create_grant,
        )
        .await
        .expect_err("petal moved between API precheck and DB create");
        let queued_nodes: Vec<serde_json::Value> = db
            .query("SELECT node_id FROM node WHERE petal_id='queued-create-petal'")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert!(queued_nodes.is_empty());
        assert_eq!(op_log_count(&db).await, count_before_denial);
        assert_eq!(
            crate::handlers::node_history::read(&db, &node_id, 0, 1, None, "did:qa")
                .await
                .unwrap()["current_version"],
            8
        );
        let fence_error =
            commit_operation_atomic(&db, probe_entry(), &captured_fence, captured_bindings)
                .await
                .expect_err("in-transaction ancestry fence must reject the old resolution");
        assert!(fence_error
            .to_string()
            .contains("authorized ancestry changed"));
        assert_eq!(
            op_log_count(&db).await,
            count_before_denial,
            "scope fence rolls back the intention too"
        );
        crate::handlers::node_history::create_object_granted(
            &db,
            "history-petal-b",
            "denied create",
            None,
            [0.0; 3],
            vec![0.0; 3],
            vec![1.0; 3],
            serde_json::Map::new(),
            "did:queued-editor",
            &grant_a,
        )
        .await
        .expect_err("queued create grant must match the current petal ancestry");
        assert_eq!(op_log_count(&db).await, count_before_denial);
        assert!(crate::handlers::node_history::read_granted(
            &db, &node_id, 0, 200, None, &grant_a, "did:qa"
        )
        .await
        .is_err());
        let old_page = crate::handlers::node_history::read_granted(
            &db, &node_id, 0, 1, None, &grant_b, "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(old_page["entries"][0]["op"], "unavailable_interval");
        assert_eq!(
            old_page["entries"][0]["payload"],
            serde_json::json!({"availability":"unavailable_scope"})
        );
        assert_eq!(old_page["entries"][0]["source_did"], "");
        assert!(old_page["coverage_started_at"].is_null());
        let old_state = crate::handlers::node_history::read_granted(
            &db,
            &node_id,
            0,
            1,
            Some(3),
            &grant_b,
            "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(old_state["availability"], "unavailable_scope");
        assert!(old_state.get("state").is_none());
        crate::handlers::entity_property::set_entity_property_granted(
            &db,
            &node_id,
            "calibration",
            &serde_json::json!({"gain":5}),
            "did:matching-editor",
            &grant_b,
        )
        .await
        .unwrap();
        let new_state = crate::handlers::node_history::read_granted(
            &db,
            &node_id,
            0,
            1,
            Some(10),
            &grant_b,
            "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(new_state["availability"], "available");
        assert_eq!(new_state["baseline_version"], 9);
        assert_eq!(new_state["state"]["properties"]["calibration"]["gain"], 5);
        crate::handlers::node_log::append_node_log(
            &db,
            &node_id,
            "legacy_unknown_scope",
            "secret-old-actor",
            &serde_json::json!({"secret":"must-not-leak"}),
        )
        .await
        .unwrap();
        let unknown_page = crate::handlers::node_history::read_granted(
            &db, &node_id, 10, 1, None, &grant_b, "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(unknown_page["entries"][0]["op"], "unavailable_interval");
        assert!(!unknown_page.to_string().contains("must-not-leak"));
        assert!(!unknown_page.to_string().contains("secret-old-actor"));
        db.query("DELETE node WHERE node_id=$id")
            .bind(("id", node_id.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(crate::handlers::node_history::read_granted(
            &db, &node_id, 0, 1, None, &grant_a, "did:qa"
        )
        .await
        .is_err());
        let retained = crate::handlers::node_history::read_granted(
            &db, &node_id, 0, 1, None, &grant_b, "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(retained["entries"][0]["op"], "unavailable_interval");

        let local_user = "did:local-history-user";
        assert_eq!(
            crate::object_command_actor(&NodeAccessGrant::Local, local_user, "spoofed-owner"),
            local_user
        );
        assert_eq!(
            crate::object_command_actor(&grant_b, local_user, "opaque-api-subject"),
            "opaque-api-subject"
        );
        db.query(
            "CREATE role:local_history_role_a CONTENT {peer_did:$did,scope:$scope,role:'viewer'}",
        )
        .bind(("did", local_user))
        .bind(("scope", scope_a.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
        let local_before = crate::handlers::node_history::snapshot(&db, legacy_id)
            .await
            .unwrap();
        let local_petal = crate::parse_scope(&scope_a).unwrap().petal_id.unwrap();
        assert!(crate::handlers::scene_materials::read(
            &db,
            &local_petal,
            &scope_a,
            &[legacy_id.to_owned()],
            local_user
        )
        .await
        .is_ok());
        assert!(crate::handlers::scene_materials::read(
            &db,
            &local_petal,
            &scope_a,
            &[legacy_id.to_owned()],
            "did:no-material-access"
        )
        .await
        .is_err());
        let local_op_count = op_log_count(&db).await;
        let local_refusals = [
            crate::handlers::entity_property::set_entity_property_as(
                &db,
                legacy_id,
                "calibration",
                &serde_json::json!({"gain":999}),
                local_user,
            )
            .await,
            crate::handlers::entity_property::delete_entity_property_as(
                &db,
                legacy_id,
                "calibration",
                local_user,
            )
            .await,
            crate::handlers::transform::update_node_transform_as(
                &db, legacy_id, [9.0; 3], [0.0; 3], [1.0; 3], local_user,
            )
            .await,
            crate::handlers::node_history::create_object_granted(
                &db,
                "history-petal",
                "local-viewer-create",
                None,
                [0.0; 3],
                vec![0.0; 3],
                vec![1.0; 3],
                serde_json::Map::new(),
                local_user,
                &NodeAccessGrant::Local,
            )
            .await
            .map(|_| ()),
        ];
        assert!(
            local_refusals.into_iter().all(|result| result.is_err()),
            "local Viewer cannot write ordinary objects"
        );
        assert_eq!(
            crate::handlers::node_history::snapshot(&db, legacy_id)
                .await
                .unwrap(),
            local_before
        );
        assert_eq!(op_log_count(&db).await, local_op_count);
        let local_read = crate::handlers::node_history::read_granted(
            &db,
            legacy_id,
            100,
            1,
            None,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert_eq!(local_read["current_version"], 101);
        let denied_creates: Vec<serde_json::Value> = db
            .query("SELECT node_id FROM node WHERE display_name='local-viewer-create'")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert!(denied_creates.is_empty());
        let local_definition = crate::handlers::field_def::create_field_def_handler(
            &db,
            &scope_a,
            "node",
            "schema_local",
            "object",
            None,
            "did:qa",
        )
        .await
        .unwrap();
        let local_descriptor = serde_json::json!({"version":1,"schema":{"type":"object","properties":{"label":{"type":"string"}}}});
        let viewer_descriptor = crate::handlers::field_def::get_field_schema_handler(
            &db,
            &local_definition,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert!(viewer_descriptor["enforced_schema"].is_null());
        crate::handlers::field_def::set_field_schema_handler(
            &db,
            &local_definition,
            &local_descriptor,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .expect_err("local Viewer cannot install a schema");
        assert!(crate::handlers::field_def::get_field_schema_handler(
            &db,
            &local_definition,
            &NodeAccessGrant::Local,
            local_user
        )
        .await
        .unwrap()["enforced_schema"]
            .is_null());
        db.query("UPDATE role:local_history_role_a SET role='editor'")
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut initial_properties = serde_json::Map::new();
        initial_properties.insert("secret".into(), serde_json::json!("old-local-scope-secret"));
        let (local_node, _) = crate::handlers::node_history::create_object_granted(
            &db,
            "history-petal",
            "local-editor-create",
            None,
            [0.0; 3],
            vec![0.0; 3],
            vec![1.0; 3],
            initial_properties,
            local_user,
            &NodeAccessGrant::Local,
        )
        .await
        .unwrap();
        let local_birth = crate::handlers::node_history::read_granted(
            &db,
            &local_node,
            0,
            1,
            None,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert_eq!(local_birth["entries"][0]["source_did"], local_user);
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            &local_node,
            "counter",
            &serde_json::json!(1),
            local_user,
        )
        .await
        .unwrap();
        crate::handlers::field_def::set_field_schema_handler(
            &db,
            &local_definition,
            &local_descriptor,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .expect_err("local Editor cannot install a schema");
        db.query("UPDATE role:local_history_role_a SET role='manager'")
            .await
            .unwrap()
            .check()
            .unwrap();
        crate::handlers::field_def::set_field_schema_handler(
            &db,
            &local_definition,
            &local_descriptor,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert_eq!(
            crate::handlers::field_def::get_field_schema_handler(
                &db,
                &local_definition,
                &NodeAccessGrant::Local,
                local_user
            )
            .await
            .unwrap()["enforced_schema"],
            local_descriptor
        );
        db.query(
            "CREATE role:local_history_role_b CONTENT {peer_did:$did,scope:$scope,role:'editor'};
            UPDATE node SET petal_id='history-petal-b' WHERE node_id=$id",
        )
        .bind(("did", local_user))
        .bind((
            "scope",
            crate::build_scope(
                "history-verse",
                Some("history-fractal"),
                Some("history-petal-b"),
            ),
        ))
        .bind(("id", local_node.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            &local_node,
            "counter",
            &serde_json::json!(2),
            local_user,
        )
        .await
        .unwrap();
        db.query("UPDATE role:local_history_role_a SET role='none'; UPDATE role:local_history_role_b SET role='viewer'")
            .await.unwrap().check().unwrap();
        let revoked_page = crate::handlers::node_history::read_granted(
            &db,
            &local_node,
            0,
            2,
            None,
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert!(revoked_page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["op"] == "unavailable_interval"));
        assert!(!revoked_page.to_string().contains("old-local-scope-secret"));
        let revoked_state = crate::handlers::node_history::read_granted(
            &db,
            &local_node,
            0,
            1,
            Some(2),
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert_eq!(revoked_state["availability"], "unavailable_scope");
        assert!(revoked_state.get("state").is_none());
        assert!(
            crate::handlers::field_def::get_field_schema_handler(
                &db,
                &local_definition,
                &NodeAccessGrant::Local,
                local_user
            )
            .await
            .is_err(),
            "current petal read permission cannot expose a revoked past scope's descriptor"
        );
        let current_local_state = crate::handlers::node_history::read_granted(
            &db,
            &local_node,
            0,
            1,
            Some(4),
            &NodeAccessGrant::Local,
            local_user,
        )
        .await
        .unwrap();
        assert_eq!(current_local_state["availability"], "available");
        let after_demotion = op_log_count(&db).await;
        crate::handlers::entity_property::set_entity_property_as(
            &db,
            &local_node,
            "counter",
            &serde_json::json!(999),
            local_user,
        )
        .await
        .expect_err("demoted local identity cannot keep writing");
        assert_eq!(op_log_count(&db).await, after_demotion);
        crate::handlers::entity_property::set_entity_property_granted(
            &db,
            &local_node,
            "counter",
            &serde_json::json!(3),
            "opaque-api-subject",
            &grant_b,
        )
        .await
        .unwrap();
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &local_node)
                .await
                .unwrap()["history_version"],
            5
        );

        // Hierarchy transactions use the same stored role resolver and preserve journal facts.
        use crate::handlers::{crud, entity};
        use fe_runtime::messages::EntityType;
        let blob_store: crate::BlobStoreHandle =
            std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        db.query("CREATE verse CONTENT {verse_id:'tree-1',name:'one',created_by:'did:qa',created_at:'2026-11-01'};
            CREATE verse CONTENT {verse_id:'tree-10',name:'ten',created_by:'did:qa',created_at:'2026-11-01'};
            CREATE fractal CONTENT {fractal_id:'tree-fractal',verse_id:'tree-1',owner_did:'did:qa',name:'f',created_at:'2026-11-01'};
            CREATE fractal CONTENT {fractal_id:'tree-other-fractal',verse_id:'tree-10',owner_did:'did:qa',name:'other',created_at:'2026-11-01'};
            CREATE petal CONTENT {petal_id:'tree-petal',fractal_id:'tree-fractal',name:'p',node_id:'p',created_at:'2026-11-01'};
            CREATE petal CONTENT {petal_id:'tree-other-petal',fractal_id:'tree-other-fractal',name:'other',node_id:'p',created_at:'2026-11-01'};
            CREATE role:tree_local CONTENT {peer_did:'did:tree-user',scope:'VERSE#tree-1',role:'viewer'};
            CREATE role:tree_prefix_other CONTENT {peer_did:'did:other',scope:'VERSE#tree-10',role:'viewer'};")
            .await.unwrap().check().unwrap();
        let tree_node = crud::create_node_as(&db, "tree-petal", "tree node", [0.0; 3], "did:qa")
            .await
            .unwrap();
        let other_node =
            crud::create_node_as(&db, "tree-other-petal", "other node", [0.0; 3], "did:qa")
                .await
                .unwrap();
        let before_tree = crate::handlers::node_history::committed_receipt(&db, &tree_node)
            .await
            .unwrap();
        let before_tree_ops = op_log_count(&db).await;
        for role in ["none", "viewer", "editor"] {
            db.query("UPDATE role:tree_local SET role=$role")
                .bind(("role", role))
                .await
                .unwrap()
                .check()
                .unwrap();
            for (kind, id) in [
                (EntityType::Verse, "tree-1"),
                (EntityType::Fractal, "tree-fractal"),
                (EntityType::Petal, "tree-petal"),
            ] {
                let details =
                    entity::get_entity_details_handler(&db, kind, id, "did:tree-user").await;
                if role == "none" {
                    details.expect_err("settings reads require current stored Viewer permission");
                } else {
                    let details = details.unwrap();
                    assert_eq!(details.entity_type, kind);
                    assert_eq!(details.entity_id, id);
                    assert_eq!(details.verse_id, "tree-1");
                    assert_eq!(details.role, role);
                }
            }
            for (kind, id) in [
                (EntityType::Verse, "tree-1"),
                (EntityType::Fractal, "tree-fractal"),
                (EntityType::Petal, "tree-petal"),
            ] {
                entity::rename_entity_handler(&db, kind, id, "forbidden", "did:tree-user")
                    .await
                    .expect_err("below Manager rename");
                entity::delete_entity_handler(&db, kind, id, "did:tree-user")
                    .await
                    .expect_err("below Manager cascade");
            }
            entity::update_fractal_description_handler(
                &db,
                "tree-fractal",
                "forbidden",
                "did:tree-user",
            )
            .await
            .expect_err("below Manager description");
            crud::create_fractal_handler(
                &db,
                &blob_store,
                None,
                "tree-1",
                "forbidden",
                "did:tree-user",
            )
            .await
            .expect_err("below Manager child creation");
            crud::create_petal_handler(&db, "tree-fractal", "forbidden", "did:tree-user")
                .await
                .expect_err("below Manager child creation");
        }
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &tree_node)
                .await
                .unwrap(),
            before_tree
        );
        assert_eq!(op_log_count(&db).await, before_tree_ops);
        let mut unchanged = db.query("SELECT name FROM verse WHERE verse_id='tree-1'; SELECT name, description FROM fractal WHERE fractal_id='tree-fractal'; SELECT name FROM petal WHERE petal_id='tree-petal'").await.unwrap().check().unwrap();
        assert_eq!(
            unchanged.take::<Vec<serde_json::Value>>(0).unwrap()[0]["name"],
            "one"
        );
        let unchanged_fractal: Vec<serde_json::Value> = unchanged.take(1).unwrap();
        assert_eq!(unchanged_fractal[0]["name"], "f");
        assert_ne!(unchanged_fractal[0]["description"], "forbidden");
        assert_eq!(
            unchanged.take::<Vec<serde_json::Value>>(2).unwrap()[0]["name"],
            "p"
        );

        db.query("UPDATE role:tree_local SET role='manager'")
            .await
            .unwrap()
            .check()
            .unwrap();
        entity::rename_entity_handler(
            &db,
            EntityType::Petal,
            "tree-petal",
            "managed",
            "did:tree-user",
        )
        .await
        .unwrap();
        entity::update_fractal_description_handler(
            &db,
            "tree-fractal",
            "managed description",
            "did:tree-user",
        )
        .await
        .unwrap();
        let child_fractal = crud::create_fractal_handler(
            &db,
            &blob_store,
            None,
            "tree-1",
            "managed child",
            "did:tree-user",
        )
        .await
        .unwrap();
        let child_petal =
            crud::create_petal_handler(&db, &child_fractal, "managed child", "did:tree-user")
                .await
                .unwrap();
        entity::delete_entity_handler(&db, EntityType::Petal, &child_petal, "did:tree-user")
            .await
            .unwrap();
        entity::delete_entity_handler(&db, EntityType::Fractal, &child_fractal, "did:tree-user")
            .await
            .unwrap();
        entity::rename_entity_handler(
            &db,
            EntityType::Petal,
            "missing-tree-petal",
            "wrong",
            "did:qa",
        )
        .await
        .expect_err("missing target refuses");

        db.query("CREATE petal:tree_duplicate CONTENT {petal_id:'tree-petal',fractal_id:'tree-other-fractal',name:'duplicate',node_id:'p',created_at:'2026-11-01'}").await.unwrap().check().unwrap();
        entity::rename_entity_handler(
            &db,
            EntityType::Petal,
            "tree-petal",
            "ambiguous write",
            "did:tree-user",
        )
        .await
        .expect_err("duplicate target identity fails closed");
        entity::delete_entity_handler(&db, EntityType::Verse, "tree-1", "did:tree-user")
            .await
            .expect_err("ambiguous descendant identity cannot cross a foreign subtree");
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &tree_node)
                .await
                .unwrap(),
            before_tree
        );
        db.query("DELETE petal:tree_duplicate")
            .await
            .unwrap()
            .check()
            .unwrap();
        let prepared =
            entity::prepare_parent_mutation(&db, EntityType::Petal, "tree-petal", "did:tree-user")
                .await
                .unwrap();
        db.query("UPDATE petal SET fractal_id='tree-other-fractal' WHERE petal_id='tree-petal'")
            .await
            .unwrap()
            .check()
            .unwrap();
        prepared
            .commit(
                &db,
                &["UPDATE petal SET name='stale write' WHERE petal_id='tree-petal'"],
                vec![],
            )
            .await
            .expect_err("moved target refuses inside transaction");
        db.query("UPDATE petal SET fractal_id='tree-fractal' WHERE petal_id='tree-petal'")
            .await
            .unwrap()
            .check()
            .unwrap();
        let prepared =
            entity::prepare_parent_mutation(&db, EntityType::Petal, "tree-petal", "did:tree-user")
                .await
                .unwrap();
        db.query("UPDATE role:tree_local SET role='viewer'")
            .await
            .unwrap()
            .check()
            .unwrap();
        prepared
            .commit(
                &db,
                &["UPDATE petal SET name='stale role write' WHERE petal_id='tree-petal'"],
                vec![],
            )
            .await
            .expect_err("revoked Manager refuses inside transaction");
        let captured_scopes = vec![
            crate::build_scope("tree-1", Some("tree-fractal"), Some("tree-petal")),
            crate::build_scope("tree-1", Some("tree-fractal"), None),
            crate::build_scope("tree-1", None, None),
        ];
        let mut captured = db.query("SELECT verse_id, created_by, default_access FROM verse WHERE verse_id='tree-1'; SELECT scope, role FROM role WHERE peer_did='did:tree-user' AND scope IN $scopes ORDER BY scope, role")
            .bind(("scopes",captured_scopes.clone())).await.unwrap().check().unwrap();
        let captured_verse: Vec<serde_json::Value> = captured.take(0).unwrap();
        let captured_roles: Vec<serde_json::Value> = captured.take(1).unwrap();
        db.query("UPDATE role:tree_local SET role='manager'")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            crate::node_role_at(&db, "did:tree-user", "VERSE#tree-1").await,
            fe_policy::RoleLevel::Manager,
            "a separate live resolver can observe temporary Manager"
        );
        entity::require_snapshot_role(
            &captured_verse[0],
            &captured_roles,
            &captured_scopes,
            "did:tree-user",
            fe_policy::RoleLevel::Manager,
        )
        .expect_err("captured Viewer must refuse even while a later live read sees Manager");
        db.query("UPDATE role:tree_local SET role='viewer'")
            .await
            .unwrap()
            .check()
            .unwrap();
        entity::require_snapshot_role(
            &captured_verse[0],
            &captured_roles,
            &captured_scopes,
            "did:tree-user",
            fe_policy::RoleLevel::Manager,
        )
        .expect_err("Viewer to Manager to Viewer cannot authorize the original snapshot");
        db.query("UPDATE role:tree_local SET role='manager'; DEFINE EVENT fail_hierarchy_cascade ON TABLE petal WHEN true THEN { THROW 'induced hierarchy failure' }").await.unwrap().check().unwrap();
        entity::delete_entity_handler(&db, EntityType::Verse, "tree-1", "did:tree-user")
            .await
            .expect_err("mid-cascade failure rolls back all descendants");
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &tree_node)
                .await
                .unwrap(),
            before_tree
        );
        let mut rolled_back=db.query("SELECT name FROM petal WHERE petal_id='tree-petal'; SELECT verse_id FROM verse WHERE verse_id='tree-1'").await.unwrap().check().unwrap();
        assert_eq!(
            rolled_back.take::<Vec<serde_json::Value>>(0).unwrap()[0]["name"],
            "managed"
        );
        assert_eq!(
            rolled_back.take::<Vec<serde_json::Value>>(1).unwrap().len(),
            1
        );
        db.query("REMOVE EVENT fail_hierarchy_cascade ON TABLE petal")
            .await
            .unwrap()
            .check()
            .unwrap();
        entity::delete_entity_handler(&db, EntityType::Verse, "tree-1", "did:tree-user")
            .await
            .unwrap();
        assert!(crate::handlers::node_history::snapshot(&db, &tree_node)
            .await
            .unwrap()
            .is_none());
        assert!(crate::handlers::node_history::snapshot(&db, &other_node)
            .await
            .unwrap()
            .is_some());
        let mut exact=db.query("SELECT scope FROM role WHERE scope IN ['VERSE#tree-1','VERSE#tree-10']; SELECT verse_id FROM verse WHERE verse_id IN ['tree-1','tree-10']; SELECT * OMIT id FROM node_log WHERE node_id=$node").bind(("node",tree_node)).await.unwrap().check().unwrap();
        let roles: Vec<serde_json::Value> = exact.take(0).unwrap();
        assert_eq!(roles, vec![serde_json::json!({"scope":"VERSE#tree-10"})]);
        assert_eq!(
            exact.take::<Vec<serde_json::Value>>(1).unwrap(),
            vec![serde_json::json!({"verse_id":"tree-10"})]
        );
        assert_eq!(
            exact.take::<Vec<serde_json::Value>>(2).unwrap().len(),
            1,
            "structural deletion retains baseline facts without fabricating replay coverage"
        );
        // The production plugin bridge persists the entire batch with ordinary journal facts.
        use fe_runtime::plugin_host::{PluginAdmission, PluginRead, PluginReply};
        use fe_sdk::plugin_host::{NodeQuery, PendingOp};
        let plugin_scope = crate::build_scope(
            "tree-10",
            Some("tree-other-fractal"),
            Some("tree-other-petal"),
        );
        let admission = PluginAdmission::new(
            "qa-plugin".into(),
            "tree-other-petal".into(),
            vec![
                fe_sdk::CAP_STORAGE_READ.into(),
                fe_sdk::CAP_STORAGE_WRITE.into(),
                fe_sdk::CAP_QUERY_SELECT.into(),
            ],
            vec!["calibration".into()],
            vec!["*".into()],
            vec![plugin_scope.clone()],
        )
        .unwrap()
        .bind_scope(plugin_scope.clone())
        .unwrap();
        let definition = crate::handlers::field_def::create_field_def_handler(
            &db,
            &plugin_scope,
            "node",
            "calibration",
            "object",
            None,
            "did:qa",
        )
        .await
        .unwrap();
        crate::handlers::field_def::set_field_schema_handler(&db,&definition,&serde_json::json!({"version":1,"schema":{"type":"object","required":["gain"],"properties":{"gain":{"type":"integer","maximum":5}}}}),&NodeAccessGrant::Local,"did:qa").await.unwrap();
        let victim =
            crud::create_node_as(&db, "tree-other-petal", "plain victim", [0.0; 3], "did:qa")
                .await
                .unwrap();
        let mixed = vec![
            PendingOp::SetProperty {
                node_id: other_node.clone(),
                key: "calibration".into(),
                value: serde_json::json!({"gain":2,"nested":{"values":[1,2]}}),
            },
            PendingOp::SetProperty {
                node_id: other_node.clone(),
                key: "calibration".into(),
                value: serde_json::json!({"gain":3,"nested":{"values":[1,2,3]}}),
            },
            PendingOp::CreateNode {
                petal_id: "tree-other-petal".into(),
                name: "plugin-created".into(),
            },
            PendingOp::DeleteNode {
                node_id: victim.clone(),
            },
        ];
        let committed = crate::handlers::plugin_host::commit(&db, &admission, &mixed, "did:qa")
            .await
            .unwrap();
        assert_eq!(committed.receipt.op_count, 4);
        assert_eq!(committed.receipt.created_node_ids.len(), 1);
        assert!(crate::handlers::node_history::snapshot(
            &db,
            &committed.receipt.created_node_ids[0]
        )
        .await
        .unwrap()
        .is_some());
        let latest = crate::handlers::node_history::committed_receipt(&db, &other_node)
            .await
            .unwrap();
        assert_eq!(latest["history_version"], 3);
        assert_eq!(latest["state"]["properties"]["calibration"]["gain"], 3);
        let replay = crate::handlers::node_history::read_granted(
            &db,
            &other_node,
            0,
            1,
            Some(2),
            &NodeAccessGrant::Local,
            "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(replay["state"]["properties"]["calibration"]["gain"], 2);
        let removed = crate::handlers::node_history::snapshot(&db, &victim)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(removed["tombstone"]["source_did"], "did:qa");
        assert!(removed["tombstone"]["hlc"].as_u64().unwrap() > 0);
        let unsupported = crate::handlers::node_history::read_granted(
            &db,
            &victim,
            0,
            1,
            Some(2),
            &NodeAccessGrant::Local,
            "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(unsupported["availability"], "unsupported_operation");
        assert!(unsupported.get("state").is_none());
        match crate::handlers::plugin_host::read(
            &db,
            &admission,
            PluginRead::Nodes(NodeQuery::parse(r#"{"name":"other node","limit":1}"#).unwrap()),
            "did:qa",
        )
        .await
        .unwrap()
        {
            PluginReply::Nodes(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].node_id, other_node);
            }
            other => panic!("unexpected plugin query reply: {other:?}"),
        }
        for id in [local_node.as_str(), "missing-plugin-node"] {
            assert!(
                matches!(
                    crate::handlers::plugin_host::read(
                        &db,
                        &admission,
                        PluginRead::Node(id.into()),
                        "did:qa"
                    )
                    .await
                    .unwrap(),
                    PluginReply::Node(None)
                ),
                "foreign and absent node reads are uniform"
            );
        }
        db.query("CREATE node CONTENT {node_id:'plugin-query-a-float',petal_id:'tree-other-petal',display_name:'exact-numeric',position:<geometry<point>> [0.0,0.0],rotation:[0.0,0.0,0.0],scale:[1.0,1.0,1.0],created_at:'2026-11-01',properties:{calibration:1.0}}; CREATE node CONTENT {node_id:'plugin-query-b-int',petal_id:'tree-other-petal',display_name:'exact-numeric',position:<geometry<point>> [0.0,0.0],rotation:[0.0,0.0,0.0],scale:[1.0,1.0,1.0],created_at:'2026-11-01',properties:{calibration:1}};").await.unwrap().check().unwrap();
        let exact = NodeQuery::parse(
            r#"{"name":"exact-numeric","property":{"key":"calibration","value":1},"limit":1}"#,
        )
        .unwrap();
        match crate::handlers::plugin_host::read(
            &db,
            &admission,
            PluginRead::Nodes(exact.clone()),
            "did:qa",
        )
        .await
        .unwrap()
        {
            PluginReply::Nodes(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].node_id, "plugin-query-b-int");
                assert!(exact.matches(&nodes[0]));
            }
            other => panic!("unexpected numeric query reply {other:?}"),
        }
        db.query("DELETE node WHERE node_id IN ['plugin-query-a-float','plugin-query-b-int']")
            .await
            .unwrap()
            .check()
            .unwrap();
        let lease = fe_sdk::plugin_host::PluginLease::default();
        let revoked = admission.clone().with_lease(lease.clone());
        // This models a request cloned/enqueued before instance removal but used afterward.
        lease.revoke();
        let before_revoked = op_log_count(&db).await;
        crate::handlers::plugin_host::read(
            &db,
            &revoked,
            PluginRead::Node(other_node.clone()),
            "did:qa",
        )
        .await
        .expect_err("queued read loses its revoked lease");
        crate::handlers::plugin_host::read(&db, &revoked, PluginRead::PetalScope, "did:qa")
            .await
            .expect_err("scope handshake loses its revoked lease");
        crate::handlers::plugin_host::commit(&db, &revoked, &mixed[..1], "did:qa")
            .await
            .expect_err("queued commit loses its revoked lease");
        assert_eq!(op_log_count(&db).await, before_revoked);
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &other_node)
                .await
                .unwrap(),
            latest
        );
        use fe_runtime::node_history::{NodeEditBefore,NodeEditPrecondition};
        use fe_runtime::messages::NodeMutation;
        let undo_node=crud::create_node_as(&db,"tree-other-petal","undo point fixture",[0.0;3],"did:qa").await.unwrap();
        let path_before=serde_json::json!([[1.0,0.0,2.0,17.25,-0.5,0.0,0.0,0.5,0.0,0.0,2.0,0.6],[8.0,2.0,9.0,29.5]]);
        crate::handlers::entity_property::set_entity_property_as(&db,&undo_node,"gpx_points",&path_before,"did:qa").await.unwrap();
        let base=crate::handlers::node_history::receipt(&db,&undo_node).await.unwrap();
        let mut path_after=path_before.clone();path_after[0][0]=serde_json::json!(4.0);path_after[0][1]=serde_json::json!(3.0);path_after[0][2]=serde_json::json!(6.0);
        let expected=NodeEditPrecondition {scope:plugin_scope.clone(),history_version:Some(base.history_version),before:NodeEditBefore::PathPoints(path_before.clone())};
        let point_move=NodeMutation::PropertySet {key:"gpx_points".into(),value:path_after.clone()};
        let saved=crate::handlers::checked_mutation::commit(&db,&undo_node,&point_move,&expected,"qa-undo-point","did:qa").await.unwrap();
        assert_eq!(saved.state.properties["gpx_points"],path_after);
        assert_eq!(saved.state.properties["gpx_points"][0].as_array().unwrap()[3..],path_before[0].as_array().unwrap()[3..]);
        let inverse_expected=NodeEditPrecondition {scope:plugin_scope.clone(),history_version:Some(saved.history_version),before:NodeEditBefore::PathPoints(path_after.clone())};
        let inverse=NodeMutation::PropertySet {key:"gpx_points".into(),value:path_before.clone()};
        let unchanged_ops=op_log_count(&db).await;
        for (request,guard,actor) in [("qa-undo-stale",&expected,"did:qa"),("qa-undo-viewer",&inverse_expected,"did:tree-viewer")] {
            crate::handlers::checked_mutation::commit(&db,&undo_node,&inverse,guard,request,actor).await.expect_err("stale or insufficient-role inverse must refuse");
        }
        db.query("DEFINE EVENT fail_editor_fact ON TABLE node_log WHEN $after.payload.editor_request_id='qa-undo-fail' THEN {THROW 'induced editor journal failure'}").await.unwrap().check().unwrap();
        crate::handlers::checked_mutation::commit(&db,&undo_node,&inverse,&inverse_expected,"qa-undo-fail","did:qa").await.expect_err("journal failure rolls checked state and intention back together");
        db.query("REMOVE EVENT fail_editor_fact ON TABLE node_log").await.unwrap().check().unwrap();
        db.query("CREATE fractal SET fractal_id='undo-reparent-fractal',verse_id='tree-10',owner_did='did:qa',name='other',created_at='2026-11-02'; UPDATE petal SET fractal_id='undo-reparent-fractal' WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();
        crate::handlers::checked_mutation::commit(&db,&undo_node,&inverse,&inverse_expected,"qa-undo-moved","did:qa").await.expect_err("queued old ancestry must refuse after reparent");
        db.query("UPDATE petal SET fractal_id='tree-other-fractal' WHERE petal_id='tree-other-petal'; DELETE fractal WHERE fractal_id='undo-reparent-fractal'").await.unwrap().check().unwrap();
        assert_eq!(op_log_count(&db).await,unchanged_ops);
        let still_saved=crate::handlers::node_history::receipt(&db,&undo_node).await.unwrap();
        assert_eq!(still_saved.history_version,saved.history_version);assert_eq!(still_saved.state.properties["gpx_points"],path_after);
        let undone=crate::handlers::checked_mutation::commit(&db,&undo_node,&inverse,&inverse_expected,"qa-undo-inverse","did:qa").await.unwrap();
        assert!(undone.history_version>saved.history_version);assert_eq!(undone.state.properties["gpx_points"],path_before);
        let (recovered,current)=crate::handlers::checked_mutation::resolve(&db,&undo_node,"tree-other-petal","qa-undo-point","did:qa").await.unwrap();
        assert_eq!(recovered.unwrap().history_version,saved.history_version);assert_eq!(current.properties["gpx_points"],path_before);
        let removed_points=serde_json::json!([path_before[1].clone()]);
        let removal=NodeMutation::PropertySet {key:"gpx_points".into(),value:removed_points.clone()};
        let remove_guard=NodeEditPrecondition {scope:plugin_scope.clone(),history_version:Some(undone.history_version),before:NodeEditBefore::PathPoints(path_before.clone())};
        let removed=crate::handlers::checked_mutation::commit(&db,&undo_node,&removal,&remove_guard,"qa-remove-point","did:qa").await.unwrap();
        assert_eq!(removed.state.properties["gpx_points"],removed_points);
        let restore_guard=NodeEditPrecondition {scope:plugin_scope.clone(),history_version:Some(removed.history_version),before:NodeEditBefore::PathPoints(removed_points.clone())};
        let restore=NodeMutation::PropertySet {key:"gpx_points".into(),value:path_before.clone()};
        let before_bad_removals=op_log_count(&db).await;
        let mut changed_survivor=path_before.clone();changed_survivor[1][3]=serde_json::json!(999);
        for (request,mutation,guard,actor) in [
            ("qa-remove-stale",removal.clone(),remove_guard.clone(),"did:qa"),
            ("qa-remove-viewer",restore.clone(),restore_guard.clone(),"did:tree-viewer"),
            ("qa-remove-metadata",NodeMutation::PropertySet {key:"gpx_points".into(),value:changed_survivor},restore_guard.clone(),"did:qa"),
            ("qa-remove-many",NodeMutation::PropertySet {key:"gpx_points".into(),value:serde_json::json!([])},remove_guard.clone(),"did:qa")
        ] { crate::handlers::checked_mutation::commit(&db,&undo_node,&mutation,&guard,request,actor).await.expect_err("invalid removal refuses atomically"); }
        assert_eq!(op_log_count(&db).await,before_bad_removals);
        let restored=crate::handlers::checked_mutation::commit(&db,&undo_node,&restore,&restore_guard,"qa-restore-point","did:qa").await.unwrap();
        assert_eq!(restored.state.properties["gpx_points"],path_before);
        let mut metadata_after=path_before.clone();metadata_after[0][10]=serde_json::json!(0.0);
        let metadata_guard=NodeEditPrecondition {scope:plugin_scope.clone(),history_version:Some(restored.history_version),before:NodeEditBefore::PathPoints(path_before.clone())};
        let metadata=NodeMutation::PropertySet {key:"gpx_points".into(),value:metadata_after.clone()};
        let metadata_saved=crate::handlers::checked_mutation::commit(&db,&undo_node,&metadata,&metadata_guard,"qa-anchor-metadata","did:qa").await.unwrap();
        assert_eq!(metadata_saved.state.properties["gpx_points"],metadata_after);
        let metadata_inverse=NodeEditPrecondition {scope:plugin_scope.clone(),history_version:Some(metadata_saved.history_version),before:NodeEditBefore::PathPoints(metadata_after.clone())};
        let unchanged=op_log_count(&db).await;
        let mut bad_timestamp=path_before.clone();bad_timestamp[0][3]=serde_json::json!(999.0);
        let mut bad_second_row=path_before.clone();bad_second_row[1]=serde_json::json!([8.0,2.0,9.0,29.5,0.0,0.0,0.0,0.0,0.0,0.0,1.0,0.5]);
        for (request,value) in [("qa-anchor-time",bad_timestamp),("qa-anchor-many",bad_second_row)] {
            crate::handlers::checked_mutation::commit(&db,&undo_node,&NodeMutation::PropertySet {key:"gpx_points".into(),value},&metadata_inverse,request,"did:qa").await.expect_err("metadata edit cannot rewrite timestamp or multiple anchors");
        }
        assert_eq!(op_log_count(&db).await,unchanged);
        let metadata_restored=crate::handlers::checked_mutation::commit(&db,&undo_node,&NodeMutation::PropertySet {key:"gpx_points".into(),value:path_before.clone()},&metadata_inverse,"qa-anchor-inverse","did:qa").await.unwrap();
        assert_eq!(metadata_restored.state.properties["gpx_points"],path_before);
        let stamp_points=serde_json::json!([[1.0,0.0,2.0,17.25,0.0,0.0,0.0,0.0,0.0,20.0,1.0,0.5],[8.0,2.0,9.0,29.5,-20.0,0.0,0.0,0.0,0.0,0.0,1.0,0.5]]);
        crate::handlers::entity_property::set_entity_property_as(&db,&undo_node,"gpx_points",&stamp_points,"did:qa").await.unwrap();
        let stamp_descriptor=serde_json::json!({"asset_path":"blob://fixture.glb","spacing_mode":"fixed_count","spacing_value":0.0,"count":3,"tangent_align":true});
        crate::handlers::entity_property::set_entity_property_as(&db,&undo_node,"path_asset",&stamp_descriptor,"did:qa").await.unwrap();
        db.query("UPDATE petal SET terrain=NONE, description=NONE, hexon_manifest=NONE, visibility_groups=NONE, view_hidden=NONE, render_distance=NONE, bounds=<geometry<polygon>> {type:'Polygon',coordinates:[[[-10.0,-10.0],[10.0,-10.0],[10.0,10.0],[-10.0,10.0],[-10.0,-10.0]]]} WHERE petal_id='tree-other-petal'")
            .await.unwrap().check().unwrap();
        let petal_rows: Vec<serde_json::Value> = db.query("SELECT *, terrain=NONE AS terrain_absent, description=NONE AS description_absent, visibility_groups=NONE AS groups_absent FROM petal WHERE petal_id='tree-other-petal'")
            .await.unwrap().check().unwrap().take(0).unwrap();
        assert_eq!(petal_rows[0]["terrain_absent"],true,"exercise actual Surreal NONE rather than a JSON null terrain");
        assert_eq!(petal_rows[0]["description_absent"],true);
        assert_eq!(petal_rows[0]["groups_absent"],true);
        assert_eq!(petal_rows[0]["visibility"],"private");
        assert_eq!(petal_rows[0]["tags"],serde_json::json!([]));
        assert_eq!(petal_rows[0]["bounds"]["type"],"Polygon","native bounds reproduce the ordinary created Petal");
        for (left,right) in [
            ("{value:NONE}","{value:NULL}"),
            ("<int> 1","<float> 1"),
            ("<decimal> 1","<float> 1"),
            ("<geometry<point>> [1.0,2.0]","[1.0,2.0]"),
            ("petal:typed_snapshot","'petal:typed_snapshot'"),
        ] {
            let snapshot=crate::handlers::preconditions::PETAL_TYPED_SNAPSHOT_SQL;
            let distinct: bool=db.query(format!("RETURN {} != {};",snapshot.replace("$this",left),snapshot.replace("$this",right)))
                .await.unwrap().check().unwrap().take(0).unwrap();
            assert!(distinct,"typed snapshot must preserve {left} versus {right}");
        }
        let promotion_source=crate::handlers::entity_property::get_entity_properties_handler(&db,&undo_node).await.unwrap();
        let promotion=fe_runtime::messages::CheckedStampPromotion {
            path_id:undo_node.clone(),petal_id:"tree-other-petal".into(),scope:plugin_scope.clone(),instance_index:1,
            path_version:None,before_properties:promotion_source.clone(),world_scale:1.0,
        };
        let promotion_node=format!("{undo_node}#inst-1");
        for race in ["role","path","descriptor","points","terrain","bounds","metadata","collision","schema","journal","birth_journal","history_collision"] {
            let prepared=crate::handlers::stamp_promotion::prepare(&db,&promotion,&format!("qa-promotion-{race}"),"did:qa").await.unwrap();
            match race {
                "role"=>{db.query("UPDATE verse SET created_by='did:other' WHERE verse_id='tree-10'").await.unwrap().check().unwrap();}
                "path"=>{db.query("UPDATE node SET petal_id='tree-petal' WHERE node_id=$id").bind(("id",undo_node.clone())).await.unwrap().check().unwrap();}
                "descriptor"=>{db.query("UPDATE node SET properties.path_asset.count=2 WHERE node_id=$id").bind(("id",undo_node.clone())).await.unwrap().check().unwrap();}
                "points"=>{db.query("UPDATE node SET properties.gpx_points=$points WHERE node_id=$id").bind(("id",undo_node.clone())).bind(("points",serde_json::json!([[8.0,2.0,9.0,29.5],[1.0,0.0,2.0,17.25]]))).await.unwrap().check().unwrap();}
                "terrain"=>{db.query("UPDATE petal SET terrain={world_scale:1.0,origin:{lat:0.0,lon:0.0},tileset_hexon_uris:[]} WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "bounds"=>{db.query("UPDATE petal SET bounds=<geometry<polygon>> {type:'Polygon',coordinates:[[[-20.0,-20.0],[20.0,-20.0],[20.0,20.0],[-20.0,20.0],[-20.0,-20.0]]]} WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "metadata"=>{db.query("UPDATE petal SET view_hidden=false WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "collision"=>{db.query("CREATE node CONTENT {node_id:$id,petal_id:'tree-other-petal',properties:{node_kind:'ordinary'}}").bind(("id",promotion_node.clone())).await.unwrap().check().unwrap();}
                "schema"=>{db.query("CREATE field_def CONTENT {field_def_id:'qa-promotion-schema',scope:$scope,entity_type:'node',key:'node_kind',enforced_schema:{version:1,schema:{type:'string'}}}").bind(("scope",plugin_scope.clone())).await.unwrap().check().unwrap();}
                "birth_journal"=>{db.query("DEFINE EVENT fail_promotion_birth ON TABLE node_log WHEN $after.op='baseline' THEN { THROW 'induced promotion baseline failure' }").await.unwrap().check().unwrap();}
                "history_collision"=>{db.query("CREATE node_log CONTENT {log_id:'promotion-orphan',node_id:$id,hlc_timestamp:1,op:'created',source_did:'legacy',row_version:1,payload:{},created_at:'legacy'}").bind(("id",promotion_node.clone())).await.unwrap().check().unwrap();}
                _=>{db.query("DEFINE EVENT fail_checked_promotion ON TABLE node WHEN $after.properties.node_kind='stamp' THEN { THROW 'induced promotion row failure' }").await.unwrap().check().unwrap();}
            }
            let before_ops=op_log_count(&db).await;
            let before_promotion_history=crate::handlers::node_history::latest(&db,&promotion_node).await.unwrap();
            let refused=crate::handlers::stamp_promotion::commit(&db,prepared).await.expect_err("checked GUI promotion race must roll back");
            if matches!(race,"terrain"|"bounds"|"metadata") {
                assert!(format!("{refused:#}").contains("stamp map changed during promotion"),"the real map fence must refuse {race}: {refused:#}");
            }
            assert_eq!(op_log_count(&db).await,before_ops);
            let rows:Vec<serde_json::Value>=db.query("SELECT node_id FROM node WHERE node_id=$id").bind(("id",promotion_node.clone())).await.unwrap().take(0).unwrap();
            assert_eq!(rows.len(),usize::from(race=="collision"),"refusal cannot leak a promoted node");
            assert_eq!(crate::handlers::node_history::latest(&db,&promotion_node).await.unwrap(),before_promotion_history,"refusal cannot rewrite or leak initial history");
            match race {
                "role"=>{db.query("UPDATE verse SET created_by='did:qa' WHERE verse_id='tree-10'").await.unwrap().check().unwrap();}
                "path"=>{db.query("UPDATE node SET petal_id='tree-other-petal' WHERE node_id=$id").bind(("id",undo_node.clone())).await.unwrap().check().unwrap();}
                "descriptor"|"points"=>{db.query("UPDATE node SET properties=$properties WHERE node_id=$id").bind(("id",undo_node.clone())).bind(("properties",promotion_source.clone())).await.unwrap().check().unwrap();}
                "terrain"=>{db.query("UPDATE petal SET terrain=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "bounds"=>{db.query("UPDATE petal SET bounds=<geometry<polygon>> {type:'Polygon',coordinates:[[[-10.0,-10.0],[10.0,-10.0],[10.0,10.0],[-10.0,10.0],[-10.0,-10.0]]]} WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "metadata"=>{db.query("UPDATE petal SET view_hidden=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "collision"=>{db.query("DELETE node WHERE node_id=$id").bind(("id",promotion_node.clone())).await.unwrap().check().unwrap();}
                "schema"=>{db.query("DELETE field_def WHERE field_def_id='qa-promotion-schema'").await.unwrap().check().unwrap();}
                "birth_journal"=>{db.query("REMOVE EVENT fail_promotion_birth ON TABLE node_log").await.unwrap().check().unwrap();}
                "history_collision"=>{db.query("DELETE node_log WHERE log_id='promotion-orphan'").await.unwrap().check().unwrap();}
                _=>{db.query("REMOVE EVENT fail_checked_promotion ON TABLE node").await.unwrap().check().unwrap();}
            }
        }
        let prepared=crate::handlers::stamp_promotion::prepare(&db,&promotion,"qa-promotion-success","did:qa").await.unwrap();
        let (stamp_node,promoted_properties,newly)=crate::handlers::stamp_promotion::commit(&db,prepared).await.unwrap();
        assert!(newly && promoted_properties["owning_path_id"]==undo_node);
        let promotion_baseline=crate::handlers::node_history::latest(&db,&stamp_node).await.unwrap().unwrap();
        assert_eq!(promotion_baseline["row_version"],1);
        assert_eq!(promotion_baseline["op"],"baseline");
        assert_eq!(promotion_baseline["source_did"],"did:qa");
        assert_eq!(promotion_baseline["payload"]["scope"],plugin_scope);
        let promotion_replay=crate::handlers::node_history::read_granted(&db,&stamp_node,0,200,Some(1),&NodeAccessGrant::Local,"did:qa").await.unwrap();
        assert_eq!(promotion_replay["availability"],"available");
        assert_eq!(promotion_replay["state"],crate::handlers::node_history::snapshot(&db,&stamp_node).await.unwrap().unwrap());
        assert_eq!(promotion_replay["state"]["position"],serde_json::json!([0.0,0.0,0.0]),"journal stores node projection, not path-derived world pose");
        assert!(crate::handlers::node_history::read_granted(&db,&stamp_node,0,200,None,&NodeAccessGrant::Scoped("VERSE#foreign".into()),"did:qa").await.is_err());

        let count_after_promotion=op_log_count(&db).await;
        let existing=crate::handlers::stamp_promotion::prepare(&db,&promotion,"qa-promotion-existing","did:qa").await.unwrap();
        let (same,properties,newly)=crate::handlers::stamp_promotion::commit(&db,existing).await.unwrap();
        assert!(!newly && same==stamp_node && properties==promoted_properties);
        assert_eq!(op_log_count(&db).await,count_after_promotion,"idempotent address checks do not mint operations");
        assert_eq!(crate::handlers::node_history::latest(&db,&stamp_node).await.unwrap().unwrap(),promotion_baseline,"repeat promotion retains exact history");
        let mut out_of_range=promotion.clone();out_of_range.instance_index=3;
        crate::handlers::stamp_promotion::prepare(&db,&out_of_range,"qa-promotion-out-of-range","did:qa").await.expect_err("index must exist in the identical shared sampler");
        crate::handlers::stamp_promotion::prepare(&db,&promotion,"qa-promotion-viewer","did:tree-viewer").await.expect_err("Viewer inspection cannot promote");
        let stamp_before=crate::handlers::entity_property::get_entity_properties_handler(&db,&stamp_node).await.unwrap();
        let stamp_input=fe_runtime::messages::StampOverrideEdit {
            node_id:stamp_node.clone(),path_id:undo_node.clone(),instance_index:1,scope:plugin_scope.clone(),
            history_version:None,path_version:None,before_properties:stamp_before.clone(),
            path_anchors:fe_sdk::path_geometry::decode_anchor_rows(&stamp_points).unwrap(),path_descriptor:stamp_descriptor,
            scale:[2.0,1.0,3.0],rotation:[0.0,0.0,0.0,1.0],arc_m:12.0,
        };
        for race in ["role","path","row","terrain","bounds","metadata","schema","journal"] {
            let request=format!("qa-stamp-{race}");
            let prepared=crate::handlers::stamp_overrides::prepare(&db,&stamp_input,&request,"did:qa").await.unwrap();
            match race {
                "role"=>{db.query("UPDATE verse SET created_by='did:other' WHERE verse_id='tree-10'").await.unwrap().check().unwrap();}
                "path"=>{db.query("UPDATE node SET petal_id='tree-petal' WHERE node_id=$id").bind(("id",undo_node.clone())).await.unwrap().check().unwrap();}
                "row"=>{db.query("UPDATE node SET interactive=false WHERE node_id=$id").bind(("id",stamp_node.clone())).await.unwrap().check().unwrap();}
                "terrain"=>{db.query("UPDATE petal SET terrain={world_scale:1.0,origin:{lat:0.0,lon:0.0},tileset_hexon_uris:[]} WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "bounds"=>{db.query("UPDATE petal SET bounds=<geometry<polygon>> {type:'Polygon',coordinates:[[[-20.0,-20.0],[20.0,-20.0],[20.0,20.0],[-20.0,20.0],[-20.0,-20.0]]]} WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "metadata"=>{db.query("UPDATE petal SET view_hidden=false WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "schema"=>{db.query("CREATE field_def CONTENT {field_def_id:'qa-stamp-schema-race',scope:$scope,entity_type:'node',key:'stamp.override.scale',enforced_schema:{version:1,schema:{type:'array'}}}").bind(("scope",plugin_scope.clone())).await.unwrap().check().unwrap();}
                _=>{db.query("DEFINE EVENT fail_stamp_fact ON TABLE node_log WHEN $after.payload.stamp_batch_id='qa-stamp-journal' AND $after.payload.data.key='stamp.override.rotation' THEN { THROW 'induced second stamp fact failure' }").await.unwrap().check().unwrap();}
            }
            let before_ops=op_log_count(&db).await;
            let before_history=crate::handlers::node_history::latest(&db,&stamp_node).await.unwrap();
            let refused=crate::handlers::stamp_overrides::commit(&db,prepared).await.expect_err("captured stamp batch race must roll back entirely");
            if matches!(race,"terrain"|"bounds"|"metadata") {
                assert!(format!("{refused:#}").contains("map changed during save"),"the real map fence must refuse {race}: {refused:#}");
            }
            assert_eq!(op_log_count(&db).await,before_ops);
            assert_eq!(crate::handlers::node_history::latest(&db,&stamp_node).await.unwrap(),before_history);
            assert_eq!(crate::handlers::entity_property::get_entity_properties_handler(&db,&stamp_node).await.unwrap(),stamp_before);
            match race {
                "role"=>{db.query("UPDATE verse SET created_by='did:qa' WHERE verse_id='tree-10'").await.unwrap().check().unwrap();}
                "path"=>{db.query("UPDATE node SET petal_id='tree-other-petal' WHERE node_id=$id").bind(("id",undo_node.clone())).await.unwrap().check().unwrap();}
                "row"=>{db.query("UPDATE node SET interactive=true WHERE node_id=$id").bind(("id",stamp_node.clone())).await.unwrap().check().unwrap();}
                "terrain"=>{db.query("UPDATE petal SET terrain=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "bounds"=>{db.query("UPDATE petal SET bounds=<geometry<polygon>> {type:'Polygon',coordinates:[[[-10.0,-10.0],[10.0,-10.0],[10.0,10.0],[-10.0,10.0],[-10.0,-10.0]]]} WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "metadata"=>{db.query("UPDATE petal SET view_hidden=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "schema"=>{db.query("DELETE field_def WHERE field_def_id='qa-stamp-schema-race'").await.unwrap().check().unwrap();}
                _=>{db.query("REMOVE EVENT fail_stamp_fact ON TABLE node_log").await.unwrap().check().unwrap();}
            }
        }
        let prepared=crate::handlers::stamp_overrides::prepare(&db,&stamp_input,"qa-stamp-success","did:qa").await.unwrap();
        let stamp_saved=crate::handlers::stamp_overrides::commit(&db,prepared).await.unwrap();
        assert_eq!(stamp_saved.history_version,4,"one initial baseline and three atomic property facts");
        let retained:Vec<serde_json::Value>=db.query("SELECT * OMIT id FROM node_log WHERE node_id=$id AND row_version=1").bind(("id",stamp_node.clone())).await.unwrap().check().unwrap().take(0).unwrap();
        assert_eq!(retained,vec![promotion_baseline],"Apply preserves the initial history prefix");
        assert_eq!(stamp_saved.state.properties["stamp.override.scale"],serde_json::json!([2.0,1.0,3.0]));
        assert_eq!(stamp_saved.state.properties["stamp.override.rotation"],serde_json::json!([0.0,0.0,0.0,1.0]));
        assert_eq!(stamp_saved.state.properties["stamp.override.arc_m"],serde_json::json!(12.0));
        assert_eq!(stamp_saved.state.properties["node_kind"],"stamp");
        assert_eq!(stamp_saved.state.position,[0.0;3],"stamp save cannot bypass the position lock");
        let (stamp_receipt,stamp_current)=crate::handlers::checked_mutation::resolve(&db,&stamp_node,"tree-other-petal","qa-stamp-success","did:qa").await.unwrap();
        assert_eq!(stamp_receipt.unwrap().history_version,stamp_saved.history_version);
        assert_eq!(stamp_current.properties,stamp_saved.state.properties);
        for version in 1..=4 {
            let replay = crate::handlers::node_history::read_granted(
                &db,
                &stamp_node,
                0,
                1,
                Some(version),
                &NodeAccessGrant::Local,
                "did:qa",
            )
            .await
            .unwrap();
            if matches!(version, 2 | 3) {
                assert_eq!(replay["availability"], "inside_atomic_batch");
                assert!(
                    replay.get("state").is_none(),
                    "an audit field step was never a separately committed snapshot"
                );
                assert_eq!(
                    replay["atomic_save"],
                    serde_json::json!({"batch_id":"qa-stamp-success","first_version":2,"committed_version":4})
                );
                let typed: fe_runtime::node_history::NodeStateAtVersion =
                    serde_json::from_value(replay).unwrap();
                assert!(typed.state.is_none());
            } else {
                assert_eq!(replay["availability"], "available");
                if version == 4 {
                    assert_eq!(
                        replay["state"],
                        serde_json::to_value(&stamp_saved.state).unwrap()
                    );
                }
            }
        }
        let page = crate::handlers::node_history::read_granted(
            &db,
            &stamp_node,
            2,
            1,
            None,
            &NodeAccessGrant::Local,
            "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(page["entries"][0]["row_version"], 3);
        assert_eq!(page["entries"][0]["atomic_save"]["first_version"], 2);
        assert_eq!(
            page["entries"][0]["atomic_save"]["committed_version"], 4,
            "a page ending inside a Save still names its completed boundary"
        );
        assert_eq!(page["next_after_version"], 3);
        let typed: fe_runtime::node_history::NodeHistoryPage = serde_json::from_value(page).unwrap();
        assert!(matches!(
            typed.entries[0].replay_availability,
            Some(fe_runtime::node_history::HistoryAvailability::InsideAtomicBatch)
        ));
        let mut second_save = stamp_input.clone();
        second_save.history_version = Some(4);
        second_save.before_properties = serde_json::to_value(&stamp_saved.state.properties).unwrap();
        second_save.arc_m = 6.0;
        let prepared = crate::handlers::stamp_overrides::prepare(
            &db,
            &second_save,
            "qa-stamp-next-save",
            "did:qa",
        )
        .await
        .unwrap();
        let second_saved = crate::handlers::stamp_overrides::commit(&db, prepared)
            .await
            .unwrap();
        assert_eq!(second_saved.history_version, 7);
        crate::handlers::entity_property::set_entity_property_granted(
            &db,
            &stamp_node,
            "qa.after_save",
            &serde_json::json!(true),
            "did:qa",
            &NodeAccessGrant::Local,
        )
        .await
        .unwrap();
        db.query("UPDATE node_log SET hlc_timestamp=42 WHERE node_id=$id")
            .bind(("id", stamp_node.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
        for (version, first, committed) in [(2, 2, 4), (3, 2, 4), (5, 5, 7), (6, 5, 7)] {
            let replay = crate::handlers::node_history::read_granted(
                &db,
                &stamp_node,
                0,
                1,
                Some(version),
                &NodeAccessGrant::Local,
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(replay["availability"], "inside_atomic_batch");
            assert!(replay.get("state").is_none());
            assert_eq!(replay["atomic_save"]["first_version"], first);
            assert_eq!(
                replay["atomic_save"]["committed_version"], committed,
                "adjacent Saves sharing HLC remain separate"
            );
        }
        for version in [1, 4, 7, 8] {
            let replay = crate::handlers::node_history::read_granted(
                &db,
                &stamp_node,
                0,
                1,
                Some(version),
                &NodeAccessGrant::Local,
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(
                replay["availability"], "available",
                "valid pre/post Save and untagged versions stay replayable despite identical HLC"
            );
            if version == 8 {
                assert!(replay.get("atomic_save").is_none());
                assert_eq!(replay["state"]["properties"]["qa.after_save"], true);
            }
        }
        db.query("UPDATE node_log SET payload.editor_request_id='mismatched-receipt' WHERE node_id=$id AND row_version=7").bind(("id",stamp_node.clone())).await.unwrap().check().unwrap();
        for version in [5, 6, 7, 8] {
            let replay = crate::handlers::node_history::read_granted(
                &db,
                &stamp_node,
                0,
                1,
                Some(version),
                &NodeAccessGrant::Local,
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(replay["availability"], "incomplete_atomic_batch");
            assert!(
                replay.get("state").is_none(),
                "mismatched final markers cannot authorize partial or later replay"
            );
        }
        db.query("UPDATE node_log SET payload.editor_request_id='qa-stamp-next-save' WHERE node_id=$id AND row_version=7").bind(("id",stamp_node.clone())).await.unwrap().check().unwrap();
        let baseline_rows: Vec<serde_json::Value> = db
            .query("SELECT * OMIT id FROM node_log WHERE node_id=$id AND row_version=1")
            .bind(("id", stamp_node.clone()))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        db.query("UPDATE node_log SET payload.stamp_batch_id='qa-malformed-baseline', payload.stamp_request_id='qa-malformed-baseline', payload.editor_request_id='qa-malformed-baseline' WHERE node_id=$id AND row_version=1")
                .bind(("id", stamp_node.clone())).await.unwrap().check().unwrap();
        let malformed_page = crate::handlers::node_history::read_granted(
            &db,
            &stamp_node,
            0,
            1,
            None,
            &NodeAccessGrant::Local,
            "did:qa",
        )
        .await
        .unwrap();
        assert_eq!(
            malformed_page["entries"][0]["replay_availability"],
            "incomplete_atomic_batch"
        );
        for version in [1, 4, 7, 8] {
            let replay = crate::handlers::node_history::read_granted(
                &db,
                &stamp_node,
                0,
                1,
                Some(version),
                &NodeAccessGrant::Local,
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(
                replay["availability"], malformed_page["entries"][0]["replay_availability"],
                "exact-baseline and crossing reads must agree with the tagged baseline's page boundary"
            );
            assert!(
                replay.get("state").is_none(),
                "a tagged baseline cannot bypass atomic validation before reconstruction"
            );
        }
        db.query("UPDATE node_log SET payload=$payload WHERE node_id=$id AND row_version=1")
            .bind(("id", stamp_node.clone()))
            .bind(("payload", baseline_rows[0]["payload"].clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
        for version in [1, 8] {
            let replay = crate::handlers::node_history::read_granted(
                &db,
                &stamp_node,
                0,
                1,
                Some(version),
                &NodeAccessGrant::Local,
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(
                replay["availability"], "available",
                "untagged baselines retain exact and crossing replay behavior"
            );
        }
        {
            use crate::handlers::editor_batch;
            use fe_runtime::editor_batch::*;
            let mut ids = Vec::new();
            for name in ["Batch Alpha", "Batch Beta"] {
                ids.push(
                    crate::handlers::node_history::create_object_granted(
                        &db,
                        "tree-other-petal",
                        name,
                        None,
                        [0.0; 3],
                        vec![0.0; 3],
                        vec![1.0; 3],
                        serde_json::from_value(serde_json::json!({"qa.shared":0})).unwrap(),
                        "did:qa",
                        &NodeAccessGrant::Local,
                    )
                    .await
                    .unwrap()
                    .0,
                );
            }
            let changes = vec![
                EditorPropertyEdit::Set {
                    key: "qa.shared".into(),
                    value: serde_json::json!(7),
                },
                EditorPropertyEdit::Set {
                    key: "qa.nullable".into(),
                    value: serde_json::Value::Null,
                },
            ];
            let request_for = |capture: EditorBatchCapture| EditorBatchRequest {
                edit: EditorBatchEdit::Targets(
                    capture
                        .members
                        .iter()
                        .map(|member| EditorTargetEdit {
                            node_id: member.node_id.clone(),
                            mutation: EditorTargetMutation::Properties(changes.clone()),
                        })
                        .collect(),
                ),
                capture,
            };
            let initial = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            assert_eq!(
                initial
                    .members
                    .iter()
                    .map(|m| m.node_id.clone())
                    .collect::<Vec<_>>(),
                ids
            );
            editor_batch::read(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[ids[0].clone(), ids[0].clone()],
                "did:qa",
            )
            .await
            .expect_err("duplicate selection cannot become a prefix edit");
            editor_batch::read(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &vec![ids[0].clone(); 257],
                "did:qa",
            )
            .await
            .expect_err("selection count is bounded before query expansion");
            let request = request_for(initial.clone());
            editor_batch::prepare(&db, &request, "qa-selection-viewer", "did:tree-viewer")
                .await
                .expect_err("Viewer cannot submit a checked selection edit");
            let prepared = editor_batch::prepare(&db, &request, "qa-selection-failure", "did:qa")
                .await
                .unwrap();
            db.query("DEFINE EVENT fail_selection_fact ON TABLE node_log WHEN $after.payload.after_state.display_name='Batch Beta' AND $after.payload.atomic_save_id='qa-selection-failure' THEN { THROW 'induced second selected member failure' }").await.unwrap().check().unwrap();
            let operations = op_log_count(&db).await;
            editor_batch::commit(&db, prepared)
                .await
                .expect_err("second member failure must roll back nodes, facts and manifest");
            db.query("REMOVE EVENT fail_selection_fact ON TABLE node_log")
                .await
                .unwrap()
                .check()
                .unwrap();
            assert_eq!(op_log_count(&db).await, operations);
            let unchanged = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&unchanged).unwrap(),
                serde_json::to_value(&initial).unwrap()
            );
            let prepared = editor_batch::prepare(&db, &request, "qa-selection-properties", "did:qa")
                .await
                .unwrap();
            let before_first_batch = op_log_count(&db).await;
            let saved = editor_batch::commit(&db, prepared).await.unwrap();
            assert_eq!(op_log_count(&db).await, before_first_batch + 1, "the current checked batch manifest is admitted exactly once, not mistaken for a preexisting request");
            assert_eq!(saved.members.len(), 2);
            for member in &saved.members {
                assert_eq!(member.state.properties["qa.shared"], 7);
                assert!(
                    member
                        .state
                        .properties
                        .get("qa.nullable")
                        .unwrap()
                        .is_null()
                );
                assert_eq!(member.history_version, Some(3));
            }
            {
                let capture =
                    editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                        .await
                        .unwrap();
                let edits = capture
                    .members
                    .iter()
                    .map(|member| EditorTargetEdit {
                        node_id: member.node_id.clone(),
                        mutation: EditorTargetMutation::Properties(vec![
                            EditorPropertyEdit::Delete {
                                key: "qa.nullable".into(),
                            },
                            EditorPropertyEdit::Set {
                                key: "qa.second".into(),
                                value: serde_json::json!(8),
                            },
                        ]),
                    })
                    .collect();
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture,
                        edit: EditorBatchEdit::Targets(edits),
                    },
                    "qa-selection-next-properties",
                    "did:qa",
                )
                .await
                .unwrap();
                let next = editor_batch::commit(&db, prepared).await.unwrap();
                assert!(next.members.iter().all(|m| m.history_version == Some(5)));
                for id in &ids {
                    db.query("UPDATE node_log SET hlc_timestamp=42 WHERE node_id=$id")
                        .bind(("id", id.clone()))
                        .await
                        .unwrap()
                        .check()
                        .unwrap();
                    for (version, first, last) in [(2, 2, 3), (4, 4, 5)] {
                        let replay = crate::handlers::node_history::read_granted(
                            &db,
                            id,
                            0,
                            1,
                            Some(version),
                            &NodeAccessGrant::Local,
                            "did:qa",
                        )
                        .await
                        .unwrap();
                        assert_eq!(replay["availability"], "inside_atomic_batch");
                        assert!(replay.get("state").is_none());
                        assert_eq!(replay["atomic_save"]["first_version"], first);
                        assert_eq!(replay["atomic_save"]["committed_version"], last);
                    }
                    for version in [1, 3, 5] {
                        let replay = crate::handlers::node_history::read_granted(
                            &db,
                            id,
                            0,
                            1,
                            Some(version),
                            &NodeAccessGrant::Local,
                            "did:qa",
                        )
                        .await
                        .unwrap();
                        assert_eq!(replay["availability"], "available");
                        if version == 5 {
                            assert!(
                                replay["state"]["properties"].get("qa.nullable").is_none(),
                                "generic save replay includes property deletion"
                            );
                        }
                    }
                    let page = crate::handlers::node_history::read_granted(
                        &db,
                        id,
                        1,
                        1,
                        None,
                        &NodeAccessGrant::Local,
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    assert_eq!(page["entries"][0]["row_version"], 2);
                    assert_eq!(
                        page["entries"][0]["atomic_save"]["committed_version"], 3,
                        "generic saved-together metadata crosses the page boundary"
                    );
                }
                let mut read=db.query("SELECT * OMIT id FROM node_log WHERE node_id=$id AND row_version<=5 ORDER BY row_version").bind(("id",ids[0].clone())).await.unwrap().check().unwrap();
                let originals: Vec<serde_json::Value> = read.take(0).unwrap();
                for (row, payload) in originals.iter().skip(1).map(|row| {
                    let mut payload = row["payload"].clone();
                    if row["row_version"].as_i64().unwrap() <= 3 {
                        let map = payload.as_object_mut().unwrap();
                        map.remove("atomic_save_id");
                        map.remove("atomic_save_request_id");
                        payload["stamp_batch_id"] = serde_json::json!("same-family-id");
                        if row["row_version"] == 3 {
                            payload["stamp_request_id"] = serde_json::json!("same-family-id");
                            payload["editor_request_id"] = serde_json::json!("same-family-id");
                        }
                    } else {
                        payload["atomic_save_id"] = serde_json::json!("same-family-id");
                        if row["row_version"] == 5 {
                            payload["atomic_save_request_id"] = serde_json::json!("same-family-id");
                            payload["editor_request_id"] = serde_json::json!("same-family-id");
                        }
                    }
                    (row, payload)
                }) {
                    db.query("UPDATE node_log SET payload=$payload WHERE node_id=$id AND row_version=$version").bind(("id",ids[0].clone())).bind(("version",row["row_version"].clone())).bind(("payload",payload)).await.unwrap().check().unwrap();
                }
                let replay = crate::handlers::node_history::read_granted(
                    &db,
                    &ids[0],
                    0,
                    1,
                    Some(5),
                    &NodeAccessGrant::Local,
                    "did:qa",
                )
                .await
                .unwrap();
                assert_eq!(replay["availability"], "available");
                assert_eq!(replay["atomic_save"]["first_version"], 4);
                assert_eq!(
                    replay["atomic_save"]["committed_version"], 5,
                    "equal identifiers from distinct tag families cannot share a cached boundary"
                );
                for row in &originals {
                    db.query("UPDATE node_log SET payload=$payload WHERE node_id=$id AND row_version=$version").bind(("id",ids[0].clone())).bind(("version",row["row_version"].clone())).bind(("payload",row["payload"].clone())).await.unwrap().check().unwrap();
                }
                for fault in ["marker", "dual_tag", "baseline", "scope"] {
                    let version = if fault == "baseline" { 1 } else { 5 };
                    let mut payload = originals[(version - 1) as usize]["payload"].clone();
                    match fault {
                        "marker" => {
                            payload["atomic_save_request_id"] = serde_json::json!("wrong-final-marker")
                        }
                        "dual_tag" => {
                            payload["stamp_batch_id"] = serde_json::json!("ambiguous-tag-family")
                        }
                        "scope" => {
                            payload["scope"] =
                                serde_json::json!("VERSE#foreign-FRACTAL#foreign-PETAL#foreign")
                        }
                        _ => {
                            payload["atomic_save_id"] = serde_json::json!("tagged-baseline");
                            payload["atomic_save_request_id"] = serde_json::json!("tagged-baseline");
                            payload["editor_request_id"] = serde_json::json!("tagged-baseline");
                        }
                    }
                    db.query("UPDATE node_log SET payload=$payload WHERE node_id=$id AND row_version=$version").bind(("id",ids[0].clone())).bind(("version",version)).bind(("payload",payload)).await.unwrap().check().unwrap();
                    for target in if fault == "baseline" {
                        vec![1, 3, 5]
                    } else {
                        vec![4, 5]
                    } {
                        let replay = crate::handlers::node_history::read_granted(
                            &db,
                            &ids[0],
                            0,
                            1,
                            Some(target),
                            &NodeAccessGrant::Local,
                            "did:qa",
                        )
                        .await
                        .unwrap();
                        assert_eq!(
                            replay["availability"],
                            if fault == "scope" {
                                "unavailable_scope"
                            } else {
                                "incomplete_atomic_batch"
                            }
                        );
                        assert!(
                            replay.get("state").is_none(),
                            "malformed generic groups/baselines cannot expose an intermediate or crossing state"
                        );
                    }
                    let page = crate::handlers::node_history::read_granted(
                        &db,
                        &ids[0],
                        if fault == "scope" { 3 } else { version - 1 },
                        1,
                        None,
                        &NodeAccessGrant::Local,
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    if fault == "scope" {
                        assert_eq!(page["entries"][0]["op"], "unavailable_interval");
                        assert!(
                            page["entries"][0].get("atomic_save").is_none(),
                            "batch metadata cannot leak through an unreadable member"
                        );
                    } else {
                        assert_eq!(
                            page["entries"][0]["replay_availability"],
                            "incomplete_atomic_batch"
                        );
                    }
                    db.query("UPDATE node_log SET payload=$payload WHERE node_id=$id AND row_version=$version").bind(("id",ids[0].clone())).bind(("version",version)).bind(("payload",originals[(version-1) as usize]["payload"].clone())).await.unwrap().check().unwrap();
                }
                let snapshot = crate::handlers::node_history::snapshot(&db, &ids[0])
                    .await
                    .unwrap()
                    .unwrap();
                for version in 6..=206 {
                    let mut payload = serde_json::json!({"journal_version":1,"scope":plugin_scope,"data":{"key":"qa.shared","value":7},"after_state":snapshot,"atomic_save_id":"qa-overlarge-atomic-group"});
                    if version == 206 {
                        payload["atomic_save_request_id"] =
                            serde_json::json!("qa-overlarge-atomic-group");
                        payload["editor_request_id"] = serde_json::json!("qa-overlarge-atomic-group");
                    }
                    db.query("CREATE node_log CONTENT $row").bind(("row",serde_json::json!({"log_id":format!("qa-overlarge-{version}"),"node_id":ids[0],"hlc_timestamp":42,"op":"property_set","source_did":"did:qa","payload":payload,"row_version":version,"created_at":"fault-fixture"}))).await.unwrap().check().unwrap();
                }
                let page = crate::handlers::node_history::read_granted(
                    &db,
                    &ids[0],
                    5,
                    1,
                    None,
                    &NodeAccessGrant::Local,
                    "did:qa",
                )
                .await
                .unwrap();
                assert_eq!(
                    page["entries"][0]["replay_availability"], "incomplete_atomic_batch",
                    "the 201-row probe refuses groups beyond the 200-fact bound"
                );
                for target in [6, 206] {
                    let replay = crate::handlers::node_history::read_granted(
                        &db,
                        &ids[0],
                        0,
                        1,
                        Some(target),
                        &NodeAccessGrant::Local,
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    assert_eq!(replay["availability"], "incomplete_atomic_batch");
                    assert!(replay.get("state").is_none());
                }
                db.query("DELETE node_log WHERE node_id=$id AND payload.atomic_save_id='qa-overlarge-atomic-group'").bind(("id",ids[0].clone())).await.unwrap().check().unwrap();
            }
            let resolved = editor_batch::resolve(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &ids,
                "qa-selection-properties",
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(
                serde_json::to_value(resolved.receipt.unwrap()).unwrap(),
                serde_json::to_value(&saved).unwrap(),
                "timeout recovery uses the immutable complete manifest"
            );
            editor_batch::resolve(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[ids[1].clone(), ids[0].clone()],
                "qa-selection-properties",
                "did:qa",
            )
            .await
            .expect_err("receipt cannot satisfy another target order");
            let duplicate_capture =
                editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                    .await
                    .unwrap();
            let duplicate_request = EditorBatchRequest {
                capture: duplicate_capture.clone(),
                edit: request.edit.clone(),
            };
            let duplicate =
                editor_batch::prepare(&db, &duplicate_request, "qa-selection-properties", "did:qa")
                    .await
                    .unwrap();
            let before_duplicate = op_log_count(&db).await;
            editor_batch::commit(&db, duplicate)
                .await
                .expect_err("a fresh capture cannot reuse an already committed request identity");
            assert_eq!(
                op_log_count(&db).await,
                before_duplicate,
                "the duplicate intention and all facts roll back"
            );
            let after_duplicate =
                editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                    .await
                    .unwrap();
            assert_eq!(
                serde_json::to_value(after_duplicate).unwrap(),
                serde_json::to_value(duplicate_capture).unwrap(),
                "duplicate refusal leaves the complete current capture unchanged"
            );

            let operations = op_log_count(&db).await;
            editor_batch::prepare(&db, &request, "qa-stale-selection", "did:qa")
                .await
                .expect_err("stale before-state/version refuses every target");
            assert_eq!(op_log_count(&db).await, operations);
            for race in ["role", "position", "registry", "schema", "ancestry"] {
                let capture =
                    editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                        .await
                        .unwrap();
                let request = EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::Targets(
                        capture
                            .members
                            .iter()
                            .map(|member| EditorTargetEdit {
                                node_id: member.node_id.clone(),
                                mutation: EditorTargetMutation::Properties(vec![
                                    EditorPropertyEdit::Set {
                                        key: "qa.shared".into(),
                                        value: serde_json::json!(9),
                                    },
                                ]),
                            })
                            .collect(),
                    ),
                };
                let prepared =
                    editor_batch::prepare(&db, &request, &format!("qa-selection-{race}"), "did:qa")
                        .await
                        .unwrap();
                match race {
                    "role" => {
                        db.query("UPDATE verse SET created_by='did:other' WHERE verse_id='tree-10'")
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    "position" => {
                        db.query("UPDATE node SET elevation=77.0 WHERE node_id=$id")
                            .bind(("id", ids[1].clone()))
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    "registry" => {
                        db.query("UPDATE petal SET visibility_groups=[{id:'race',name:'Race',visible:true,order:0.0}] WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();
                    }
                    "schema" => {
                        db.query("CREATE field_def CONTENT {field_def_id:'batch-race-rule',scope:$scope,entity_type:'node',key:'qa.shared',enforced_schema:{version:1,schema:{type:'number'}}}").bind(("scope",plugin_scope.clone())).await.unwrap().check().unwrap();
                    }
                    _ => {
                        db.query("UPDATE node SET petal_id='tree-petal' WHERE node_id=$id")
                            .bind(("id", ids[1].clone()))
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                }
                let operations = op_log_count(&db).await;
                editor_batch::commit(&db, prepared)
                    .await
                    .expect_err("captured batch guard must be checked at transaction use");
                assert_eq!(op_log_count(&db).await, operations);
                assert_eq!(
                    crate::handlers::node_history::snapshot(&db, &ids[0])
                        .await
                        .unwrap()
                        .unwrap()["properties"]["qa.shared"],
                    7,
                    "an unaffected first target cannot leak a subset edit"
                );
                match race {
                    "role" => {
                        db.query("UPDATE verse SET created_by='did:qa' WHERE verse_id='tree-10'")
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    "position" => {
                        db.query("UPDATE node SET elevation=0.0 WHERE node_id=$id")
                            .bind(("id", ids[1].clone()))
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    "registry" => {
                        db.query(
                            "UPDATE petal SET visibility_groups=NONE WHERE petal_id='tree-other-petal'",
                        )
                        .await
                        .unwrap()
                        .check()
                        .unwrap();
                    }
                    "schema" => {
                        db.query("DELETE field_def WHERE field_def_id='batch-race-rule'")
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    _ => {
                        db.query("UPDATE node SET petal_id='tree-other-petal' WHERE node_id=$id")
                            .bind(("id", ids[1].clone()))
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                }
            }
            let group = ulid::Ulid::new().to_string();
            let capture = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            let request = EditorBatchRequest {
                capture,
                edit: EditorBatchEdit::CreateGroup {
                    id: group.clone(),
                    name: "Selected objects".into(),
                },
            };
            let prepared = editor_batch::prepare(&db, &request, "qa-group-create-failure", "did:qa")
                .await
                .unwrap();
            db.query("DEFINE EVENT fail_group_member ON TABLE node_log WHEN $after.payload.after_state.display_name='Batch Beta' AND $after.payload.atomic_save_id='qa-group-create-failure' THEN { THROW 'induced group member failure' }").await.unwrap().check().unwrap();
            let operations = op_log_count(&db).await;
            editor_batch::commit(&db, prepared)
                .await
                .expect_err("Group registry and all memberships commit together");
            db.query("REMOVE EVENT fail_group_member ON TABLE node_log")
                .await
                .unwrap()
                .check()
                .unwrap();
            assert_eq!(op_log_count(&db).await, operations);
            assert!(
                editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                    .await
                    .unwrap()
                    .groups
                    .is_empty()
            );
            let prepared = editor_batch::prepare(&db, &request, "qa-group-create", "did:qa")
                .await
                .unwrap();
            let grouped = editor_batch::commit(&db, prepared).await.unwrap();
            assert_eq!(grouped.created_group_id.as_deref(), Some(group.as_str()));
            for member in &grouped.members {
                assert_eq!(
                    member.state.properties["view.groups"],
                    serde_json::json!([group])
                );
            }
            let partial = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids[..1], "did:qa")
                .await
                .unwrap();
            editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: partial,
                    edit: EditorBatchEdit::DeleteGroup {
                        group_id: group.clone(),
                    },
                },
                "qa-partial-group-inverse",
                "did:qa",
            )
            .await
            .expect_err("Group inverse refuses uncaptured members");
            {
                let mut registry = grouped.groups.clone();
                registry[0].name = "Exact hidden legacy Group".into();
                registry[0].visible = false;
                registry[0].order = -2.5;
                registry.insert(
                    0,
                    fe_runtime::messages::VisibilityGroupDto {
                        id: "left-neighbor".into(),
                        name: "Left".into(),
                        visible: true,
                        order: 9.0,
                    },
                );
                registry.push(fe_runtime::messages::VisibilityGroupDto {
                    id: "right-neighbor".into(),
                    name: "Right".into(),
                    visible: true,
                    order: 0.125,
                });
                db.query("UPDATE petal SET visibility_groups=$groups WHERE petal_id='tree-other-petal'")
                    .bind(("groups", serde_json::json!(registry)))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let target = EditorBatchTarget::Group {
                    group_id: group.clone(),
                };
                let complete = editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &[],
                    &target,
                    "did:qa",
                )
                .await
                .unwrap();
                let mut sorted_ids = ids.clone();
                sorted_ids.sort();
                assert_eq!(complete.members.iter().map(|member| member.node_id.clone()).collect::<Vec<_>>(), sorted_ids, "Group targets derive every persisted member in canonical order without the selected set");
                let mut invalid_membership = complete.members[0].state.properties.clone();
                invalid_membership.insert("view.groups".into(), serde_json::json!([group, group]));
                db.query("UPDATE node SET properties=$properties WHERE node_id=$id")
                    .bind(("id", complete.members[0].node_id.clone()))
                    .bind(("properties", serde_json::json!(invalid_membership)))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                editor_batch::read_target(&db, &plugin_scope, "tree-other-petal", &[], &target, "did:qa").await.expect_err("legacy membership representations that cannot be restored exactly refuse before deletion");
                db.query("UPDATE node SET properties=$properties WHERE node_id=$id")
                    .bind(("id", complete.members[0].node_id.clone()))
                    .bind((
                        "properties",
                        serde_json::json!(complete.members[0].state.properties),
                    ))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let delete_request = EditorBatchRequest {
                    capture: complete.clone(),
                    edit: EditorBatchEdit::DeleteGroup {
                        group_id: group.clone(),
                    },
                };
                let raced = editor_batch::prepare(&db, &delete_request, "qa-group-raced-delete", "did:qa")
                    .await
                    .unwrap();
                let outside = crate::handlers::node_history::create_object_granted(
                    &db,
                    "tree-other-petal",
                    "Raced group member",
                    None,
                    [0.0; 3],
                    vec![0.0; 3],
                    vec![1.0; 3],
                    serde_json::from_value(serde_json::json!({"view.groups":[group]})).unwrap(),
                    "did:qa",
                    &NodeAccessGrant::Local,
                )
                .await
                .unwrap()
                .0;
                let count = op_log_count(&db).await;
                editor_batch::commit(&db, raced)
                    .await
                    .expect_err("new membership after complete capture refuses the entire Group delete");
                assert_eq!(op_log_count(&db).await, count);
                db.query("UPDATE node SET tombstone={source_did:'qa',hlc:1} WHERE node_id=$id")
                    .bind(("id", outside.clone()))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let error = editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &[],
                    &target,
                    "did:qa",
                )
                .await
                .expect_err(
                    "protected deleted-object references cannot disappear from the complete target set",
                );
                assert!(error
                    .to_string()
                    .contains("1 protected deleted-object references"));
                db.query("UPDATE node SET properties={} WHERE node_id=$id; UPDATE node UNSET tombstone WHERE node_id=$id").bind(("id",outside)).await.unwrap().check().unwrap();
                let prepared =
                    editor_batch::prepare(&db, &delete_request, "qa-group-complete-delete", "did:qa")
                        .await
                        .unwrap();
                let deleted = editor_batch::commit(&db, prepared).await.unwrap();
                assert_eq!(
                    deleted.members.len(),
                    ids.len(),
                    "nonowning delete retains every object"
                );
                assert!(!deleted.groups.iter().any(|entry| entry.id == group));
                let resolved = editor_batch::resolve_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &sorted_ids,
                    &target,
                    "qa-group-complete-delete",
                    "did:qa",
                )
                .await
                .unwrap();
                assert_eq!(
                    serde_json::to_value(resolved.receipt.unwrap()).unwrap(),
                    serde_json::to_value(&deleted).unwrap()
                );
                let generic_capture =
                    editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                        .await
                        .unwrap();
                editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: generic_capture,
                        edit: EditorBatchEdit::CreateGroup {
                            id: group.clone(),
                            name: "Client-chosen old ID".into(),
                        },
                    },
                    "qa-generic-group-resurrection",
                    "did:qa",
                )
                .await
                .expect_err("CreateGroup cannot bypass a checked delete's required restore proof");
                let legacy_restore = crate::handlers::visibility_groups::set_petal_visibility_groups_reply(
                    &db,
                    "did:qa",
                    "tree-other-petal".into(),
                    registry.clone(),
                    fe_runtime::messages::CallerAuth::Local,
                    Some("qa-legacy-group-resurrection".into()),
                )
                .await;
                assert!(
                    matches!(
                        legacy_restore,
                        fe_runtime::messages::DbResult::RequestFailed { .. }
                    ),
                    "legacy whole-registry writes cannot bypass the durable proof boundary"
                );
                let proof_target = EditorBatchTarget::DeletedGroup {
                    delete_request_id: "qa-group-complete-delete".into(),
                };
                editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &[],
                    &proof_target,
                    "did:other",
                )
                .await
                .expect_err("a different actor cannot borrow a local Group deletion receipt");
                let inverse = editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &[],
                    &proof_target,
                    "did:qa",
                )
                .await
                .unwrap();
                assert_eq!(
                    inverse.deleted_group.as_ref().unwrap().registry_before,
                    registry
                );
                let restore_request = EditorBatchRequest {
                    capture: inverse.clone(),
                    edit: EditorBatchEdit::RestoreDeletedGroup {
                        delete_request_id: "qa-group-complete-delete".into(),
                    },
                };
                let mut forged = restore_request.clone();
                forged
                    .capture
                    .deleted_group
                    .as_mut()
                    .unwrap()
                    .registry_before[1]
                    .id = "client-chosen-resurrection".into();
                editor_batch::prepare(&db, &forged, "qa-forged-group-restore", "did:qa")
                    .await
                    .expect_err("client context is never a grant to choose an old Group ID");
                let restore =
                    editor_batch::prepare(&db, &restore_request, "qa-group-complete-restore", "did:qa")
                        .await
                        .unwrap();
                let restored = editor_batch::commit(&db, restore).await.unwrap();
                assert_eq!(
                    restored.groups, registry,
                    "inverse restores exact raw registry position, name, visibility and noninteger order"
                );
                for (old, restored) in complete.members.iter().zip(&restored.members) {
                    assert_eq!(
                        serde_json::to_value(&old.state).unwrap(),
                        serde_json::to_value(&restored.state).unwrap()
                    );
                }
                editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &[],
                    &proof_target,
                    "did:qa",
                )
                .await
                .expect_err("a consumed proof cannot resurrect an ID after some later unrelated deletion");
                db.query("UPDATE petal SET visibility_groups=$groups WHERE petal_id='tree-other-petal'")
                    .bind(("groups", serde_json::json!(grouped.groups)))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
            }
            let capture = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::RestoreMemberships(vec![
                        EditorMembershipValue::Groups(vec![]),
                        EditorMembershipValue::Null,
                    ]),
                },
                "qa-group-membership-inverse",
                "did:qa",
            )
            .await
            .unwrap();
            let inverse = editor_batch::commit(&db, prepared).await.unwrap();
            assert_eq!(
                inverse.members[0].state.properties["view.groups"],
                serde_json::json!([])
            );
            assert!(inverse.members[1]
                .state
                .properties
                .get("view.groups")
                .unwrap()
                .is_null());
            let empty_target = EditorBatchTarget::Group {
                group_id: group.clone(),
            };
            let capture = editor_batch::read_target(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[],
                &empty_target,
                "did:qa",
            )
            .await
            .unwrap();
            assert!(
                capture.members.is_empty(),
                "no phantom targets are invented for an empty Group"
            );
            let mut unaffected_rows = db.query("SELECT node_id, properties FROM node WHERE petal_id='tree-other-petal' ORDER BY node_id").await.unwrap().check().unwrap();
            let unaffected: Vec<serde_json::Value> = unaffected_rows.take(0).unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::DeleteGroup {
                        group_id: group.clone(),
                    },
                },
                "qa-group-delete",
                "did:qa",
            )
            .await
            .unwrap();
            let deleted = editor_batch::commit(&db, prepared).await.unwrap();
            assert!(deleted.groups.is_empty());
            assert_eq!(
                deleted.members.len(),
                0,
                "empty Group receives a real registry-only receipt"
            );
            let empty_resolution = editor_batch::resolve_target(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[],
                &empty_target,
                "qa-group-delete",
                "did:qa",
            )
            .await
            .unwrap();
            assert!(empty_resolution.receipt.unwrap().members.is_empty());
            let empty_proof = EditorBatchTarget::DeletedGroup {
                delete_request_id: "qa-group-delete".into(),
            };
            let empty_restore = editor_batch::read_target(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[],
                &empty_proof,
                "did:qa",
            )
            .await
            .unwrap();
            let restore = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: empty_restore,
                    edit: EditorBatchEdit::RestoreDeletedGroup {
                        delete_request_id: "qa-group-delete".into(),
                    },
                },
                "qa-empty-group-restore",
                "did:qa",
            )
            .await
            .unwrap();
            let second_empty_restore = editor_batch::read_target(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[],
                &empty_proof,
                "did:qa",
            )
            .await
            .unwrap();
            let second_empty_restore = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: second_empty_restore,
                    edit: EditorBatchEdit::RestoreDeletedGroup {
                        delete_request_id: "qa-group-delete".into(),
                    },
                },
                "qa-empty-group-restore-again",
                "did:qa",
            )
            .await
            .unwrap();
            let restored_empty = editor_batch::commit(&db, restore).await.unwrap();
            assert!(restored_empty.members.is_empty());
            let unchanged = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            assert_eq!(
                unchanged.members[0].state.properties["view.groups"],
                serde_json::json!([])
            );
            assert_eq!(
                unchanged.members[1].state.properties["view.groups"],
                serde_json::Value::Null
            );
            db.query("UPDATE petal SET visibility_groups=[] WHERE petal_id='tree-other-petal'")
                .await
                .unwrap()
                .check()
                .unwrap();
            let mut retained_rows = db.query("SELECT node_id, properties FROM node WHERE petal_id='tree-other-petal' ORDER BY node_id").await.unwrap().check().unwrap();
            let retained: Vec<serde_json::Value> = retained_rows.take(0).unwrap();
            assert_eq!(retained, unaffected, "empty Group deletion/restoration preserves every nonmember's absent/null/explicit-empty membership exactly");
            let count = op_log_count(&db).await;
            editor_batch::commit(&db, second_empty_restore).await.expect_err("single-use proof remains consumed even when an outside writer resets the registry to its old empty state");
            assert_eq!(op_log_count(&db).await, count);
            {
                db.query("CREATE petal CONTENT {petal_id:'group-empty-petal',fractal_id:'tree-other-fractal',name:'empty',node_id:'empty',created_at:'2026-11-02',visibility_groups:[{id:'empty-legacy-group',name:'Empty legacy',visible:false,order:2.5}]}").await.unwrap().check().unwrap();
                let empty_scope = crate::build_scope(
                    "tree-10",
                    Some("tree-other-fractal"),
                    Some("group-empty-petal"),
                );
                let target = EditorBatchTarget::Group {
                    group_id: "empty-legacy-group".into(),
                };
                let capture = editor_batch::read_target(
                    &db,
                    &empty_scope,
                    "group-empty-petal",
                    &[],
                    &target,
                    "did:qa",
                )
                .await
                .unwrap();
                assert!(capture.members.is_empty());
                let request = EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::DeleteGroup {
                        group_id: "empty-legacy-group".into(),
                    },
                };
                let prepared = editor_batch::prepare(&db, &request, "qa-zero-node-race", "did:qa")
                    .await
                    .unwrap();
                db.query("UPDATE petal SET view_hidden=true WHERE petal_id='group-empty-petal'")
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let count = op_log_count(&db).await;
                editor_batch::commit(&db, prepared)
                    .await
                    .expect_err("registry-only batches still fence the full actual Petal row");
                assert_eq!(op_log_count(&db).await, count);
                db.query("UPDATE petal UNSET view_hidden WHERE petal_id='group-empty-petal'")
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let prepared = editor_batch::prepare(&db, &request, "qa-zero-node-delete", "did:qa")
                    .await
                    .unwrap();
                let deleted = editor_batch::commit(&db, prepared).await.unwrap();
                assert!(deleted.members.is_empty() && deleted.groups.is_empty());
                let proof = EditorBatchTarget::DeletedGroup {
                    delete_request_id: "qa-zero-node-delete".into(),
                };
                let current = editor_batch::read_target(
                    &db,
                    &empty_scope,
                    "group-empty-petal",
                    &[],
                    &proof,
                    "did:qa",
                )
                .await
                .unwrap();
                let inverse = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: current,
                        edit: EditorBatchEdit::RestoreDeletedGroup {
                            delete_request_id: "qa-zero-node-delete".into(),
                        },
                    },
                    "qa-zero-node-restore",
                    "did:qa",
                )
                .await
                .unwrap();
                let restored = editor_batch::commit(&db, inverse).await.unwrap();
                assert!(restored.members.is_empty());
                assert_eq!(restored.groups, capture.groups);
                db.query("CREATE petal CONTENT {petal_id:'group-overflow-petal',fractal_id:'tree-other-fractal',name:'overflow',node_id:'overflow',created_at:'2026-11-02',visibility_groups:[{id:'overflow-group',name:'Overflow',visible:true,order:0.0}]}").await.unwrap().check().unwrap();
                for index in 0..257 {
                    db.query("CREATE node SET node_id=$id,petal_id='group-overflow-petal',display_name='overflow',position=<geometry<point>> [0.0,0.0],rotation=[0.0,0.0,0.0],scale=[1.0,1.0,1.0],created_at='2026-11-02',properties=$properties").bind(("id",format!("group-overflow-{index}"))).bind(("properties",serde_json::json!({"view.groups":["overflow-group"]}))).await.unwrap().check().unwrap();
                }
                let overflow_scope = crate::build_scope(
                    "tree-10",
                    Some("tree-other-fractal"),
                    Some("group-overflow-petal"),
                );
                let overflow = editor_batch::read_target(
                    &db,
                    &overflow_scope,
                    "group-overflow-petal",
                    &[],
                    &EditorBatchTarget::Group {
                        group_id: "overflow-group".into(),
                    },
                    "did:qa",
                )
                .await
                .expect_err("server-expanded Groups are bounded before a target subset is exposed");
                assert!(overflow
                    .to_string()
                    .contains("more than 256 stored members"));
            }

            let group = ulid::Ulid::new().to_string();
            let capture = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::CreateGroup {
                        id: group.clone(),
                        name: "Exact inverse".into(),
                    },
                },
                "qa-group-exact-create",
                "did:qa",
            )
            .await
            .unwrap();
            let created_exact = editor_batch::commit(&db, prepared).await.unwrap();
            // Preserve the creation proof through a checked Delete and its own exact Undo.
            let deleting = editor_batch::read_target(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[],
                &EditorBatchTarget::Group {
                    group_id: group.clone(),
                },
                "did:qa",
            )
            .await
            .unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: deleting,
                    edit: EditorBatchEdit::DeleteGroup {
                        group_id: group.clone(),
                    },
                },
                "qa-created-group-delete",
                "did:qa",
            )
            .await
            .unwrap();
            editor_batch::commit(&db, prepared).await.unwrap();
            let deleted_target = EditorBatchTarget::DeletedGroup {
                delete_request_id: "qa-created-group-delete".into(),
            };
            let restoring = editor_batch::read_target(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &ids,
                &deleted_target,
                "did:qa",
            )
            .await
            .unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: restoring,
                    edit: EditorBatchEdit::RestoreDeletedGroup {
                        delete_request_id: "qa-created-group-delete".into(),
                    },
                },
                "qa-created-group-undo-delete",
                "did:qa",
            )
            .await
            .unwrap();
            editor_batch::commit(&db, prepared).await.unwrap();
            let capture = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::UndoCreatedGroup {
                        create_request_id: "qa-group-exact-create".into(),
                        group_id: group.clone(),
                        memberships: vec![
                            EditorMembershipValue::Groups(vec![]),
                            EditorMembershipValue::Null,
                        ],
                    },
                },
                "qa-group-exact-undo",
                "did:qa",
            )
            .await
            .unwrap();
            let exact = editor_batch::commit(&db, prepared).await.unwrap();
            assert!(exact.groups.is_empty());
            assert_eq!(
                exact.members[0].state.properties["view.groups"],
                serde_json::json!([])
            );
            assert!(
                exact.members[1]
                    .state
                    .properties
                    .get("view.groups")
                    .unwrap()
                    .is_null(),
                "one created-Group inverse preserves exact original presence"
            );
            {
                let current = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                    .await
                    .unwrap();
                let operations = op_log_count(&db).await;
                for proof in ["forged-creation-undo", "qa-group-exact-create"] {
                    editor_batch::prepare(
                        &db,
                        &EditorBatchRequest {
                            capture: current.clone(),
                            edit: EditorBatchEdit::RedoCreatedGroup {
                                undo_request_id: proof.into(),
                            },
                        },
                        &format!("qa-forged-redo-{proof}"),
                        "did:qa",
                    )
                    .await
                    .expect_err("only a durable matching creation Undo can authorize Redo");
                }
                editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: current.clone(),
                        edit: EditorBatchEdit::CreateGroup {
                            id: group.clone(),
                            name: "Generic resurrection".into(),
                        },
                    },
                    "qa-created-generic-reuse",
                    "did:qa",
                )
                .await
                .expect_err("generic Create cannot bypass a historical creation proof");
                db.query("CREATE role:qa_creation_redo_peer CONTENT {peer_did:'did:other',scope:$scope,role:'editor'}").bind(("scope", plugin_scope.clone())).await.unwrap().check().unwrap();
                let other_current =
                    editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:other")
                        .await
                        .unwrap();
                let cross_actor = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: other_current,
                        edit: EditorBatchEdit::RedoCreatedGroup {
                            undo_request_id: "qa-group-exact-undo".into(),
                        },
                    },
                    "qa-created-cross-actor",
                    "did:other",
                )
                .await
                .expect_err(
                    "an authorized different Editor cannot borrow another actor's creation Undo proof",
                );
                assert!(cross_actor.to_string().contains("same-actor"));
                db.query("DELETE role:qa_creation_redo_peer")
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                assert_eq!(op_log_count(&db).await, operations);
                db.query("UPDATE node SET properties['view.groups']=['unrelated-group'] WHERE node_id=$id")
                    .bind(("id", ids[0].clone()))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let changed_context =
                    editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                        .await
                        .unwrap();
                editor_batch::prepare(&db, &EditorBatchRequest { capture: changed_context, edit: EditorBatchEdit::RedoCreatedGroup { undo_request_id: "qa-group-exact-undo".into() } }, "qa-created-wrong-membership-context", "did:qa").await.expect_err("a fresh capture cannot substitute different membership context into the proven lifecycle");
                db.query("UPDATE node SET properties['view.groups']=[] WHERE node_id=$id")
                    .bind(("id", ids[0].clone()))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let redo = EditorBatchRequest {
                    capture: current,
                    edit: EditorBatchEdit::RedoCreatedGroup {
                        undo_request_id: "qa-group-exact-undo".into(),
                    },
                };
                let prepared = editor_batch::prepare(&db, &redo, "qa-created-source-race", "did:qa")
                    .await
                    .unwrap();
                db.query("UPDATE node SET display_name='Raced creation context' WHERE node_id=$id")
                    .bind(("id", ids[0].clone()))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                editor_batch::commit(&db, prepared)
                    .await
                    .expect_err("a fresh-context race refuses the whole proven creation Redo");
                assert_eq!(op_log_count(&db).await, operations);
                db.query("UPDATE node SET display_name=$name WHERE node_id=$id")
                    .bind(("id", ids[0].clone()))
                    .bind(("name", redo.capture.members[0].state.display_name.clone()))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let prepared = editor_batch::prepare(&db, &redo, "qa-created-redo", "did:qa")
                    .await
                    .unwrap();
                let competing = editor_batch::prepare(&db, &redo, "qa-created-redo-competing", "did:qa")
                    .await
                    .unwrap();
                let redone = editor_batch::commit(&db, prepared).await.unwrap();
                assert_eq!(
                    redone.groups, created_exact.groups,
                    "creation Redo restores the exact saved registry vector"
                );
                assert_eq!(redone.created_group_id.as_deref(), Some(group.as_str()));
                assert_eq!(
                    serde_json::to_value(
                        redone
                            .members
                            .iter()
                            .map(|m| m.state.properties.get("view.groups"))
                            .collect::<Vec<_>>()
                    )
                    .unwrap(),
                    serde_json::to_value(
                        created_exact
                            .members
                            .iter()
                            .map(|m| m.state.properties.get("view.groups"))
                            .collect::<Vec<_>>()
                    )
                    .unwrap()
                );
                editor_batch::commit(&db, competing)
                    .await
                    .expect_err("two prepared Redo requests cannot consume one Undo proof twice");
                let deleting = editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &[],
                    &EditorBatchTarget::Group {
                        group_id: group.clone(),
                    },
                    "did:qa",
                )
                .await
                .unwrap();
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: deleting,
                        edit: EditorBatchEdit::DeleteGroup {
                            group_id: group.clone(),
                        },
                    },
                    "qa-created-redo-delete",
                    "did:qa",
                )
                .await
                .unwrap();
                editor_batch::commit(&db, prepared).await.unwrap();
                let restoring = editor_batch::read_target(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &ids,
                    &EditorBatchTarget::DeletedGroup {
                        delete_request_id: "qa-created-redo-delete".into(),
                    },
                    "did:qa",
                )
                .await
                .unwrap();
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: restoring,
                        edit: EditorBatchEdit::RestoreDeletedGroup {
                            delete_request_id: "qa-created-redo-delete".into(),
                        },
                    },
                    "qa-created-redo-undo-delete",
                    "did:qa",
                )
                .await
                .unwrap();
                editor_batch::commit(&db, prepared).await.unwrap();
                let current = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                    .await
                    .unwrap();
                let restored_memberships = vec![
                    EditorMembershipValue::Groups(vec![]),
                    EditorMembershipValue::Null,
                ];
                let old_undo = EditorBatchRequest {
                    capture: current.clone(),
                    edit: EditorBatchEdit::UndoCreatedGroup {
                        create_request_id: "qa-group-exact-create".into(),
                        group_id: group.clone(),
                        memberships: restored_memberships.clone(),
                    },
                };
                editor_batch::prepare(&db, &old_undo, "qa-created-old-create-proof", "did:qa")
                    .await
                    .expect_err("old creation proof remains consumed after a later legitimate Redo");
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: current,
                        edit: EditorBatchEdit::UndoCreatedGroup {
                            create_request_id: "qa-created-redo".into(),
                            group_id: group.clone(),
                            memberships: restored_memberships,
                        },
                    },
                    "qa-created-new-undo",
                    "did:qa",
                )
                .await
                .unwrap();
                editor_batch::commit(&db, prepared).await.unwrap();
                let current = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                    .await
                    .unwrap();
                editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: current.clone(),
                        edit: EditorBatchEdit::RedoCreatedGroup {
                            undo_request_id: "qa-group-exact-undo".into(),
                        },
                    },
                    "qa-created-consumed-redo",
                    "did:qa",
                )
                .await
                .expect_err("a consumed old Undo proof cannot be reused under a matching reset state");
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: current,
                        edit: EditorBatchEdit::RedoCreatedGroup {
                            undo_request_id: "qa-created-new-undo".into(),
                        },
                    },
                    "qa-created-new-redo",
                    "did:qa",
                )
                .await
                .unwrap();
                editor_batch::commit(&db, prepared).await.unwrap();
            }
            let dangling = crate::handlers::node_history::create_object_granted(
                &db,
                "tree-other-petal",
                "Legacy dangling",
                None,
                [0.0; 3],
                vec![0.0; 3],
                vec![1.0; 3],
                serde_json::from_value(serde_json::json!({"view.groups":["legacy-deleted-id"]}))
                    .unwrap(),
                "did:qa",
                &NodeAccessGrant::Local,
            )
            .await
            .unwrap()
            .0;
            db.query("UPDATE node SET tombstone={source_did:'legacy',hlc:1} WHERE node_id=$id")
                .bind(("id", dangling))
                .await
                .unwrap()
                .check()
                .unwrap();
            let result = crate::handlers::visibility_groups::set_petal_visibility_groups_reply(
                &db,
                "did:qa",
                "tree-other-petal".into(),
                vec![fe_runtime::messages::VisibilityGroupDto {
                    id: "legacy-deleted-id".into(),
                    name: "Reused".into(),
                    visible: true,
                    order: 0.0,
                }],
                fe_runtime::messages::CallerAuth::Local,
                Some("qa-no-resurrection".into()),
            )
            .await;
            assert!(
                matches!(result, fe_runtime::messages::DbResult::RequestFailed { .. }),
                "even tombstoned legacy references cannot be resurrected by an API/legacy registry write"
            );
            let points = serde_json::json!([[0.0,1.0,2.0,123.5,1.0,0.0,0.0,0.0,0.0,1.0,2,0.5,{"opaque":"keep"}],[3.0,2.0,1.0,456.75]]);
            let path = crate::handlers::node_history::create_object_granted(
                &db,
                "tree-other-petal",
                "Batch route",
                None,
                [0.0; 3],
                vec![0.0; 3],
                vec![1.0; 3],
                serde_json::from_value(serde_json::json!({"gpx_points":points})).unwrap(),
                "did:qa",
                &NodeAccessGrant::Local,
            )
            .await
            .unwrap()
            .0;
            let capture = editor_batch::read(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[path.clone()],
                "did:qa",
            )
            .await
            .unwrap();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: path.clone(),
                        mutation: EditorTargetMutation::PathAffine(EditorAffine {
                            pivot: [1.0, 0.0, 1.0],
                            translation: [10.0, 0.0, -3.0],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [2.0, 3.0, 4.0],
                        }),
                    }]),
                },
                "qa-route-affine",
                "did:qa",
            )
            .await
            .unwrap();
            let route = editor_batch::commit(&db, prepared).await.unwrap();
            let transformed = &route.members[0].state.properties["gpx_points"];
            assert_eq!(transformed[0][0], 9.0);
            assert_eq!(transformed[0][1], 3.0);
            assert_eq!(transformed[0][2], 2.0);
            assert_eq!(transformed[0][4], 2.0);
            assert_eq!(
                transformed[0][9], 4.0,
                "relative handles receive no pivot translation"
            );
            for index in [3, 10, 11, 12] {
                assert_eq!(
                    transformed[0][index], points[0][index],
                    "timestamp and trailing metadata remain exact"
                );
            }
            assert_eq!(
                route.members[0].state.position, capture.members[0].state.position,
                "whole-path edits never fake a stored node transform"
            );
            let capture = editor_batch::read(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[path.clone()],
                "did:qa",
            )
            .await
            .unwrap();
            let mut bad = points.clone();
            bad[0][3] = serde_json::json!(999);
            editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: path.clone(),
                        mutation: EditorTargetMutation::PathRestore { points: bad },
                    }]),
                },
                "qa-route-bad-inverse",
                "did:qa",
            )
            .await
            .expect_err("route inverse cannot rewrite timestamps");
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: path.clone(),
                        mutation: EditorTargetMutation::PathRestore {
                            points: points.clone(),
                        },
                    }]),
                },
                "qa-route-inverse",
                "did:qa",
            )
            .await
            .unwrap();
            assert_eq!(
                editor_batch::commit(&db, prepared).await.unwrap().members[0]
                    .state
                    .properties["gpx_points"],
                points
            );
            let before = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            let edits = before
                .members
                .iter()
                .enumerate()
                .map(|(index, member)| EditorTargetEdit {
                    node_id: member.node_id.clone(),
                    mutation: EditorTargetMutation::Transform(
                        fe_runtime::node_history::NodeTransformValue {
                            position: [1.0 + index as f64, 2.0, 3.0],
                            rotation: bevy::math::Quat::from_rotation_y(0.5).to_array(),
                            scale: [2.0, 3.0, 4.0],
                        },
                    ),
                })
                .collect();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: before.clone(),
                    edit: EditorBatchEdit::Targets(edits),
                },
                "qa-selection-transform",
                "did:qa",
            )
            .await
            .unwrap();
            let moved = editor_batch::commit(&db, prepared).await.unwrap();
            for (index, member) in moved.members.iter().enumerate() {
                assert_eq!(member.state.position, [1.0 + index as f64, 2.0, 3.0]);
                assert_eq!(member.state.scale, [2.0, 3.0, 4.0]);
                assert_eq!(
                    member.state.edit_seq,
                    before.members[index].state.edit_seq + 1
                );
                assert_eq!(
                    member.state.properties,
                    before.members[index].state.properties
                );
                let replay = crate::handlers::node_history::read_granted(
                    &db,
                    &member.node_id,
                    0,
                    1,
                    member.history_version,
                    &NodeAccessGrant::Local,
                    "did:qa",
                )
                .await
                .unwrap();
                assert_eq!(
                    replay["availability"], "available",
                    "single-fact generic transform boundaries replay normally"
                );
            }
            let capture = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, "did:qa")
                .await
                .unwrap();
            let edits = before
                .members
                .iter()
                .map(|member| EditorTargetEdit {
                    node_id: member.node_id.clone(),
                    mutation: EditorTargetMutation::Transform(
                        fe_runtime::node_history::NodeTransformValue::from_row(&member.state).unwrap(),
                    ),
                })
                .collect();
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::Targets(edits),
                },
                "qa-selection-transform-inverse",
                "did:qa",
            )
            .await
            .unwrap();
            let restored = editor_batch::commit(&db, prepared).await.unwrap();
            for (actual, old) in restored.members.iter().zip(&before.members) {
                assert!(
                    fe_runtime::node_history::NodeTransformValue::from_row(&actual.state)
                        .unwrap()
                        .matches(
                            &fe_runtime::node_history::NodeTransformValue::from_row(&old.state)
                                .unwrap()
                        )
                );
            }
            let mut promote = promotion.clone();
            promote.instance_index = 0;
            let prepared = crate::handlers::stamp_promotion::prepare(
                &db,
                &promote,
                "qa-batch-inherited-stamp",
                "did:qa",
            )
            .await
            .unwrap();
            let (fresh_stamp, original_properties, _) =
                crate::handlers::stamp_promotion::commit(&db, prepared)
                    .await
                    .unwrap();
            let capture = editor_batch::read(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[fresh_stamp.clone()],
                "did:qa",
            )
            .await
            .unwrap();
            let mut edit = stamp_input.clone();
            edit.node_id = fresh_stamp.clone();
            edit.instance_index = 0;
            edit.history_version = capture.members[0].history_version;
            edit.path_version = capture.members[0]
                .stamp_path
                .as_ref()
                .unwrap()
                .history_version;
            edit.before_properties = serde_json::json!(capture.members[0].state.properties);
            edit.arc_m = 0.0;
            let mut displaced = edit.clone();
            displaced.arc_m = 1.0;
            editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: fresh_stamp.clone(),
                        mutation: EditorTargetMutation::StampOverrides(displaced),
                    }]),
                },
                "qa-stamp-group-position-lock",
                "did:qa",
            )
            .await
            .expect_err("center-mode group edit cannot move a derived stamp origin");
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: fresh_stamp.clone(),
                        mutation: EditorTargetMutation::StampOverrides(edit.clone()),
                    }]),
                },
                "qa-stamp-group-forward",
                "did:qa",
            )
            .await
            .unwrap();
            let explicit = editor_batch::commit(&db, prepared).await.unwrap();
            assert_eq!(
                explicit.members[0].state.position,
                capture.members[0].state.position
            );
            assert!(
                explicit.members[0]
                    .state
                    .properties
                    .contains_key("stamp.override.scale")
            );
            let capture = editor_batch::read(
                &db,
                &plugin_scope,
                "tree-other-petal",
                &[fresh_stamp.clone()],
                "did:qa",
            )
            .await
            .unwrap();
            edit.history_version = capture.members[0].history_version;
            edit.before_properties = serde_json::json!(capture.members[0].state.properties);
            edit.scale = [1.0; 3];
            let missing = EditorStampOverrideSet {
                scale: EditorStampOverrideValue::Absent,
                rotation: EditorStampOverrideValue::Absent,
                arc_m: EditorStampOverrideValue::Absent,
            };
            let mut malformed = missing.clone();
            malformed.scale = EditorStampOverrideValue::Value(serde_json::json!({"bad":"shape"}));
            editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture: capture.clone(),
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: fresh_stamp.clone(),
                        mutation: EditorTargetMutation::StampRestore {
                            edit: edit.clone(),
                            overrides: malformed,
                        },
                    }]),
                },
                "qa-stamp-bad-presence-inverse",
                "did:qa",
            )
            .await
            .expect_err("typed restore cannot bypass override shape rules");
            let prepared = editor_batch::prepare(
                &db,
                &EditorBatchRequest {
                    capture,
                    edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
                        node_id: fresh_stamp.clone(),
                        mutation: EditorTargetMutation::StampRestore {
                            edit: edit.clone(),
                            overrides: missing,
                        },
                    }]),
                },
                "qa-stamp-inherited-inverse",
                "did:qa",
            )
            .await
            .unwrap();
            let inherited = editor_batch::commit(&db, prepared).await.unwrap();
            assert_eq!(
                serde_json::json!(inherited.members[0].state.properties),
                original_properties,
                "stamp Undo restores exact absent override keys without ordinary property bypass"
            );
            {
                let nested = |size: usize| serde_json::json!({"largeBag":{"schema":{"fields":["position","material","measurements"]},"events":[{"sequence":1,"payload":{"text":"x".repeat(size)}}]}});
                let mut budget_ids = Vec::new();
                for index in 0..4 {
                    let id = crate::handlers::node_history::create_object_granted(
                        &db,
                        "tree-other-petal",
                        &format!("Lifecycle boundary {index}"),
                        None,
                        [0.0; 3],
                        vec![0.0; 3],
                        vec![1.0; 3],
                        serde_json::from_value(nested(4096)).unwrap(),
                        "did:qa",
                        &NodeAccessGrant::Local,
                    )
                    .await
                    .unwrap()
                    .0;
                    budget_ids.push(id);
                }
                let operations = op_log_count(&db).await;
                let group = ulid::Ulid::new().to_string();
                let mut accepted = None;
                let mut rejected = false;
                let mut first_rejected_size = None;
                for size in [4096, 8192, 16384, 32768, 49152] {
                    for id in &budget_ids {
                        db.query("UPDATE node SET properties=$properties WHERE node_id=$id")
                            .bind(("id", id.clone()))
                            .bind(("properties", nested(size)))
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    let before = editor_batch::read(
                        &db,
                        &plugin_scope,
                        "tree-other-petal",
                        &budget_ids,
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    let prepared = editor_batch::prepare(
                        &db,
                        &EditorBatchRequest {
                            capture: before.clone(),
                            edit: EditorBatchEdit::CreateGroup {
                                id: group.clone(),
                                name: "Reserved lifecycle".into(),
                            },
                        },
                        "qa-lifecycle-boundary-probe",
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    match prepared.check_admission() {
                        Ok(_) => accepted = Some(size),
                        Err(error) => {
                            assert!(format!("{error:#}").contains("lifecycle reservation"),"the initial transaction fits but its future lifecycle must be reserved: {error:#}");
                            let actual_error = editor_batch::commit(&db, prepared).await.expect_err(
                                "the actual initial Save must reject before the atomic seam too",
                            );
                            assert!(format!("{actual_error:#}").contains("lifecycle reservation"));
                            assert_eq!(
                                op_log_count(&db).await,
                                operations,
                                "no initial manifest or member prefix may persist after lifecycle refusal"
                            );
                            let unchanged = editor_batch::read(
                                &db,
                                &plugin_scope,
                                "tree-other-petal",
                                &budget_ids,
                                "did:qa",
                            )
                            .await
                            .unwrap();
                            assert_eq!(
                                serde_json::to_value(before).unwrap(),
                                serde_json::to_value(unchanged).unwrap()
                            );
                            rejected = true;
                            first_rejected_size = Some(size);
                            break;
                        }
                    }
                }
                let mut admitted_size =
                    accepted.expect("a useful nested property bag must remain admissible");
                assert!(
                    admitted_size >= 8192 && rejected,
                    "exercise both sides of the complete lifecycle boundary"
                );
                let mut ceiling = first_rejected_size.unwrap();
                while ceiling - admitted_size > 1 {
                    let size = admitted_size + (ceiling - admitted_size) / 2;
                    for id in &budget_ids {
                        db.query("UPDATE node SET properties=$properties WHERE node_id=$id")
                            .bind(("id", id.clone()))
                            .bind(("properties", nested(size)))
                            .await
                            .unwrap()
                            .check()
                            .unwrap();
                    }
                    let capture = editor_batch::read(
                        &db,
                        &plugin_scope,
                        "tree-other-petal",
                        &budget_ids,
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    let prepared = editor_batch::prepare(
                        &db,
                        &EditorBatchRequest {
                            capture,
                            edit: EditorBatchEdit::CreateGroup {
                                id: group.clone(),
                                name: "Reserved lifecycle".into(),
                            },
                        },
                        "a",
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    if prepared.check_admission().is_ok() {
                        admitted_size = size;
                    } else {
                        ceiling = size;
                    }
                }
                assert_eq!(
                    ceiling,
                    admitted_size + 1,
                    "reservation tested at the last admissible bag byte"
                );
                for id in &budget_ids {
                    db.query("UPDATE node SET properties=$properties WHERE node_id=$id")
                        .bind(("id", id.clone()))
                        .bind(("properties", nested(admitted_size)))
                        .await
                        .unwrap()
                        .check()
                        .unwrap();
                }
                let before = editor_batch::read(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &budget_ids,
                    "did:qa",
                )
                .await
                .unwrap();
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: before,
                        edit: EditorBatchEdit::CreateGroup {
                            id: group.clone(),
                            name: "Reserved lifecycle".into(),
                        },
                    },
                    "qa-lifecycle-large-create",
                    "did:qa",
                )
                .await
                .unwrap();
                let canonical_bytes = prepared.check_admission().unwrap();
                assert!(
                    MAX_EDITOR_BATCH_BYTES - canonical_bytes < 256,
                    "use a byte-tight admission boundary"
                );
                editor_batch::commit(&db, prepared).await.unwrap();
                let before = editor_batch::read(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &budget_ids,
                    "did:qa",
                )
                .await
                .unwrap();
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: before,
                        edit: EditorBatchEdit::UndoCreatedGroup {
                            create_request_id: "qa-lifecycle-large-create".into(),
                            group_id: group.clone(),
                            memberships: vec![EditorMembershipValue::Absent; budget_ids.len()],
                        },
                    },
                    "qa-lifecycle-large-undo",
                    "did:qa",
                )
                .await
                .unwrap();
                editor_batch::commit(&db, prepared).await.unwrap();
                let before = editor_batch::read(
                    &db,
                    &plugin_scope,
                    "tree-other-petal",
                    &budget_ids,
                    "did:qa",
                )
                .await
                .unwrap();
                let prepared = editor_batch::prepare(
                    &db,
                    &EditorBatchRequest {
                        capture: before,
                        edit: EditorBatchEdit::RedoCreatedGroup {
                            undo_request_id: "qa-lifecycle-large-undo".into(),
                        },
                    },
                    "qa-lifecycle-large-redo",
                    "did:qa",
                )
                .await
                .unwrap();
                editor_batch::commit(&db, prepared).await.unwrap();
                let wide_id = |ordinal: u32| {
                    format!(
                        "{}{}{}",
                        "\0".repeat(126),
                        char::from_u32(1 + ordinal / 7).unwrap(),
                        char::from_u32(1 + ordinal % 7).unwrap()
                    )
                };
                let mut delete_request = "a".to_string();
                let mut final_receipt = None;
                for cycle in 0..5_u32 {
                    let before = editor_batch::read_target(
                        &db,
                        &plugin_scope,
                        "tree-other-petal",
                        &[],
                        &EditorBatchTarget::Group {
                            group_id: group.clone(),
                        },
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    let prepared = editor_batch::prepare(
                        &db,
                        &EditorBatchRequest {
                            capture: before,
                            edit: EditorBatchEdit::DeleteGroup {
                                group_id: group.clone(),
                            },
                        },
                        &delete_request,
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    assert_eq!(prepared.check_admission().unwrap(),canonical_bytes,"tiny and maximal escaped IDs/counter widths must have identical canonical reservations");
                    let deleted = editor_batch::commit(&db, prepared).await.unwrap();
                    if cycle == 4 {
                        final_receipt = Some(deleted);
                        break;
                    }
                    let deleted_ids = deleted
                        .members
                        .iter()
                        .map(|m| m.node_id.clone())
                        .collect::<Vec<_>>();
                    let before = editor_batch::read_target(
                        &db,
                        &plugin_scope,
                        "tree-other-petal",
                        &deleted_ids,
                        &EditorBatchTarget::DeletedGroup {
                            delete_request_id: delete_request.clone(),
                        },
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    let prepared = editor_batch::prepare(
                        &db,
                        &EditorBatchRequest {
                            capture: before,
                            edit: EditorBatchEdit::RestoreDeletedGroup {
                                delete_request_id: delete_request.clone(),
                            },
                        },
                        &wide_id(cycle * 2),
                        "did:qa",
                    )
                    .await
                    .unwrap();
                    editor_batch::commit(&db, prepared).await.unwrap();
                    delete_request = wide_id(cycle * 2 + 1);
                }
                let final_receipt = final_receipt.unwrap();
                assert!(final_receipt
                    .members
                    .iter()
                    .all(|m| serde_json::to_value(&m.state.properties).unwrap() == nested(admitted_size)));
                assert!(
                    final_receipt
                        .members
                        .iter()
                        .all(|m| m.history_version.unwrap() >= 10),
                    "actual owned facts cross a decimal counter-width boundary"
                );
                assert_eq!(
                    op_log_count(&db).await,
                    operations + 12,
                    "Create/Undo/Redo plus five Deletes and four Restores remain whole atomic receipts"
                );
            }
            let long_actor = format!("did:{}", "a".repeat(65532));
            db.query("UPDATE verse SET created_by=$actor WHERE verse_id='tree-10'")
                .bind(("actor", long_actor.clone()))
                .await
                .unwrap()
                .check()
                .unwrap();
            let capture = editor_batch::read(&db, &plugin_scope, "tree-other-petal", &ids, &long_actor)
                .await
                .unwrap();
            let bounded_request = EditorBatchRequest {
                capture: capture.clone(),
                edit: EditorBatchEdit::Targets(
                    capture
                        .members
                        .iter()
                        .map(|member| EditorTargetEdit {
                            node_id: member.node_id.clone(),
                            mutation: EditorTargetMutation::Properties(
                                (0..32)
                                    .map(|index| EditorPropertyEdit::Set {
                                        key: format!("qa.bound.{index}"),
                                        value: serde_json::json!(index),
                                    })
                                    .collect(),
                            ),
                        })
                        .collect(),
                ),
            };
            let prepared = editor_batch::prepare(
                &db,
                &bounded_request,
                "qa-final-envelope-bound",
                &long_actor,
            )
            .await
            .expect("pre-envelope payload fits while repeated actual fact envelopes do not");
            let operations = op_log_count(&db).await;
            let error=editor_batch::commit(&db,prepared).await.expect_err("final manifest and repeated actor/tag/envelope bytes must be bounded before transaction use");
            assert!(format!("{error:#}").contains("4 MiB"));
            assert_eq!(op_log_count(&db).await, operations);
            db.query("UPDATE verse SET created_by='did:qa' WHERE verse_id='tree-10'")
                .await
                .unwrap()
                .check()
                .unwrap();
            for id in &ids {
                assert!(
                    crate::handlers::node_history::snapshot(&db, id)
                        .await
                        .unwrap()
                        .unwrap()["properties"]
                        .get("qa.bound.0")
                        .is_none()
                );
            }
        }
        let before_plugin_ops = op_log_count(&db).await;
        let invalid = vec![
            PendingOp::SetProperty {
                node_id: other_node.clone(),
                key: "calibration".into(),
                value: serde_json::json!({"gain":4}),
            },
            PendingOp::CreateNode {
                petal_id: "tree-other-petal".into(),
                name: "must not exist".into(),
            },
            PendingOp::SetProperty {
                node_id: other_node.clone(),
                key: "calibration".into(),
                value: serde_json::json!({"gain":8}),
            },
        ];
        crate::handlers::plugin_host::commit(&db, &admission, &invalid, "did:qa")
            .await
            .expect_err("an invalid later operation refuses the complete staged batch");
        crate::handlers::plugin_host::commit(&db, &admission, &mixed[..1], "did:tree-viewer")
            .await
            .expect_err("local Viewer cannot write through a plugin");
        let mut denied = mixed[..1].to_vec();
        if let PendingOp::SetProperty { key, .. } = &mut denied[0] {
            *key = "private".into();
        }
        crate::handlers::plugin_host::commit(&db, &admission, &denied, "did:qa")
            .await
            .expect_err("property capability is authoritative in the DB");
        assert_eq!(op_log_count(&db).await, before_plugin_ops);
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &other_node)
                .await
                .unwrap(),
            latest
        );
        let before_nodes: Vec<serde_json::Value> = db
            .query("SELECT node_id FROM node WHERE petal_id='tree-other-petal' ORDER BY node_id")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        db.query("DEFINE EVENT fail_plugin_fact ON TABLE node_log WHEN $after.payload.plugin_id='qa-plugin' THEN {THROW 'induced plugin journal failure'}").await.unwrap().check().unwrap();
        crate::handlers::plugin_host::commit(&db, &admission, &invalid[..2], "did:qa")
            .await
            .expect_err("journal failure rolls materializations and batch intention back together");
        db.query("REMOVE EVENT fail_plugin_fact ON TABLE node_log")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(op_log_count(&db).await, before_plugin_ops);
        assert_eq!(
            crate::handlers::node_history::committed_receipt(&db, &other_node)
                .await
                .unwrap(),
            latest
        );
        let after_nodes: Vec<serde_json::Value> = db
            .query("SELECT node_id FROM node WHERE petal_id='tree-other-petal' ORDER BY node_id")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(after_nodes, before_nodes);
        let target = committed.receipt.created_node_ids[0].clone();
        let orphaning = vec![
            PendingOp::DeleteNode {
                node_id: target.clone(),
            },
            PendingOp::SetProperty {
                node_id: other_node.clone(),
                key: "parent_id".into(),
                value: serde_json::json!(target),
            },
        ];
        let broad = PluginAdmission::new(
            "qa-plugin-broad".into(),
            "tree-other-petal".into(),
            vec![
                fe_sdk::CAP_STORAGE_READ.into(),
                fe_sdk::CAP_STORAGE_WRITE.into(),
            ],
            vec![],
            vec![],
            vec![],
        )
        .unwrap()
        .bind_scope(plugin_scope.clone())
        .unwrap();
        crate::handlers::plugin_host::commit(&db, &broad, &orphaning, "did:qa")
            .await
            .expect_err("staged dependencies cannot bypass the plain-delete contract");
        assert_eq!(op_log_count(&db).await, before_plugin_ops);
        db.query("UPDATE petal SET fractal_id='history-fractal' WHERE petal_id='tree-other-petal'")
            .await
            .unwrap()
            .check()
            .unwrap();
        crate::handlers::plugin_host::commit(&db, &admission, &mixed[..1], "did:qa")
            .await
            .expect_err("queued plugin admission cannot follow a petal reparent");
        crate::handlers::plugin_host::read(
            &db,
            &admission,
            PluginRead::Node(other_node.clone()),
            "did:qa",
        )
        .await
        .expect_err("read admission also remains pinned");
        db.query(
            "UPDATE petal SET fractal_id='tree-other-fractal' WHERE petal_id='tree-other-petal'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let url_db = probe_db().await;
        url_db.query("CREATE verse CONTENT {verse_id:'url-v',name:'v',created_by:'did:qa',default_access:'viewer',created_at:'2026-11-01'}; CREATE fractal CONTENT {fractal_id:'url-f',verse_id:'url-v',owner_did:'did:qa',name:'f',created_at:'2026-11-01'}; CREATE petal CONTENT {petal_id:'url-p',fractal_id:'url-f',name:'p',node_id:'p',created_at:'2026-11-01'}; CREATE model CONTENT {asset_id:'url-a',petal_id:'url-p',transform:{}};")
            .await.unwrap().check().unwrap();
        let (url_node, _) = crate::handlers::node_history::create_object(&url_db, "url-p", "URL model", None, [0.0;3],vec![0.0;3],vec![1.0;3],serde_json::Map::new(),"did:qa").await.unwrap();
        url_db.query("UPDATE node SET asset_id='url-a' WHERE node_id=$node").bind(("node",url_node.clone())).await.unwrap().check().unwrap();
        let url_scope = crate::build_scope("url-v",Some("url-f"),Some("url-p"));
        let initial_ops = op_log_count(&url_db).await;
        let saved = crate::handlers::transform::update_node_url_handler(&url_db,&url_node,Some("https://example.com/confirmed".into()),&url_scope,"did:qa").await.unwrap();
        assert_eq!(saved["external_url"],"https://example.com/confirmed");
        assert_eq!(op_log_count(&url_db).await,initial_ops+1);
        for (scope,actor) in [(url_scope.as_str(),"did:url-viewer"),("VERSE#foreign-FRACTAL#f-PETAL#p","did:qa")] {
            assert!(crate::handlers::transform::update_node_url_handler(&url_db,&url_node,None,scope,actor).await.is_err());
        }
        assert_eq!(op_log_count(&url_db).await,initial_ops+1);
        url_db.query("DEFINE EVENT refuse_url ON TABLE model WHEN $event='UPDATE' THEN { THROW 'induced URL failure'; }").await.unwrap().check().unwrap();
        assert!(crate::handlers::transform::update_node_url_handler(&url_db,&url_node,None,&url_scope,"did:qa").await.is_err());
        assert_eq!(op_log_count(&url_db).await,initial_ops+1,"failed URL transaction leaves no intention");
        let stored:Vec<serde_json::Value> = url_db.query("SELECT external_url FROM model WHERE asset_id='url-a'").await.unwrap().take(0).unwrap();
        assert_eq!(stored[0]["external_url"],"https://example.com/confirmed");
        url_db.query("REMOVE EVENT refuse_url ON TABLE model").await.unwrap().check().unwrap();
        let cleared = crate::handlers::transform::update_node_url_handler(&url_db,&url_node,None,&url_scope,"did:qa").await.unwrap();
        assert!(cleared.get("external_url").is_none_or(serde_json::Value::is_null));
        let (shared_node,_) = crate::handlers::node_history::create_object(&url_db,"url-p","shared",None,[0.0;3],vec![0.0;3],vec![1.0;3],serde_json::Map::new(),"did:qa").await.unwrap();
        url_db.query("UPDATE node SET asset_id='url-a' WHERE node_id=$node").bind(("node",shared_node)).await.unwrap().check().unwrap();
        let shared_ops = op_log_count(&url_db).await;
        assert!(crate::handlers::transform::update_node_url_handler(&url_db,&url_node,Some("https://example.com/unsafe-shared".into()),&url_scope,"did:qa").await.is_err());
        assert_eq!(op_log_count(&url_db).await,shared_ops,"shared model refuses before commit");
        let current_url=crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,Some("url-a"),"did:url-viewer").await.unwrap();
        assert_eq!(current_url.node_id,url_node);
        assert_eq!(current_url.asset_id.as_deref(),Some("url-a"));
        assert_eq!(current_url.scope,url_scope);
        assert!(current_url.url.is_none());
        assert!(crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,Some("wrong-asset"),"did:qa").await.is_err());
        assert!(crate::handlers::transform::read_node_url_handler(&url_db,&url_node,"VERSE#foreign",None,"did:qa").await.is_err());
        url_db.query("UPDATE model SET external_url='javascript:legacy-stored-value' WHERE asset_id='url-a'").await.unwrap().check().unwrap();
        let legacy_url=crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,None,"did:qa").await.unwrap();
        assert_eq!(legacy_url.url.as_deref(),Some("javascript:legacy-stored-value"),"bounded legacy data is returned literally, not silently normalized");
        url_db.query("UPDATE verse SET default_access='none' WHERE verse_id='url-v'").await.unwrap().check().unwrap();
        assert!(crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,None,"did:url-viewer").await.is_err());
        assert!(crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,None,"did:qa").await.is_ok());
        url_db.query("UPDATE model SET external_url=$large WHERE asset_id='url-a'").bind(("large","x".repeat(8193))).await.unwrap().check().unwrap();
        assert!(crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,None,"did:qa").await.is_err(),"over-limit data is refused instead of returning a truncated URL");
        url_db.query("CREATE model CONTENT {asset_id:'url-other',petal_id:'url-p',transform:{},external_url:'https://example.com/current-model'}; UPDATE node SET asset_id='url-other' WHERE node_id=$node")
            .bind(("node",url_node.clone())).await.unwrap().check().unwrap();
        assert!(crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,Some("url-a"),"did:qa").await.is_err());
        let replaced=crate::handlers::transform::read_node_url_handler(&url_db,&url_node,&url_scope,None,"did:qa").await.unwrap();
        assert_eq!(replaced.asset_id.as_deref(),Some("url-other"));
        assert_eq!(replaced.url.as_deref(),Some("https://example.com/current-model"));
        assert_eq!(op_log_count(&url_db).await,shared_ops,"URL reads and refused reads append no operations");
        use crate::handlers::transform::{prepare_node_url_write, read_node_url_handler, update_node_url_handler};
        use fe_runtime::messages::NodeUrlStorage;
        let store: crate::BlobStoreHandle = std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let import_dir = tempfile::tempdir().unwrap();
        let import_path = import_dir.path().join("portal.gltf");
        std::fs::write(&import_path, br#"{"asset":{"version":"2.0"}}"#).unwrap();
        let (imported_node, imported_asset, _, imported_current) = crate::handlers::crud::import_gltf_handler(
            &url_db, &store, "url-p", "Unique imported Portal", import_path.to_str().unwrap(), [2.0,3.0,4.0], "did:qa",
        ).await.unwrap();
        assert_eq!(imported_current.node.node_id,imported_node);
        assert_eq!(imported_current.history_version,1);
        assert_eq!(imported_current.node.position,[2.0,3.0,4.0]);
        let imported_page = crate::handlers::node_history::read_granted(&url_db,&imported_node,0,200,None,&NodeAccessGrant::Local,"did:qa").await.unwrap();
        assert_eq!(imported_page["current_version"],1);
        assert_eq!(imported_page["coverage_start_version"],1);
        assert_eq!(imported_page["current_state_verified"],true);
        assert_eq!(imported_page["entries"].as_array().unwrap().len(),1);
        assert_eq!(imported_page["entries"][0]["op"],"baseline");
        assert_eq!(imported_page["entries"][0]["source_did"],"did:qa");
        assert_eq!(imported_page["entries"][0]["payload"]["scope"],url_scope);
        let imported_replay = crate::handlers::node_history::read_granted(&url_db,&imported_node,0,200,Some(1),&NodeAccessGrant::Local,"did:qa").await.unwrap();
        assert_eq!(imported_replay["availability"],"available");
        assert_eq!(imported_replay["state"],crate::handlers::node_history::snapshot(&url_db,&imported_node).await.unwrap().unwrap());
        assert_eq!(imported_replay["state"]["asset_id"],imported_asset);
        assert_eq!(imported_replay["state"]["position"],serde_json::json!([2.0,3.0,4.0]));
        assert_eq!(imported_replay["state"]["rotation"],serde_json::json!([0.0,0.0,0.0,1.0]));
        assert_eq!(imported_replay["state"]["scale"],serde_json::json!([1.0,1.0,1.0]));
        assert!(crate::handlers::node_history::read_granted(&url_db,&imported_node,0,200,Some(1),&NodeAccessGrant::Scoped("VERSE#foreign".into()),"did:qa").await.is_err());
        let denied_store = std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let denied_handle:crate::BlobStoreHandle = denied_store.clone();
        assert!(crate::handlers::crud::import_gltf_handler(&url_db,&denied_handle,"url-p","Viewer import",import_path.to_str().unwrap(),[0.0;3],"did:url-viewer").await.is_err());
        assert!(denied_store.is_empty(),"Viewer import refuses before blob storage");
        url_db.query("CREATE fractal CONTENT {fractal_id:'import-other-fractal',verse_id:'url-v',owner_did:'did:qa',name:'other',created_at:'2026-11-02'}").await.unwrap().check().unwrap();
        for race in ["role","ancestry","schema","journal"] {
            use crate::handlers::object_creation::{ImportedSceneAsset,ObjectCreation,SceneAsset};
            let mut inventory=url_db.query("SELECT asset_id FROM asset ORDER BY asset_id; SELECT node_id FROM node ORDER BY node_id; SELECT log_id FROM node_log ORDER BY log_id").await.unwrap().check().unwrap();
            let asset_rows:Vec<serde_json::Value>=inventory.take(0).unwrap();
            let node_rows:Vec<serde_json::Value>=inventory.take(1).unwrap();
            let fact_rows:Vec<serde_json::Value>=inventory.take(2).unwrap();
            let ops_before=op_log_count(&url_db).await;
            let asset=ImportedSceneAsset::new("import race","model/gltf+json",&"03".repeat(32),27).unwrap();
            let prepared=crate::handlers::object_creation::prepare(&url_db,ObjectCreation {
                petal_id:"url-p",name:"import race",asset:SceneAsset::Imported(asset),position:[2.0,3.0,4.0],
                rotation:vec![0.0,0.0,0.0,1.0],scale:vec![1.0;3],
                properties:serde_json::json!({"import_marker":"captured"}).as_object().unwrap().clone(),
                actor:"did:qa",grant:&NodeAccessGrant::Local,
            }).await.unwrap();
            match race {
                "role"=>{url_db.query("UPDATE verse SET created_by='did:changed-import-owner' WHERE verse_id='url-v'").await.unwrap().check().unwrap();}
                "ancestry"=>{url_db.query("UPDATE petal SET fractal_id='import-other-fractal' WHERE petal_id='url-p'").await.unwrap().check().unwrap();}
                "schema"=>{url_db.query("CREATE field_def CONTENT {field_def_id:'import-schema-race',scope:$scope,entity_type:'node',key:'import_marker',enforced_schema:{version:1,schema:{type:'string'}}}").bind(("scope",url_scope.clone())).await.unwrap().check().unwrap();}
                _=>{url_db.query("DEFINE EVENT fail_import_baseline ON TABLE node_log WHEN $event='CREATE' AND $after.op='baseline' AND $after.payload.state.display_name='import race' THEN {THROW 'induced import journal failure'}").await.unwrap().check().unwrap();}
            }
            if race == "journal" {
                let failed_store=std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
                let failed_handle:crate::BlobStoreHandle=failed_store.clone();
                crate::handlers::crud::import_gltf_handler(&url_db,&failed_handle,"url-p","import race",import_path.to_str().unwrap(),[2.0,3.0,4.0],"did:qa")
                    .await.expect_err("the actual import pipeline rolls metadata back when its baseline fails");
                assert!(!failed_store.is_empty(),"admitted bytes precede metadata; failed metadata does not erase shared blob content");
            } else {
                crate::handlers::object_creation::commit(&url_db,prepared).await.expect_err("captured import refusal rolls back asset/node/baseline/intention together");
            }
            assert_eq!(op_log_count(&url_db).await,ops_before);
            let mut after=url_db.query("SELECT asset_id FROM asset ORDER BY asset_id; SELECT node_id FROM node ORDER BY node_id; SELECT log_id FROM node_log ORDER BY log_id").await.unwrap().check().unwrap();
            assert_eq!(after.take::<Vec<serde_json::Value>>(0).unwrap(),asset_rows);
            assert_eq!(after.take::<Vec<serde_json::Value>>(1).unwrap(),node_rows);
            assert_eq!(after.take::<Vec<serde_json::Value>>(2).unwrap(),fact_rows);
            match race {
                "role"=>{url_db.query("UPDATE verse SET created_by='did:qa' WHERE verse_id='url-v'").await.unwrap().check().unwrap();}
                "ancestry"=>{url_db.query("UPDATE petal SET fractal_id='url-f' WHERE petal_id='url-p'").await.unwrap().check().unwrap();}
                "schema"=>{url_db.query("DELETE field_def WHERE field_def_id='import-schema-race'").await.unwrap().check().unwrap();}
                _=>{url_db.query("REMOVE EVENT fail_import_baseline ON TABLE node_log").await.unwrap().check().unwrap();}
            }
        }
        let missing = read_node_url_handler(&url_db,&imported_node,&url_scope,None,"did:qa").await.unwrap();
        assert_eq!(missing.storage,NodeUrlStorage::MissingModel,"ordinary imports have no implicit model metadata");
        assert!(missing.url.is_none());
        let mut before = url_db.query("SELECT * OMIT id FROM node WHERE node_id=$node; SELECT * OMIT id FROM asset WHERE asset_id=$asset; SELECT * OMIT id FROM node_log WHERE node_id=$node; SELECT tombstone FROM node WHERE node_id=$node")
            .bind(("node",imported_node.clone())).bind(("asset",imported_asset.clone())).await.unwrap().check().unwrap();
        let before_node: Vec<serde_json::Value> = before.take(0).unwrap();
        let before_asset: Vec<serde_json::Value> = before.take(1).unwrap();
        let before_journal: Vec<serde_json::Value> = before.take(2).unwrap();
        let raw_tombstone: Vec<serde_json::Value> = before.take(3).unwrap();
        let mut binding_probe = url_db.query("RETURN {raw_matches: (SELECT tombstone FROM node WHERE node_id=$node) = $snapshot, none_matches_null: NONE = $null, null_matches_null: NULL = $null, live_none: (NONE = NONE OR NONE = NULL), live_null: (NULL = NONE OR NULL = NULL)}")
            .bind(("node",imported_node.clone())).bind(("snapshot",serde_json::json!(raw_tombstone))).bind(("null",serde_json::Value::Null)).await.unwrap().check().unwrap();
        let probe: serde_json::Value = binding_probe.take(0).unwrap();
        assert_eq!(probe["raw_matches"],false,"raw NONE snapshots rebound as JSON NULL are not an exact CAS");
        assert_eq!(probe["none_matches_null"],false);
        assert_eq!(probe["null_matches_null"],true);
        assert_eq!(probe["live_none"],true);
        assert_eq!(probe["live_null"],true);
        let mut guard_failure = url_db.query("BEGIN TRANSACTION; IF true { THROW 'URL node identity changed'; }; SELECT asset_id FROM model LIMIT 1; COMMIT TRANSACTION;").await.unwrap();
        let diagnostic = crate::handlers::transform::url_read_transaction_error(guard_failure.take_errors(),2).to_string();
        assert!(diagnostic.contains("URL node identity changed"),"the actual guard error must survive canceled transaction echoes");
        let mut projection_failure = url_db.query("BEGIN TRANSACTION; SELECT string::len({private_url:'https://example.com/private-value'}) AS value FROM model LIMIT 1; COMMIT TRANSACTION;").await.unwrap();
        let diagnostic = crate::handlers::transform::url_read_transaction_error(projection_failure.take_errors(),1).to_string();
        assert!(diagnostic.contains("model projection failed"));
        assert!(!diagnostic.contains("https://"),"diagnostics must not emit stored URL values");

        let absent_ops = op_log_count(&url_db).await;
        assert!(update_node_url_handler(&url_db,&imported_node,None,&url_scope,"did:qa").await.is_err(),"clear does not initialize absent metadata");
        assert!(update_node_url_handler(&url_db,&imported_node,Some("https://example.com/viewer".into()),&url_scope,"did:url-viewer").await.is_err());
        assert_eq!(op_log_count(&url_db).await,absent_ops);
        let pending = prepare_node_url_write(&url_db,&imported_node,Some("https://example.com/first".into()),&url_scope,"did:qa").await.unwrap();
        let (competing_node,_,_) = crate::handlers::crud::create_node_with_asset_as(&url_db,"url-p","shared after capture",&imported_asset,[0.0;3],[0.0;3],[1.0;3],"did:qa").await.unwrap();
        let shared_after_capture = op_log_count(&url_db).await;
        assert!(pending.commit(&url_db).await.is_err(),"a new live reference refuses initialization at transaction use");
        assert_eq!(op_log_count(&url_db).await,shared_after_capture);
        assert_eq!(read_node_url_handler(&url_db,&imported_node,&url_scope,None,"did:qa").await.unwrap().storage,NodeUrlStorage::SharedModel);
        url_db.query("DELETE node WHERE node_id=$node").bind(("node",competing_node)).await.unwrap().check().unwrap();
        let pending = prepare_node_url_write(&url_db,&imported_node,Some("https://example.com/first".into()),&url_scope,"did:qa").await.unwrap();
        url_db.query("CREATE fractal CONTENT {fractal_id:'url-foreign-f',verse_id:'tree-foreign',owner_did:'did:foreign',name:'foreign',created_at:'2026-11-02'}; UPDATE petal SET fractal_id='url-foreign-f' WHERE petal_id='url-p'").await.unwrap().check().unwrap();
        assert!(pending.commit(&url_db).await.is_err(),"initialization remains fenced to captured ancestry");
        assert_eq!(op_log_count(&url_db).await,shared_after_capture);
        url_db.query("UPDATE petal SET fractal_id='url-f' WHERE petal_id='url-p'; DEFINE EVENT refuse_url_create ON TABLE model WHEN $event='CREATE' THEN { THROW 'induced URL initialization failure'; }").await.unwrap().check().unwrap();
        assert!(update_node_url_handler(&url_db,&imported_node,Some("https://example.com/first".into()),&url_scope,"did:qa").await.is_err());
        assert_eq!(op_log_count(&url_db).await,shared_after_capture,"failed initialization rolls back its ordinary operation");
        assert_eq!(read_node_url_handler(&url_db,&imported_node,&url_scope,None,"did:qa").await.unwrap().storage,NodeUrlStorage::MissingModel);
        url_db.query("REMOVE EVENT refuse_url_create ON TABLE model").await.unwrap().check().unwrap();
        let initialized = update_node_url_handler(&url_db,&imported_node,Some("https://example.com/first".into()),&url_scope,"did:qa").await.unwrap();
        assert_eq!(initialized["external_url"],"https://example.com/first");
        assert_eq!(initialized["transform"]["position"],before_node[0]["position"]);
        assert_eq!(initialized["transform"]["elevation"],before_node[0]["elevation"]);
        assert_eq!(initialized["transform"]["rotation"],before_node[0]["rotation"]);
        assert_eq!(initialized["transform"]["scale"],before_node[0]["scale"]);
        let ready = read_node_url_handler(&url_db,&imported_node,&url_scope,Some(&imported_asset),"did:qa").await.unwrap();
        assert_eq!(ready.storage,NodeUrlStorage::Ready);
        assert_eq!(ready.url.as_deref(),Some("https://example.com/first"));
        update_node_url_handler(&url_db,&imported_node,None,&url_scope,"did:qa").await.unwrap();
        assert!(read_node_url_handler(&url_db,&imported_node,&url_scope,None,"did:qa").await.unwrap().url.is_none());
        update_node_url_handler(&url_db,&imported_node,Some("https://example.com/second".into()),&url_scope,"did:qa").await.unwrap();
        assert_eq!(read_node_url_handler(&url_db,&imported_node,&url_scope,None,"did:qa").await.unwrap().url.as_deref(),Some("https://example.com/second"));
        let mut after = url_db.query("SELECT * OMIT id FROM node WHERE node_id=$node; SELECT * OMIT id FROM asset WHERE asset_id=$asset; SELECT * OMIT id FROM node_log WHERE node_id=$node; SELECT asset_id FROM model WHERE asset_id=$asset")
            .bind(("node",imported_node)).bind(("asset",imported_asset)).await.unwrap().check().unwrap();
        assert_eq!(after.take::<Vec<serde_json::Value>>(0).unwrap(),before_node,"Portal metadata cannot change physical node state");
        assert_eq!(after.take::<Vec<serde_json::Value>>(1).unwrap(),before_asset,"asset content is retained");
        assert_eq!(after.take::<Vec<serde_json::Value>>(2).unwrap(),before_journal,"object history is retained");
        assert_eq!(after.take::<Vec<serde_json::Value>>(3).unwrap().len(),1,"only the explicit first Save initializes metadata");
        assert_eq!(op_log_count(&url_db).await,shared_after_capture+3,"first Save, clear and update each commit once");


        // Committed full-scene publication, original clocks and durable peer readback.
        {
            use crate::handlers::replicated_row::apply_replicated_row_handler;
            use crate::handlers::{node_history, scene_publication as scene};
            use fe_runtime::messages::{DeferralReason, ReplicatedRowOutcome};
            use serde_json::json;
            let source = probe_db().await;
            let peer = probe_db().await;
            for db in [&source, &peer] {
                crate::schema::apply_all(db).await.unwrap();
                db.query("CREATE verse CONTENT {verse_id:'scene-v',name:'v',created_by:'did:qa',created_at:'2026-11-02'}; CREATE fractal CONTENT {fractal_id:'scene-f',verse_id:'scene-v',owner_did:'did:qa',name:'f',created_at:'2026-11-02'}; CREATE petal CONTENT {petal_id:'scene-p',fractal_id:'scene-f',name:'p',node_id:'p',created_at:'2026-11-02'}")
                                .await.unwrap().check().unwrap();
            }
            let scope = "VERSE#scene-v-FRACTAL#scene-f-PETAL#scene-p";
            let properties = json!({"nested":{"null":null,"measurements":[{"t":1,"value":10.5},null,{"t":2,"value":12}]},"remove_me":{"preserve_before_delete":true}});
            let (id, _) = node_history::create_object(
                &source,
                "scene-p",
                "Complex",
                None,
                [2.0, 3.0, 4.0],
                vec![0.0; 3],
                vec![1.0; 3],
                properties.as_object().unwrap().clone(),
                "did:qa",
            )
            .await
            .unwrap();
            let concrete = std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
            let blobs: crate::BlobStoreHandle = concrete.clone();
            let (tx, rx) = crossbeam::channel::bounded(256);
            let original_fact = node_history::latest(&source, &id).await.unwrap().unwrap();
            scene::prepare(&source, &id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            let event = rx.try_recv().unwrap();
            assert_eq!(event.table, "node");
            assert_eq!(event.verse_id, "scene-v");
            assert_eq!(event.petal_id.as_deref(), Some("scene-p"));
            let first_bytes = concrete.bytes_for(&event.content_hash).unwrap();
            let first: serde_json::Value = serde_json::from_slice(&first_bytes).unwrap();
            assert_eq!(
                first["_fe_scene"]["hlc"],
                original_fact["hlc_timestamp"].as_u64().unwrap().to_string()
            );
            assert_eq!(first["_fe_scene"]["publisher"], "did:qa");
            assert_eq!(first["properties"], properties);
            assert!(first.get("asset_id").is_none());
            assert!(first.get("id").is_none());
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &first_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Applied
            );
            assert_eq!(
                node_history::snapshot(&peer, &id).await.unwrap(),
                node_history::snapshot(&source, &id).await.unwrap()
            );
            assert!(
                node_history::latest(&peer, &id).await.unwrap().is_none(),
                "current-row convergence never invents historical journal convergence"
            );
            let projected = scene::read_current(&peer, &blobs, "scene-v", &id, "did:viewer")
                .await
                .unwrap();
            assert_eq!(projected.scope, scope);
            assert_eq!(
                projected.state.properties,
                properties.as_object().unwrap().clone()
            );
            assert!(matches!(
                projected.asset,
                fe_runtime::scene_replication::ReplicaNodeAsset::NoAsset
            ));
            assert!(
                matches!(projected.provenance,fe_runtime::scene_replication::ReplicaNodeProvenance::OriginalSceneCommit {hlc,..} if hlc==original_fact["hlc_timestamp"].as_u64().unwrap())
            );
            let (projection_tx, projection_rx) = crossbeam::channel::bounded(8);
            scene::echo_applied(
                &peer,
                &blobs,
                "did:viewer",
                &projection_tx,
                vec![crate::handlers::inbound_admissions::RowReport {
                    verse_id: "scene-v".into(),
                    table: "node".into(),
                    record_id: id.clone(),
                    outcome: ReplicatedRowOutcome::Applied,
                }],
            )
            .await;
            assert!(matches!(
                projection_rx.try_recv().unwrap(),
                fe_runtime::messages::DbResult::ReplicatedRowApplied { .. }
            ));
            assert!(
                matches!(projection_rx.try_recv().unwrap(),fe_runtime::messages::DbResult::ReplicatedNodeCurrent {current} if current.state.node_id==id)
            );
            scene::echo_applied(
                &peer,
                &blobs,
                "did:viewer",
                &projection_tx,
                vec![crate::handlers::inbound_admissions::RowReport {
                    verse_id: "scene-v".into(),
                    table: "node".into(),
                    record_id: id.clone(),
                    outcome: ReplicatedRowOutcome::NotApplicable,
                }],
            )
            .await;
            projection_rx.try_recv().unwrap();
            assert!(
                projection_rx.try_recv().is_err(),
                "ignored/stale rows emit no native current projection"
            );

            assert!(
                scene::prepare(&peer, &id, "did:qa").await.is_err(),
                "inbound-only state is not automatically re-signed by another current Editor"
            );
            let mut wrong_author = first.clone();
            wrong_author["_fe_scene"]["publisher"] = json!("did:other");
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &serde_json::to_vec(&wrong_author).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Denied
            );
            let mut malformed = first.clone();
            malformed["_fe_scene"]
                .as_object_mut()
                .unwrap()
                .remove("hlc");
            malformed["_fe_scene"]["other"] = json!("replacement");
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &serde_json::to_vec(&malformed).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Denied
            );
            let mut unrenderable = first.clone();
            unrenderable["scale"] = json!([1e100, 1.0, 1.0]);
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &serde_json::to_vec(&unrenderable).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Denied
            );
            let before_clock = snapshot_hlc();
            let mut future = first.clone();
            future["_fe_scene"]["hlc"] = json!(u64::MAX.to_string());
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &serde_json::to_vec(&future).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Denied
            );
            assert_eq!(
                snapshot_hlc(),
                before_clock,
                "denied clocks do not advance local causality"
            );

            node_history::authorize_scope(
                &source,
                &NodeAccessGrant::Local,
                "did:qa",
                scope,
                fe_policy::RoleLevel::Editor,
            )
            .await
            .unwrap();
            crate::handlers::entity_property::delete_entity_property_as(
                &source,
                &id,
                "remove_me",
                "did:qa",
            )
            .await
            .unwrap();
            crate::handlers::entity_property::set_entity_property_as(
                &source,
                &id,
                "explicit_null",
                &json!(null),
                "did:qa",
            )
            .await
            .unwrap();
            crate::handlers::transform::update_node_transform_as(
                &source,
                &id,
                [9.0, 8.0, 7.0],
                [0.2, 0.4, 0.6],
                [2.0, 3.0, 4.0],
                "did:qa",
            )
            .await
            .unwrap();
            scene::prepare(&source, &id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            let second = rx.try_recv().unwrap();
            let second_bytes = concrete.bytes_for(&second.content_hash).unwrap();
            peer.query("CREATE field_def CONTENT {field_def_id:'scene-known-schema',scope:$scope,entity_type:'node',key:'explicit_null',enforced_schema:{version:1,schema:{type:'string'}}}").bind(("scope",scope)).await.unwrap().check().unwrap();
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &second_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Denied,
                "full snapshots cannot bypass recipient enforced schemas"
            );
            peer.query("DELETE field_def WHERE field_def_id='scene-known-schema'")
                .await
                .unwrap()
                .check()
                .unwrap();
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &second_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Applied
            );
            let current = node_history::snapshot(&peer, &id).await.unwrap().unwrap();
            assert_eq!(current["position"], json!([9.0, 8.0, 7.0]));
            assert_eq!(current["properties"]["nested"], properties["nested"]);
            assert!(
                current["properties"].get("remove_me").is_none(),
                "a complete snapshot removes old nested keys"
            );
            assert_eq!(
                current["properties"].get("explicit_null"),
                Some(&json!(null)),
                "explicit property null is distinct from deletion"
            );
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &first_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::NotApplicable
            );
            assert_eq!(
                node_history::snapshot(&peer, &id).await.unwrap().unwrap(),
                current
            );
            let mut markerless: serde_json::Value = serde_json::from_slice(&first_bytes).unwrap();
            markerless.as_object_mut().unwrap().remove("_fe_scene");
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &serde_json::to_vec(&markerless).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::NotApplicable
            );
            let mut alias = markerless.clone();
            alias["node_id"] = json!(&id);
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    "different-wire-key",
                    &serde_json::to_vec(&alias).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Denied
            );

            // A failed clock write rolls back the full row and cannot advance the process HLC.
            let failed = probe_db().await;
            crate::schema::apply_all(&failed).await.unwrap();
            failed.query("CREATE verse CONTENT {verse_id:'scene-v',name:'v',created_by:'did:qa',created_at:'2026-11-02'}; CREATE fractal CONTENT {fractal_id:'scene-f',verse_id:'scene-v',owner_did:'did:qa',name:'f',created_at:'2026-11-02'}; CREATE petal CONTENT {petal_id:'scene-p',fractal_id:'scene-f',name:'p',node_id:'p',created_at:'2026-11-02'}; DEFINE EVENT fail_scene_clock ON TABLE node_scene_clock WHEN $event='CREATE' THEN {THROW 'induced scene clock failure'}")
                            .await.unwrap().check().unwrap();
            let mut advancing: serde_json::Value = serde_json::from_slice(&second_bytes).unwrap();
            let advanced = next_hlc_timestamp().0 + 1;
            advancing["_fe_scene"]["hlc"] = json!(advanced.to_string());
            let before_clock = snapshot_hlc();
            assert!(apply_replicated_row_handler(
                &failed,
                "scene-v",
                "node",
                &id,
                &serde_json::to_vec(&advancing).unwrap(),
                "did:qa",
                None
            )
            .await
            .is_err());
            assert!(node_history::snapshot(&failed, &id)
                .await
                .unwrap()
                .is_none());
            assert!(scene::read_clock(&failed, &id).await.unwrap().is_empty());
            assert_eq!(snapshot_hlc(), before_clock);
            failed
                .query("REMOVE EVENT fail_scene_clock ON TABLE node_scene_clock")
                .await
                .unwrap()
                .check()
                .unwrap();
            assert_eq!(
                apply_replicated_row_handler(
                    &failed,
                    "scene-v",
                    "node",
                    &id,
                    &serde_json::to_vec(&advancing).unwrap(),
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Applied
            );
            assert!(
                next_hlc_timestamp().0 > advanced,
                "a local commit follows the received original HLC"
            );
            assert_eq!(scene::persisted_hlc(&failed).await.unwrap(), advanced);
            let retained_scene_clock = scene::persisted_hlc(&failed).await.unwrap();
            init_hlc(retained_scene_clock);
            assert!(
                next_hlc_timestamp().0 > advanced,
                "startup includes the persisted accepted scene watermark"
            );

            // A same-value local Save changes journal ordering without changing row or scene clock.
            for absent in [true, false] {
                let before_row = node_history::snapshot(&failed, &id).await.unwrap();
                let before_clock = scene::read_clock(&failed, &id).await.unwrap();
                let mut guards = Vec::new();
                let mut bindings = Vec::new();
                let captured =
                    scene::capture_ordering_fact(&failed, &id, &mut guards, &mut bindings)
                        .await
                        .unwrap();
                assert_eq!(
                    captured.is_none(),
                    absent,
                    "both absent and present latest facts are fenced"
                );
                let mut unchanged = failed
                    .query(format!("BEGIN TRANSACTION; {}; COMMIT TRANSACTION;", guards.join(";\n")))
                    .bind(("scene_id", id.clone()));
                for binding in bindings.clone() {
                    unchanged = unchanged.bind(binding);
                }
                unchanged.await.unwrap().check()
                    .expect("unchanged latest-fact/absence guard is valid SQL and matches");
                crate::handlers::entity_property::set_entity_property_as(
                    &failed,
                    &id,
                    "nested",
                    &before_row.as_ref().unwrap()["properties"]["nested"],
                    "did:qa",
                )
                .await
                .unwrap();
                assert_eq!(
                    node_history::snapshot(&failed, &id).await.unwrap(),
                    before_row
                );
                assert_eq!(scene::read_clock(&failed, &id).await.unwrap(), before_clock);
                let local_fact = node_history::latest(&failed, &id).await.unwrap().unwrap();
                assert!(local_fact["hlc_timestamp"].as_u64().unwrap() > advanced);
                if let Some(captured) = captured {
                    assert!(
                        local_fact["row_version"].as_i64().unwrap()
                            > captured["row_version"].as_i64().unwrap()
                    );
                }
                let local_hlc = snapshot_hlc();
                guards.extend([
                    "UPDATE node SET name='stale incoming write' WHERE node_id=$scene_id".into(),
                    "UPDATE node_scene_clock SET publisher='stale incoming write' WHERE node_id=$scene_id".into(),
                ]);
                let mut transaction = failed
                    .query(format!(
                        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
                        guards.join(";\n")
                    ))
                    .bind(("scene_id", id.clone()));
                for binding in bindings {
                    transaction = transaction.bind(binding);
                }
                let result = transaction.await.unwrap().check();
                assert!(
                    result.is_err(),
                    "the production ordering guard refuses stale latest-fact/absence capture"
                );
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("local scene ordering fact changed"));
                assert_eq!(
                    node_history::snapshot(&failed, &id).await.unwrap(),
                    before_row
                );
                assert_eq!(scene::read_clock(&failed, &id).await.unwrap(), before_clock);
                assert_eq!(
                    snapshot_hlc(),
                    local_hlc,
                    "rejected receive never observes its clock"
                );
            }

            // Missing parents stay in the ordinary admission queue's Deferred vocabulary.
            let missing = probe_db().await;
            crate::schema::apply_all(&missing).await.unwrap();
            missing.query("CREATE verse CONTENT {verse_id:'scene-v',name:'v',created_by:'did:qa',created_at:'2026-11-02'}").await.unwrap().check().unwrap();
            assert_eq!(
                apply_replicated_row_handler(
                    &missing,
                    "scene-v",
                    "node",
                    &id,
                    &first_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Deferred(DeferralReason::Parent {
                    table: "petal".into(),
                    id: "scene-p".into()
                })
            );

            // Preparation and use-time publication both retain exact current authority and row fences.
            let prepared = scene::prepare(&source, &id, "did:qa").await.unwrap();
            source
                .query("UPDATE verse SET created_by='did:other' WHERE verse_id='scene-v'")
                .await
                .unwrap()
                .check()
                .unwrap();
            assert!(prepared.emit(&source, &blobs, &tx).await.is_err());
            assert!(rx.try_recv().is_err());
            source
                .query("UPDATE verse SET created_by='did:qa' WHERE verse_id='scene-v'")
                .await
                .unwrap()
                .check()
                .unwrap();
            assert!(scene::prepare(&source, &id, "did:viewer").await.is_err());
            let prepared = scene::prepare(&source, &id, "did:qa").await.unwrap();
            source
                .query("UPDATE node SET properties.uncovered=1 WHERE node_id=$id")
                .bind(("id", id.clone()))
                .await
                .unwrap()
                .check()
                .unwrap();
            assert!(prepared.emit(&source, &blobs, &tx).await.is_err());
            assert!(rx.try_recv().is_err());
            assert!(
                scene::prepare(&source, &id, "did:qa").await.is_err(),
                "uncovered local/current drift is not re-signed"
            );
            source.query("UPDATE node SET properties=object::remove(properties,'uncovered') WHERE node_id=$id").bind(("id",id.clone())).await.unwrap().check().unwrap();
            let prepared = scene::prepare(&source, &id, "did:qa").await.unwrap();
            let (full_tx, full_rx) = crossbeam::channel::bounded(1);
            full_tx.try_send(second.clone()).unwrap();
            assert!(prepared.emit(&source, &blobs, &full_tx).await.is_err());
            full_rx.try_recv().unwrap();
            scene::prepare(&source, &id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &full_tx)
                .await
                .unwrap();
            let retry = full_rx.try_recv().unwrap();
            assert_eq!(
                retry.content_hash, second.content_hash,
                "queue failure retries the same original state/clock"
            );

            // Distinct authenticated API actors remain local commits, not foreign journal ingress.
            let (delegated, _) = node_history::create_object_granted(
                &source,
                "scene-p",
                "Delegated",
                None,
                [0.0; 3],
                vec![0.0; 3],
                vec![1.0; 3],
                serde_json::Map::new(),
                "did:client",
                &NodeAccessGrant::Scoped(scope.into()),
            )
            .await
            .unwrap();
            assert_eq!(
                node_history::latest(&source, &delegated)
                    .await
                    .unwrap()
                    .unwrap()["source_did"],
                "did:client"
            );
            scene::prepare(&source, &delegated, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            rx.try_recv().unwrap();

            // Local tombstones retain their original operation HLC and dominate stale live rows.
            crate::handlers::crud::tombstone_node_handler(&source, &id, "did:qa")
                .await
                .unwrap();
            scene::prepare(&source, &id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            let event = rx.try_recv().unwrap();
            let deleted = concrete.bytes_for(&event.content_hash).unwrap();
            let deleted_value: serde_json::Value = serde_json::from_slice(&deleted).unwrap();
            assert_eq!(
                deleted_value["_fe_scene"]["hlc"],
                deleted_value["tombstone"]["hlc"]
                    .as_u64()
                    .unwrap()
                    .to_string()
            );
            assert_eq!(
                apply_replicated_row_handler(&peer, "scene-v", "node", &id, &deleted, "did:qa", None)
                    .await
                    .unwrap(),
                ReplicatedRowOutcome::AppliedTombstone
            );
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &id,
                    &second_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::SkippedTombstoned
            );
            let report = scene::recover(&source, &blobs, Some(&tx), "did:qa", "scene-v")
                .await
                .unwrap();
            assert_eq!(report.examined, 2);
            assert_eq!(report.enqueued, 2);
            assert_eq!(report.deferred, 0);
            assert_eq!(rx.try_iter().count(), 2);

            // A real local API actor is distinct from the authenticated publisher in equal-HLC ties.
            let (tie_id, _) = node_history::create_object_granted(
                &source,
                "scene-p",
                "Tie",
                None,
                [0.0; 3],
                vec![0.0; 3],
                vec![1.0; 3],
                serde_json::Map::new(),
                "did:zzclient",
                &NodeAccessGrant::Scoped(scope.into()),
            )
            .await
            .unwrap();
            scene::prepare(&source, &tie_id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            let local = rx.try_recv().unwrap();
            let mut winner: serde_json::Value =
                serde_json::from_slice(&concrete.bytes_for(&local.content_hash).unwrap()).unwrap();
            winner["_fe_scene"]["publisher"] = json!("did:ra");
            winner["properties"]["peer_winner"] = json!(true);
            source
                .query("CREATE role CONTENT {peer_did:'did:ra',scope:$scope,role:'editor'}")
                .bind(("scope", scope))
                .await
                .unwrap()
                .check()
                .unwrap();
            let mut admissions =
                crate::handlers::inbound_admissions::InboundAdmissions::for_local_publisher("did:qa");
            let reports = admissions
                .apply(
                    &source,
                    "scene-v",
                    "node",
                    &tie_id,
                    &serde_json::to_vec(&winner).unwrap(),
                    "did:ra",
                    None,
                )
                .await;
            assert_eq!(
                reports[0].outcome,
                ReplicatedRowOutcome::Applied,
                "tiebreak uses actual publisher, never delegated journal actor"
            );
            assert!(
                scene::prepare(&source, &tie_id, "did:qa").await.is_err(),
                "automatic recovery does not re-sign changed foreign state"
            );
            crate::handlers::entity_property::set_entity_property_as(
                &source,
                &tie_id,
                "peer_winner",
                &json!(false),
                "did:qa",
            )
            .await
            .unwrap();
            let edited = node_history::latest(&source, &tie_id)
                .await
                .unwrap()
                .unwrap();
            assert!(
                edited["hlc_timestamp"].as_u64().unwrap()
                    > winner["_fe_scene"]["hlc"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap()
            );
            scene::prepare(&source, &tie_id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            rx.try_recv().unwrap();

            // Imported metadata is anchored/ordered, while missing bytes stay explicitly unavailable.
            use crate::handlers::object_creation::{ImportedSceneAsset, ObjectCreation, SceneAsset};
            use fe_runtime::scene_replication::{ReplicaAssetUnavailable, ReplicaNodeAsset};
            let asset_bytes = b"admitted scene content fixture";
            let hash = blobs.add_blob(asset_bytes).unwrap();
            let hash = crate::hash_to_hex(&hash);
            let asset = ImportedSceneAsset::new(
                "scene fixture",
                "model/gltf-binary",
                &hash,
                asset_bytes.len(),
            )
            .unwrap();
            let asset_id = asset.id().to_owned();
            let prepared = crate::handlers::object_creation::prepare(
                &source,
                ObjectCreation {
                    petal_id: "scene-p",
                    name: "Imported",
                    asset: SceneAsset::Imported(asset),
                    position: [1.0, 2.0, 3.0],
                    rotation: vec![0.0; 3],
                    scale: vec![1.0; 3],
                    properties: serde_json::Map::new(),
                    actor: "did:qa",
                    grant: &NodeAccessGrant::Local,
                },
            )
            .await
            .unwrap();
            let imported = crate::handlers::object_creation::commit(&source, prepared)
                .await
                .unwrap();
            scene::prepare(&source, &imported.node.node_id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            let metadata = rx.try_recv().unwrap();
            let node = rx.try_recv().unwrap();
            assert_eq!(metadata.table, "asset");
            assert_eq!(node.table, "node");
            assert_eq!(metadata.record_id, asset_id);
            let metadata_bytes = concrete.bytes_for(&metadata.content_hash).unwrap();
            let node_bytes = concrete.bytes_for(&node.content_hash).unwrap();
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "node",
                    &imported.node.node_id,
                    &node_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Applied
            );
            let peer_concrete = std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
            let peer_blobs: crate::BlobStoreHandle = peer_concrete.clone();
            let absent = scene::read_current(
                &peer,
                &peer_blobs,
                "scene-v",
                &imported.node.node_id,
                "did:viewer",
            )
            .await
            .unwrap();
            assert!(matches!(
                absent.asset,
                ReplicaNodeAsset::Unavailable {
                    reason: ReplicaAssetUnavailable::MissingMetadata,
                    ..
                }
            ));
            assert_eq!(
                apply_replicated_row_handler(
                    &peer,
                    "scene-v",
                    "asset",
                    &asset_id,
                    &metadata_bytes,
                    "did:qa",
                    None
                )
                .await
                .unwrap(),
                ReplicatedRowOutcome::Applied
            );
            let absent = scene::read_current(
                &peer,
                &peer_blobs,
                "scene-v",
                &imported.node.node_id,
                "did:viewer",
            )
            .await
            .unwrap();
            assert!(matches!(
                absent.asset,
                ReplicaNodeAsset::Unavailable {
                    reason: ReplicaAssetUnavailable::MissingBlob,
                    ..
                }
            ));
            peer_blobs.add_blob(asset_bytes).unwrap();
            scene::echo_applied(
                &peer,
                &peer_blobs,
                "did:viewer",
                &projection_tx,
                vec![crate::handlers::inbound_admissions::RowReport {
                    verse_id: "scene-v".into(),
                    table: "asset".into(),
                    record_id: asset_id.clone(),
                    outcome: ReplicatedRowOutcome::Applied,
                }],
            )
            .await;
            projection_rx.try_recv().unwrap();
            let fe_runtime::messages::DbResult::ReplicatedNodeCurrent { current } =
                projection_rx.try_recv().unwrap()
            else {
                panic!("asset arrival produces a fresh affected-node current read")
            };
            assert_eq!(current.state.asset_id.as_deref(), Some(asset_id.as_str()));
            assert_eq!(current.asset.asset_id(), current.state.asset_id.as_deref());
            assert!(
                matches!(&current.asset,ReplicaNodeAsset::LocalBlob {asset_path,..} if asset_path==&format!("blob://{hash}.glb"))
            );
            assert_eq!(
                current.provenance, absent.provenance,
                "asset availability can refresh at the same original node version"
            );
            assert!(projection_rx.try_recv().is_err());
            peer.query("UPDATE asset SET petal_id='foreign' WHERE asset_id=$id")
                .bind(("id", asset_id.clone()))
                .await
                .unwrap()
                .check()
                .unwrap();
            let unavailable = scene::read_current(
                &peer,
                &peer_blobs,
                "scene-v",
                &imported.node.node_id,
                "did:viewer",
            )
            .await
            .unwrap();
            assert!(matches!(
                unavailable.asset,
                ReplicaNodeAsset::Unavailable {
                    reason: ReplicaAssetUnavailable::ForeignAnchor,
                    ..
                }
            ));

            // Local rename and duplicate retain complete ordinary provenance and exact source fields.
            crate::handlers::crud::rename_node_handler(
                &source,
                &delegated,
                "Renamed",
                "did:qa",
                "did:client",
            )
            .await
            .unwrap();
            scene::prepare(&source, &delegated, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            rx.try_recv().unwrap();
            assert_eq!(
                node_history::latest(&source, &delegated)
                    .await
                    .unwrap()
                    .unwrap()["op"],
                "node_renamed"
            );
            source.query("UPDATE node SET position=<geometry<point>> [1.000000000123,2.000000000321],interactive=false WHERE node_id=$id").bind(("id",imported.node.node_id.clone())).await.unwrap().check().unwrap();
            let original = node_history::snapshot(&source, &imported.node.node_id)
                .await
                .unwrap()
                .unwrap();
            let expected_copy_position = json!([
                original["position"][0].as_f64().unwrap() + 1.0,
                original["position"][1].as_f64().unwrap(),
                original["position"][2].as_f64().unwrap() + 1.0
            ]);
            let copy = crate::handlers::crud::duplicate_node_handler(
                &source,
                &imported.node.node_id,
                "did:qa",
                "did:client",
            )
            .await
            .unwrap();
            let copied = node_history::snapshot(&source, &copy.node_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(copied["position"], expected_copy_position);
            assert_eq!(copied["interactive"], false);
            assert_eq!(copied["asset_id"], asset_id);
            scene::prepare(&source, &copy.node_id, "did:qa")
                .await
                .unwrap()
                .emit(&source, &blobs, &tx)
                .await
                .unwrap();
            assert_eq!(rx.try_iter().count(), 2);

            // The 1025th candidate refuses before any node prefix is emitted.
            let oversized = probe_db().await;
            crate::schema::apply_all(&oversized).await.unwrap();
            oversized.query("CREATE verse CONTENT {verse_id:'scene-v',name:'v',created_by:'did:qa',created_at:'2026-11-02'}; CREATE fractal CONTENT {fractal_id:'scene-f',verse_id:'scene-v',owner_did:'did:qa',name:'f',created_at:'2026-11-02'}; CREATE petal CONTENT {petal_id:'scene-p',fractal_id:'scene-f',name:'p',node_id:'p',created_at:'2026-11-02'}").await.unwrap().check().unwrap();
            let statements=(0..1025).map(|index|format!("CREATE node SET node_id='limit-{index}',petal_id='scene-p',position=<geometry<point>> [0.0,0.0],rotation=[0.0,0.0,0.0],scale=[1.0,1.0,1.0],created_at='2026-11-02';")).collect::<String>();
            oversized.query(statements).await.unwrap().check().unwrap();
            assert!(
                scene::recover(&oversized, &blobs, Some(&tx), "did:qa", "scene-v")
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("1024")
            );
            assert!(rx.try_recv().is_err());
            // The byte cap is checked across the whole eligible recovery plan before enqueue.
            oversized
                .query("DELETE node")
                .await
                .unwrap()
                .check()
                .unwrap();
            for index in 0..145 {
                node_history::create_object(
                    &oversized,
                    "scene-p",
                    &format!("large-{index}"),
                    None,
                    [0.0; 3],
                    vec![0.0; 3],
                    vec![1.0; 3],
                    json!({"payload":"x".repeat(60_000)})
                        .as_object()
                        .unwrap()
                        .clone(),
                    "did:qa",
                )
                .await
                .unwrap();
            }
            assert!(
                scene::recover(&oversized, &blobs, Some(&tx), "did:qa", "scene-v")
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("8 MiB")
            );
            assert!(
                rx.try_recv().is_err(),
                "oversized plans never enqueue a misleading prefix"
            );
        }

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
