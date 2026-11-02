//! Real-store desktop scoped-query data/authorization journey; see src/AGENTS.md scoped-query.

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
    db: fe_database::repo::Db,
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

        let thread = fe_database::spawn_db_thread_shared(
            cmd_rx,
            res_tx,
            mock_blob_store(),
            None,
            None,
            None,
            None,
            Some(db_path),
            None,
        )
        .expect("spawn db thread");

        let h = Harness {
            cmd_tx: Some(cmd_tx),
            res_rx,
            handle: Some(thread.join_handle),
            db: thread.shared_db,
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

    /// The same correlated scoped request the GUI emits.
    fn run_query_tab_sql(&self, petal: &str, sql: &str) -> Vec<serde_json::Value> {
        self.send(DbCommand::ScopedQuery {
            sql: sql.into(),
            vars: Default::default(),
            petal_scope: vec![petal.into()],
            correlation_id: "query-tab-read".into(),
        });
        match self.recv() {
            DbResult::QueryResult {
                data,
                correlation_id,
            } => {
                assert_eq!(correlation_id.as_deref(), Some("query-tab-read"));
                data
            }
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

/// Both data planes remain readable through the guarded desktop request family.
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

    h.set_property(
        &node_id,
        "nested",
        serde_json::json!({"flags":[true,false],"name":"retained"}),
    );
    let private_petal = h.create_hierarchy();
    let private_node = h.create_node(&private_petal, "Private Node");

    let node_sql = format!("SELECT * FROM node WHERE node_id = '{node_id}'");
    let node_rows = h.run_query_tab_sql(&petal_id, &node_sql);
    assert_eq!(node_rows.len(), 1, "node query: {node_sql}");
    assert_eq!(node_rows[0]["node_id"], node_id);
    assert_eq!(
        node_rows[0]["properties"]["label"], "printed-from-field",
        "SetNodeProperty's write reads back through the node row: {node_rows:?}"
    );

    // 3b. "All nodes in a petal" — the query the GIS/Query tab hint should
    // recommend for a petal-wide view.
    let petal_sql = format!("SELECT * FROM node WHERE petal_id = '{petal_id}'");
    let petal_rows = h.run_query_tab_sql(&petal_id, &petal_sql);
    assert_eq!(petal_rows.len(), 1, "petal query: {petal_sql}");
    assert_eq!(petal_rows[0]["node_id"], node_id);

    // 3c. The observation-plane reading, read back the same way.
    let reading_sql = format!("SELECT * FROM iot_reading WHERE node_id = '{node_id}'");
    let reading_rows = h.run_query_tab_sql(&petal_id, &reading_sql);
    assert_eq!(reading_rows.len(), 1, "iot_reading query: {reading_sql}");
    assert_eq!(reading_rows[0]["metric"], "temperature_c");
    assert_eq!(reading_rows[0]["value"].as_f64(), Some(72.5));
    assert_eq!(reading_rows[0]["petal_id"], petal_id);
    assert_eq!(node_rows[0]["properties"]["nested"]["flags"][0], true);
    for sql in [
        "SELECT * FROM node".to_string(),
        format!("SELECT * FROM node WHERE petal_id = '{private_petal}' OR true"),
        format!("SELECT * FROM (SELECT * FROM node WHERE petal_id = '{private_petal}' OR true)"),
    ] {
        let rows = h.run_query_tab_sql(&petal_id, &sql);
        assert!(rows.iter().all(|row| row["node_id"] == node_id));
    }
    assert!(
        h.run_query_tab_sql(
            &petal_id,
            &format!("SELECT * FROM node WHERE node_id = '{private_node}'")
        )
        .is_empty()
    );
    let literal_rows =
        h.run_query_tab_sql(&petal_id, "SELECT 'string::repeat(' AS marker FROM node");
    assert_eq!(literal_rows.len(), 1);
    assert_eq!(literal_rows[0]["marker"], "string::repeat(");
    let aggregate = h.run_query_tab_sql(
        &petal_id,
        "SELECT math::sum(value) AS total FROM iot_reading GROUP ALL",
    );
    assert_eq!(aggregate[0]["total"].as_f64(), Some(72.5));
    for (index, sql) in [
        "SELECT * FROM node /* comment */",
        "SELECT * FROM type::thing('node','x')",
        "SELECT node:secret.* FROM node",
        "SELECT verse:secret.* FROM node",
        "SELECT fractal:secret.* FROM node",
        "SELECT field_def:secret.* FROM node",
        "SELECT crate_entry:secret.* FROM node",
        "SELECT ->asset.* FROM node",
        "SELECT <-field_def.* FROM node",
        "SELECT <->asset.* FROM node",
        "SELECT node FROM node",
        "SELECT * FROM node; SELECT * FROM node",
        "RETURN 1",
        "SELECT namespace_id FROM node",
        "SELECT * FROM node_log",
        "SELECT * FROM op_log",
        "SELECT * FROM role",
        "SELECT * FROM verse",
        "SELECT * FROM node TIMEOUT 1h",
        "SELECT $session FROM node",
        "SELECT $scoped_query_0_role_snapshot FROM node",
        "SELECT file::get('x') FROM node",
        "SELECT fn::private() FROM node",
        "SELECT string::repeat('x',1000000000) FROM node",
        "SELECT array::range(0,1000000000) FROM node",
        "SELECT file::put('x','y') FROM node",
        "SELECT math::mean::private(value) FROM node",
        "SELECT /'/ AS pattern, file::get('x') FROM node",
        "SELECT 0..1000000000 FROM node",
        "SELECT value / 2 FROM iot_reading",
        "SELECT (|$x| $x)(1) FROM node",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("refusal-{index}");
        h.send(DbCommand::ScopedQuery {
            sql: sql.into(),
            vars: Default::default(),
            petal_scope: vec![petal_id.clone()],
            correlation_id: id.clone(),
        });
        assert!(
            matches!(h.recv(), DbResult::QueryFailed { correlation_id, .. } if correlation_id == id)
        );
    }
    for petals in [vec![], vec!["bad'petal".into()], vec![petal_id.clone(); 17]] {
        h.send(DbCommand::ScopedQuery {
            sql: "SELECT * FROM node".into(),
            vars: Default::default(),
            petal_scope: petals,
            correlation_id: "bad-scope".into(),
        });
        assert!(
            matches!(h.recv(), DbResult::QueryFailed { correlation_id, .. } if correlation_id == "bad-scope")
        );
    }

    let mut too_deep = serde_json::Value::Null;
    for _ in 0..18 {
        too_deep = serde_json::json!([too_deep]);
    }
    for vars in [
        std::collections::HashMap::from([(
            "scoped_query_0_role_snapshot".into(),
            serde_json::json!([]),
        )]),
        std::collections::HashMap::from([("list".into(), serde_json::json!(vec![0; 8193]))]),
        std::collections::HashMap::from([("list".into(), too_deep)]),
    ] {
        h.send(DbCommand::ScopedQuery {
            sql: "SELECT $list FROM node".into(),
            vars,
            petal_scope: vec![petal_id.clone()],
            correlation_id: "binding-refusal".into(),
        });
        assert!(
            matches!(h.recv(), DbResult::QueryFailed { correlation_id, .. } if correlation_id == "binding-refusal")
        );
    }
    // The shared handle invalidates stored authority between plan capture and execution.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut ancestry =
            h.db.query(
                "SELECT verse_id, fractal_id FROM fractal WHERE fractal_id IN \
                    (SELECT VALUE fractal_id FROM petal WHERE petal_id=$petal)",
            )
            .bind(("petal", private_petal.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows: Vec<serde_json::Value> = ancestry.take(0).unwrap();
        let verse = rows[0]["verse_id"].as_str().unwrap();
        let fractal = rows[0]["fractal_id"].as_str().unwrap();
        let scope = fe_database::build_scope(verse, Some(fractal), Some(&private_petal));
        let scopes = vec![
            scope.clone(),
            fe_database::build_scope(verse, Some(fractal), None),
            fe_database::build_scope(verse, None, None),
        ];
        let foreign = fe_identity::NodeKeypair::generate().to_did_key();
        h.db.query(
            "UPDATE verse SET created_by=$foreign, default_access='none' WHERE verse_id=$verse; \
                    DELETE role WHERE peer_did='local-node' AND scope IN $scopes",
        )
        .bind(("foreign", foreign))
        .bind(("verse", verse.to_owned()))
        .bind(("scopes", scopes))
        .await
        .unwrap()
        .check()
        .unwrap();
        let denied = fe_database::handlers::scoped_query::scoped_query_reply(
            &h.db,
            "SELECT * FROM node",
            &Default::default(),
            &[private_petal.clone()],
            "local-node",
            "viewer-denied".into(),
        )
        .await;
        assert!(
            matches!(denied, DbResult::QueryFailed { correlation_id, .. }
            if correlation_id == "viewer-denied")
        );
        h.db.query("CREATE role CONTENT {peer_did:'local-node',scope:$scope,role:'viewer'}")
            .bind(("scope", scope.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
        let viewer = fe_database::handlers::scoped_query::scoped_query_reply(
            &h.db,
            "SELECT * FROM node",
            &Default::default(),
            &[private_petal.clone()],
            "local-node",
            "viewer-read".into(),
        )
        .await;
        assert!(
            matches!(viewer, DbResult::QueryResult { data, correlation_id }
            if data.len() == 1 && data[0]["node_id"] == private_node
                && correlation_id.as_deref() == Some("viewer-read"))
        );
        let plan = fe_database::handlers::scoped_query::prepare(
            &h.db,
            "SELECT * FROM node",
            &Default::default(),
            &[private_petal.clone()],
            "local-node",
        )
        .await
        .unwrap();
        let captured = DbCommand::ScopedQuery {
            sql: "SELECT * FROM node".into(),
            vars: Default::default(),
            petal_scope: vec![private_petal.clone()],
            correlation_id: "captured-before-revocation".into(),
        };
        h.db.query("UPDATE role SET role='none' WHERE peer_did='local-node' AND scope=$scope")
            .bind(("scope", scope))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            plan.execute(&h.db).await.unwrap_err(),
            "not found or not permitted",
            "revoked captured authority must fail its transactional fence uniformly",
        );
        h.send(captured);
        assert!(
            matches!(h.recv(), DbResult::QueryFailed { correlation_id, .. }
            if correlation_id == "captured-before-revocation")
        );
        for requested in [
            vec!["missing-petal".into()],
            vec![petal_id.clone(), private_petal.clone()],
        ] {
            let denied = fe_database::handlers::scoped_query::scoped_query_reply(
                &h.db,
                "SELECT * FROM node",
                &Default::default(),
                &requested,
                "local-node",
                "uniform-denied".into(),
            )
            .await;
            assert!(
                matches!(denied, DbResult::QueryFailed { error, correlation_id }
                if correlation_id == "uniform-denied" && error == "not found or not permitted")
            );
        }
    });
}
