//! Round-trip test for the GPX path editor's `gpx_points` flat node property
//! (FR-3, `gpx_path_editor_20260711`) — a JSON array of `[x, y, z,
//! time_seconds]` in petal-local meters. See `fractalengine/src/gpx_bridge.rs`
//! (`GPX_POINTS_KEY`) and `fe-ui/src/path_ops.rs`. Uses the existing
//! `SetNodeProperty`/`GetNodeProperties`/`CreateNode` commands only — no new
//! `DbCommand`/`DbResult` variants (fe-database/src/lib.rs is quarantined).

use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};
use std::sync::Arc;
use std::time::Duration;

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// An isolated DB thread backed by a unique temp directory.
struct TestDb {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    _tmp_dir: tempfile::TempDir,
}

fn spawn_test_db() -> TestDb {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir for test DB");
    let db_path = tmp_dir.path().join("gpx_path_persistence_test.db").to_string_lossy().to_string();

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
    .expect("gpx_path_persistence test DB init failed");

    let started = res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("gpx_path_persistence test DB did not start within 30s");
    assert!(matches!(started, DbResult::Started), "expected DbResult::Started, got {started:?}");

    TestDb { cmd_tx, res_rx, _tmp_dir: tmp_dir }
}

/// Build the minimal verse -> fractal -> petal hierarchy and return the petal ID.
fn seed_hierarchy(db: &TestDb) -> String {
    db.cmd_tx.send(DbCommand::CreateVerse { name: "gpx-path-test-verse".to_string() }).unwrap();
    let verse_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateVerse result") {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreateFractal { verse_id, name: "gpx-path-test-fractal".to_string() })
        .unwrap();
    let fractal_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateFractal result") {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreatePetal { fractal_id, name: "gpx-path-test-petal".to_string() })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreatePetal result") {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    }
}

/// Create a node in `petal_id` at local `position` and return its node ID.
fn create_node(db: &TestDb, petal_id: &str, name: &str, position: [f32; 3]) -> String {
    db.cmd_tx
        .send(DbCommand::CreateNode { petal_id: petal_id.to_string(), name: name.to_string(), position })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateNode result") {
        DbResult::NodeCreated { id, .. } => id,
        other => panic!("expected NodeCreated, got {other:?}"),
    }
}

/// Set a custom property on a node via the real `SetNodeProperty` command.
fn set_property(db: &TestDb, node_id: &str, key: &str, value: serde_json::Value) {
    db.cmd_tx
        .send(DbCommand::SetNodeProperty { node_id: node_id.to_string(), key: key.to_string(), value })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("SetNodeProperty result") {
        DbResult::NodePropertySet { .. } => {}
        other => panic!("expected NodePropertySet, got {other:?}"),
    }
}

/// Load a node's custom properties via the real `GetNodeProperties` command.
fn get_properties(db: &TestDb, node_id: &str) -> serde_json::Value {
    db.cmd_tx.send(DbCommand::GetNodeProperties { node_id: node_id.to_string() }).unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("GetNodeProperties result") {
        DbResult::NodePropertiesLoaded { properties, .. } => properties,
        other => panic!("expected NodePropertiesLoaded, got {other:?}"),
    }
}

#[test]
fn gpx_points_round_trip_persists_and_reads_back() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);
    let node_id = create_node(&db, &petal_id, "Test Track", [0.0, 0.0, 0.0]);

    let points = serde_json::json!([
        [0.0, 0.0, 0.0, 0.0],
        [10.5, 2.0, -3.25, 1.0],
        [20.0, 4.0, -6.5, 2.0],
    ]);
    set_property(&db, &node_id, "gpx_points", points.clone());
    set_property(&db, &node_id, "gpx_type", serde_json::json!("track"));

    let props = get_properties(&db, &node_id);
    assert_eq!(props["gpx_points"], points, "gpx_points must round-trip exactly");
    assert_eq!(props["gpx_type"], serde_json::json!("track"));
}

#[test]
fn gpx_points_empty_array_round_trips_for_freshly_created_track() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);
    let node_id = create_node(&db, &petal_id, "Empty Track", [1.0, 0.0, 1.0]);

    set_property(&db, &node_id, "gpx_points", serde_json::json!([]));
    set_property(&db, &node_id, "gpx_type", serde_json::json!("track"));

    let props = get_properties(&db, &node_id);
    assert_eq!(props["gpx_points"], serde_json::json!([]));
}

#[test]
fn gpx_points_overwrite_replaces_previous_value() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);
    let node_id = create_node(&db, &petal_id, "Appended Track", [0.0, 0.0, 0.0]);

    set_property(&db, &node_id, "gpx_points", serde_json::json!([[0.0, 0.0, 0.0, 0.0]]));
    set_property(
        &db,
        &node_id,
        "gpx_points",
        serde_json::json!([[0.0, 0.0, 0.0, 0.0], [5.0, 1.0, 5.0, 1.0]]),
    );

    let props = get_properties(&db, &node_id);
    let arr = props["gpx_points"].as_array().expect("gpx_points must be an array");
    assert_eq!(arr.len(), 2, "second SetNodeProperty must overwrite, not append");
}

#[test]
fn gpx_points_survives_alongside_other_gpx_track_stat_properties() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);
    let node_id = create_node(&db, &petal_id, "Stats Track", [0.0, 0.0, 0.0]);

    set_property(&db, &node_id, "gpx_type", serde_json::json!("track"));
    set_property(&db, &node_id, "total_distance_m", serde_json::json!(1234.5));
    set_property(
        &db,
        &node_id,
        "gpx_points",
        serde_json::json!([[0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 1.0, 1.0]]),
    );

    let props = get_properties(&db, &node_id);
    assert_eq!(props["gpx_type"], serde_json::json!("track"));
    assert_eq!(props["total_distance_m"], serde_json::json!(1234.5));
    assert_eq!(
        props["gpx_points"],
        serde_json::json!([[0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 1.0, 1.0]])
    );
}
