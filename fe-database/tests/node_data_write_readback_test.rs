//! F4 (in-app "does the Inspector Query tab actually work?"): an end-to-end
//! probe of the real path the GUI uses — create verse → fractal → petal →
//! node with the SAME `DbCommand`s the GUI sends, write to the node on BOTH
//! planes a user might mean by "send data to a node" (a `SetNodeProperty`
//! write AND an `InsertIotReadings` observation-plane write), then run the
//! SAME kind of raw `SELECT` the Inspector Query tab's "Run Query" button
//! submits (`DbCommand::RawQuery`, no bind vars — `fe-ui/src/actions/query.rs`
//! always sends `vars: HashMap::new()`, so a real user of that panel can only
//! type literal ids into the WHERE clause, never `$petal_id`-style binds).
//!
//! This exercises the real DB thread over an on-disk SurrealKV store (not the
//! in-memory bypass `iot_reading_test.rs` uses), the same gateway every panel
//! (Inspector Query tab, GIS panel, Paths tab) goes through. See
//! fe-database/src/AGENTS.md §handlers "raw_query" and §iot-readings.

use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult, IotReadingInput, SourceClass};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

/// A running DB thread + its channels, over a temp on-disk SurrealKV store —
/// one open per process, joined before the temp dir drops (Windows file-lock
/// safe), mirrors `node_lifecycle_test.rs`'s `Harness`.
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    handle: Option<std::thread::JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn start() -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir
            .path()
            .join("node_data_rt.db")
            .to_string_lossy()
            .to_string();

        let handle = fe_database::spawn_db_thread_with_sync(
            cmd_rx,
            res_tx,
            mock_blob_store(),
            None,
            None,
            None,
            None,
            Some(db_path),
        )
        .expect("spawn db thread");

        let h = Harness {
            cmd_tx: Some(cmd_tx),
            res_rx,
            handle: Some(handle),
            _dir: dir,
        };
        assert!(
            matches!(h.recv(), DbResult::Started),
            "first result is Started"
        );
        h
    }

    fn send(&self, cmd: DbCommand) {
        self.cmd_tx.as_ref().unwrap().send(cmd).unwrap();
    }
    fn recv(&self) -> DbResult {
        self.res_rx
            .recv_timeout(TIMEOUT)
            .expect("db result within timeout")
    }

    /// Same 3 `DbCommand`s the GUI sends on CreateVerse/CreateFractal/CreatePetal
    /// clicks (`fe-ui/src/actions`). Returns `petal_id`.
    fn create_hierarchy(&self) -> String {
        self.send(DbCommand::CreateVerse { name: "V".into() });
        let verse_id = match self.recv() {
            DbResult::VerseCreated { id, .. } => id,
            o => panic!("expected VerseCreated, got {o:?}"),
        };
        self.send(DbCommand::CreateFractal {
            verse_id,
            name: "F".into(),
        });
        let fractal_id = match self.recv() {
            DbResult::FractalCreated { id, .. } => id,
            o => panic!("expected FractalCreated, got {o:?}"),
        };
        self.send(DbCommand::CreatePetal {
            fractal_id,
            name: "P".into(),
        });
        match self.recv() {
            DbResult::PetalCreated { id, .. } => id,
            o => panic!("expected PetalCreated, got {o:?}"),
        }
    }

    /// Same `DbCommand` the GUI's "create node" tool sends.
    fn create_node(&self, petal_id: &str, name: &str) -> String {
        self.send(DbCommand::CreateNode {
            petal_id: petal_id.into(),
            name: name.into(),
            position: [0.0, 0.0, 0.0],
            correlation_id: None,
        });
        match self.recv() {
            DbResult::NodeCreated { id, .. } => id,
            o => panic!("expected NodeCreated, got {o:?}"),
        }
    }

    /// Same `DbCommand` the Inspector's property editor sends on a field edit.
    fn set_property(&self, node_id: &str, key: &str, value: serde_json::Value) {
        self.send(DbCommand::SetNodeProperty {
            node_id: node_id.into(),
            key: key.into(),
            value,
            correlation_id: None,
        });
        assert!(matches!(self.recv(), DbResult::NodePropertySet { .. }));
    }

    /// The observation-plane "send data to a node" door (device/IoT ingest).
    /// `verse_id: None` is the durable-only leg (`replicated_petal_scope`
    /// short-circuits on `None`, so no node-role check applies here) — this
    /// test probes the data path, not RBAC (per task scope).
    fn insert_reading(&self, petal_id: &str, node_id: &str, metric: &str, value: f64) -> usize {
        self.send(DbCommand::InsertIotReadings {
            petal_id: petal_id.into(),
            verse_id: None,
            source_did: "did:key:z6MkTestSensor".into(),
            readings: vec![IotReadingInput {
                node_id: node_id.into(),
                metric: metric.into(),
                value,
                units: "C".into(),
                recorded_at: None,
                payload_id: None,
            }],
            source_class: SourceClass::Manual,
            correlation_id: None,
        });
        match self.recv() {
            DbResult::IotReadingsInserted { written, .. } => written,
            o => panic!("expected IotReadingsInserted, got {o:?}"),
        }
    }

    /// The EXACT path `fe-ui/src/actions/query.rs::submit` takes: a bare
    /// `DbCommand::RawQuery` with `vars: HashMap::new()` — the Inspector Query
    /// tab's "Run Query" button never binds variables, so this helper takes
    /// literal SQL text, same as what a user types into that text box.
    fn run_query_tab_sql(&self, sql: &str) -> Vec<serde_json::Value> {
        self.send(DbCommand::RawQuery {
            sql: sql.into(),
            vars: std::collections::HashMap::new(),
            correlation_id: None,
        });
        match self.recv() {
            DbResult::QueryResult { data, .. } => data,
            o => panic!("expected QueryResult, got {o:?}"),
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.cmd_tx.take()); // close the channel -> the DB loop breaks
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // `_dir` is removed after this, once the SurrealKV file lock is released.
    }
}

/// F4: can we send data to a node and query it back through the exact GUI
/// seam (CreateVerse/Fractal/Petal/Node → SetNodeProperty + InsertIotReadings
/// → the Inspector Query tab's unscoped-vars `RawQuery`)? YES — this is the
/// positive proof: both write planes land, and the literal-SQL `SELECT` a
/// real Query-tab user would type reads every written value back.
#[test]
fn node_data_written_on_both_planes_reads_back_through_query_tab_raw_query() {
    let h = Harness::start();

    // 1. Build the hierarchy exactly as GUI navigation/creation would.
    let petal_id = h.create_hierarchy();
    let node_id = h.create_node(&petal_id, "Sensor Node");

    // 2a. Write plane #1: a node property edit (Inspector "Properties" tab).
    h.set_property(&node_id, "label", serde_json::json!("printed-from-field"));

    // 2b. Write plane #2: the observation plane ("send data to a node" —
    // the device/IoT ingest door a real sensor or the simulation lab uses).
    let written = h.insert_reading(&petal_id, &node_id, "temperature_c", 72.5);
    assert_eq!(written, 1, "exactly one reading accepted");

    // 3a. The Inspector Query tab's scoped-looking "one node's data" query.
    // NB: the Query tab's displayed "Scope: VERSE#..-FRACTAL#..-PETAL#.." is
    // cosmetic only — `node` carries no `scope` column and `query.rs::submit`
    // drops the scope string on the floor (`UiAction::SubmitQuery { scope: _ }`
    // in fe-ui/src/actions/mod.rs). A real user must filter by the literal
    // `node_id` themselves; there is no automatic scoping.
    let node_sql = format!("SELECT * FROM node WHERE node_id = '{node_id}'");
    let node_rows = h.run_query_tab_sql(&node_sql);
    assert_eq!(node_rows.len(), 1, "node query: {node_sql}");
    assert_eq!(node_rows[0]["node_id"], node_id);
    assert_eq!(
        node_rows[0]["properties"]["label"], "printed-from-field",
        "SetNodeProperty's write reads back through the node row: {node_rows:?}"
    );

    // 3b. "All nodes in a petal" — the query the GIS/Query tab hint should
    // recommend for a petal-wide view.
    let petal_sql = format!("SELECT * FROM node WHERE petal_id = '{petal_id}'");
    let petal_rows = h.run_query_tab_sql(&petal_sql);
    assert_eq!(petal_rows.len(), 1, "petal query: {petal_sql}");
    assert_eq!(petal_rows[0]["node_id"], node_id);

    // 3c. The observation-plane reading, read back the same way.
    let reading_sql = format!("SELECT * FROM iot_reading WHERE node_id = '{node_id}'");
    let reading_rows = h.run_query_tab_sql(&reading_sql);
    assert_eq!(reading_rows.len(), 1, "iot_reading query: {reading_sql}");
    assert_eq!(reading_rows[0]["metric"], "temperature_c");
    assert_eq!(reading_rows[0]["value"].as_f64(), Some(72.5));
    assert_eq!(reading_rows[0]["petal_id"], petal_id);
}
