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
        let stamp_bounds=serde_json::json!({"type":"Polygon","coordinates":[[[-10.0,-10.0],[10.0,-10.0],[10.0,10.0],[-10.0,10.0],[-10.0,-10.0]]]});
        db.query("UPDATE petal SET terrain=NONE, description=NONE, hexon_manifest=NONE, visibility_groups=NONE, view_hidden=NONE, render_distance=NONE, bounds=<geometry<polygon>> $bounds WHERE petal_id='tree-other-petal'")
            .bind(("bounds",stamp_bounds.clone())).await.unwrap().check().unwrap();
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
        for race in ["role","path","descriptor","points","terrain","bounds","metadata","collision","schema","journal"] {
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
                _=>{db.query("DEFINE EVENT fail_checked_promotion ON TABLE node WHEN $after.properties.node_kind='stamp' THEN { THROW 'induced promotion row failure' }").await.unwrap().check().unwrap();}
            }
            let before_ops=op_log_count(&db).await;
            let refused=crate::handlers::stamp_promotion::commit(&db,prepared).await.expect_err("checked GUI promotion race must roll back");
            if matches!(race,"terrain"|"bounds"|"metadata") {
                assert!(format!("{refused:#}").contains("stamp map changed during promotion"),"the real map fence must refuse {race}: {refused:#}");
            }
            assert_eq!(op_log_count(&db).await,before_ops);
            let rows:Vec<serde_json::Value>=db.query("SELECT node_id FROM node WHERE node_id=$id").bind(("id",promotion_node.clone())).await.unwrap().take(0).unwrap();
            assert_eq!(rows.len(),usize::from(race=="collision"),"refusal cannot leak a promoted node");
            match race {
                "role"=>{db.query("UPDATE verse SET created_by='did:qa' WHERE verse_id='tree-10'").await.unwrap().check().unwrap();}
                "path"=>{db.query("UPDATE node SET petal_id='tree-other-petal' WHERE node_id=$id").bind(("id",undo_node.clone())).await.unwrap().check().unwrap();}
                "descriptor"|"points"=>{db.query("UPDATE node SET properties=$properties WHERE node_id=$id").bind(("id",undo_node.clone())).bind(("properties",promotion_source.clone())).await.unwrap().check().unwrap();}
                "terrain"=>{db.query("UPDATE petal SET terrain=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "bounds"=>{db.query("UPDATE petal SET bounds=<geometry<polygon>> $bounds WHERE petal_id='tree-other-petal'").bind(("bounds",stamp_bounds.clone())).await.unwrap().check().unwrap();}
                "metadata"=>{db.query("UPDATE petal SET view_hidden=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "collision"=>{db.query("DELETE node WHERE node_id=$id").bind(("id",promotion_node.clone())).await.unwrap().check().unwrap();}
                "schema"=>{db.query("DELETE field_def WHERE field_def_id='qa-promotion-schema'").await.unwrap().check().unwrap();}
                _=>{db.query("REMOVE EVENT fail_checked_promotion ON TABLE node").await.unwrap().check().unwrap();}
            }
        }
        let prepared=crate::handlers::stamp_promotion::prepare(&db,&promotion,"qa-promotion-success","did:qa").await.unwrap();
        let (stamp_node,promoted_properties,newly)=crate::handlers::stamp_promotion::commit(&db,prepared).await.unwrap();
        assert!(newly && promoted_properties["owning_path_id"]==undo_node);
        let count_after_promotion=op_log_count(&db).await;
        let existing=crate::handlers::stamp_promotion::prepare(&db,&promotion,"qa-promotion-existing","did:qa").await.unwrap();
        let (same,properties,newly)=crate::handlers::stamp_promotion::commit(&db,existing).await.unwrap();
        assert!(!newly && same==stamp_node && properties==promoted_properties);
        assert_eq!(op_log_count(&db).await,count_after_promotion,"idempotent address checks do not mint operations");
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
                "bounds"=>{db.query("UPDATE petal SET bounds=<geometry<polygon>> $bounds WHERE petal_id='tree-other-petal'").bind(("bounds",stamp_bounds.clone())).await.unwrap().check().unwrap();}
                "metadata"=>{db.query("UPDATE petal SET view_hidden=NONE WHERE petal_id='tree-other-petal'").await.unwrap().check().unwrap();}
                "schema"=>{db.query("DELETE field_def WHERE field_def_id='qa-stamp-schema-race'").await.unwrap().check().unwrap();}
                _=>{db.query("REMOVE EVENT fail_stamp_fact ON TABLE node_log").await.unwrap().check().unwrap();}
            }
        }
        let prepared=crate::handlers::stamp_overrides::prepare(&db,&stamp_input,"qa-stamp-success","did:qa").await.unwrap();
        let stamp_saved=crate::handlers::stamp_overrides::commit(&db,prepared).await.unwrap();
        assert_eq!(stamp_saved.state.properties["stamp.override.scale"],serde_json::json!([2.0,1.0,3.0]));
        assert_eq!(stamp_saved.state.properties["stamp.override.rotation"],serde_json::json!([0.0,0.0,0.0,1.0]));
        assert_eq!(stamp_saved.state.properties["stamp.override.arc_m"],serde_json::json!(12.0));
        assert_eq!(stamp_saved.state.properties["node_kind"],"stamp");
        assert_eq!(stamp_saved.state.position,[0.0;3],"stamp save cannot bypass the position lock");
        let (stamp_receipt,stamp_current)=crate::handlers::checked_mutation::resolve(&db,&stamp_node,"tree-other-petal","qa-stamp-success","did:qa").await.unwrap();
        assert_eq!(stamp_receipt.unwrap().history_version,stamp_saved.history_version);
        assert_eq!(stamp_current.properties,stamp_saved.state.properties);
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
        let (imported_node, imported_asset, _) = crate::handlers::crud::import_gltf_handler(
            &url_db, &store, "url-p", "Unique imported Portal", import_path.to_str().unwrap(), [2.0,3.0,4.0],
        ).await.unwrap();
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
        let (competing_node,_) = crate::handlers::crud::create_node_with_asset_as(&url_db,"url-p","shared after capture",&imported_asset,[0.0;3],[0.0;3],[1.0;3],"did:qa").await.unwrap();
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
