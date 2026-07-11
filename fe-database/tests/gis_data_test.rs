//! Integration tests for `fe_query::gis` builders against the real embedded
//! DB, exercised through `DbCommand::RawQuery` -- see `fe-database/src/AGENTS.md`
//! §gis and `fe-query/AGENTS.md` §gis for the shared contract.

use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// An isolated DB thread backed by a unique temp directory -- no shared
/// state or file-lock contention with other tests (each gets its own path).
struct TestDb {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    _tmp_dir: tempfile::TempDir,
}

fn spawn_test_db() -> TestDb {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir for test DB");
    let db_path = tmp_dir.path().join("gis_test.db").to_string_lossy().to_string();

    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(256);
    let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(256);

    let _handle = fe_database::spawn_db_thread_with_sync(
        cmd_rx,
        res_tx,
        mock_blob_store(),
        None,
        None,
        None,
        None,
        Some(db_path),
    )
    .expect("gis test DB init failed");

    let started = res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("gis test DB did not start within 30s");
    assert!(matches!(started, DbResult::Started), "expected DbResult::Started, got {started:?}");

    TestDb { cmd_tx, res_rx, _tmp_dir: tmp_dir }
}

/// Build the minimal verse -> fractal -> petal hierarchy and return the petal ID.
fn seed_hierarchy(db: &TestDb) -> String {
    db.cmd_tx
        .send(DbCommand::CreateVerse { name: "gis-test-verse".to_string() })
        .unwrap();
    let verse_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateVerse result") {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreateFractal { verse_id, name: "gis-test-fractal".to_string() })
        .unwrap();
    let fractal_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateFractal result") {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreatePetal { fractal_id, name: "gis-test-petal".to_string() })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreatePetal result") {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    }
}

/// Create a node in `petal_id` at local `position` ([x, elevation, z]) and
/// return its node ID.
fn create_node(db: &TestDb, petal_id: &str, name: &str, position: [f32; 3]) -> String {
    db.cmd_tx
        .send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: name.to_string(),
            position,
        })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateNode result") {
        DbResult::NodeCreated { id, .. } => id,
        other => panic!("expected NodeCreated, got {other:?}"),
    }
}

/// Set a custom property on a node via the real `SetNodeProperty` command.
fn set_property(db: &TestDb, node_id: &str, key: &str, value: serde_json::Value) {
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value,
        })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("SetNodeProperty result") {
        DbResult::NodePropertySet { .. } => {}
        other => panic!("expected NodePropertySet, got {other:?}"),
    }
}

/// Execute an `fe_query::gis` [`fe_query::BuiltQuery`] through the real
/// `DbCommand::RawQuery` path and return the resulting rows.
fn run_gis_query(db: &TestDb, query: fe_query::BuiltQuery) -> Vec<serde_json::Value> {
    let vars: HashMap<String, serde_json::Value> = query.params.into_iter().collect();
    db.cmd_tx
        .send(DbCommand::RawQuery { sql: query.sql, vars })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("RawQuery result") {
        DbResult::QueryResult { data } => data,
        other => panic!("expected QueryResult, got {other:?}"),
    }
}

fn node_ids(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| r.get("node_id").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// nodes_in_bbox
// ---------------------------------------------------------------------------

#[test]
fn nodes_in_bbox_includes_inside_and_excludes_just_outside() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);

    // Inside [0,0] .. [10,10].
    let inside_id = create_node(&db, &petal_id, "inside", [5.0, 2.0, 5.0]);
    // Just past max_x=10 -- negative case.
    let outside_id = create_node(&db, &petal_id, "just-outside", [11.0, 0.0, 5.0]);

    let q = fe_query::gis::nodes_in_bbox(&petal_id, 0.0, 0.0, 10.0, 10.0)
        .expect("bbox builder should validate cleanly");
    let rows = run_gis_query(&db, q);
    let ids = node_ids(&rows);

    assert!(ids.contains(&inside_id), "inside node missing from bbox result: {ids:?}");
    assert!(!ids.contains(&outside_id), "just-outside node must be excluded: {ids:?}");
}

// ---------------------------------------------------------------------------
// nodes_within_radius
// ---------------------------------------------------------------------------

#[test]
fn nodes_within_radius_includes_near_and_excludes_far() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);

    // Distance from origin: sqrt(5^2 + 5^2) ~= 7.07, within radius 10.
    let near_id = create_node(&db, &petal_id, "near", [5.0, 2.0, 5.0]);
    // Distance from origin: sqrt(11^2 + 5^2) ~= 12.08, outside radius 10.
    let far_id = create_node(&db, &petal_id, "far", [11.0, 0.0, 5.0]);

    let q = fe_query::gis::nodes_within_radius(&petal_id, 0.0, 0.0, 10.0)
        .expect("radius builder should validate cleanly");
    let rows = run_gis_query(&db, q);
    let ids = node_ids(&rows);

    assert!(ids.contains(&near_id), "near node missing from radius result: {ids:?}");
    assert!(!ids.contains(&far_id), "far node must be excluded: {ids:?}");
}

// ---------------------------------------------------------------------------
// annotated_nodes
// ---------------------------------------------------------------------------

#[test]
fn annotated_nodes_returns_only_nodes_with_gis_annotation_properties() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);

    let annotated_id = create_node(&db, &petal_id, "annotated", [3.0, 1.0, 3.0]);
    set_property(&db, &annotated_id, "gis.annotation.title", serde_json::json!("Base Camp"));
    set_property(&db, &annotated_id, "gis.annotation.body", serde_json::json!("Trailhead marker"));
    set_property(&db, &annotated_id, "gis.annotation.color", serde_json::json!("#ff8800"));

    // A plain node with no properties at all -- negative case (properties IS NONE).
    let plain_id = create_node(&db, &petal_id, "plain", [4.0, 1.0, 4.0]);

    // A node with an unrelated custom property -- negative case (properties
    // set, but no gis.annotation.* keys).
    let other_prop_id = create_node(&db, &petal_id, "other-prop", [6.0, 1.0, 6.0]);
    set_property(&db, &other_prop_id, "color", serde_json::json!("blue"));

    let q = fe_query::gis::annotated_nodes(&petal_id)
        .expect("annotated builder should validate cleanly");
    let rows = run_gis_query(&db, q);
    let ids = node_ids(&rows);

    assert!(ids.contains(&annotated_id), "annotated node missing: {ids:?}");
    assert!(!ids.contains(&plain_id), "plain node (no properties) must be excluded: {ids:?}");
    assert!(
        !ids.contains(&other_prop_id),
        "node with unrelated property must be excluded: {ids:?}"
    );

    let annotated_row = rows
        .iter()
        .find(|r| r.get("node_id").and_then(|v| v.as_str()) == Some(annotated_id.as_str()))
        .expect("annotated row present");
    assert_eq!(
        annotated_row["properties"]["gis.annotation.title"],
        serde_json::json!("Base Camp")
    );
    assert_eq!(
        annotated_row["properties"]["gis.annotation.color"],
        serde_json::json!("#ff8800")
    );
}
