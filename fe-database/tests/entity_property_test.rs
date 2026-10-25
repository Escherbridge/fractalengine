//! Regression test for the annotation-save silent-failure bug: SetNodeProperty
//! against a nonexistent node_id must surface as `DbResult::Error`, not silently
//! succeed. See `fe-database/src/handlers/entity_property.rs` (matched-row assertion).

use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};
use std::sync::Arc;
use std::time::Duration;

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

struct TestDb {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    _tmp_dir: tempfile::TempDir,
}

fn spawn_test_db() -> TestDb {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir for test DB");
    let db_path = tmp_dir
        .path()
        .join("entity_property_test.db")
        .to_string_lossy()
        .to_string();

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
    .expect("entity_property test DB init failed");

    let started = res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("entity_property test DB did not start within 30s");
    assert!(
        matches!(started, DbResult::Started),
        "expected DbResult::Started, got {started:?}"
    );

    TestDb {
        cmd_tx,
        res_rx,
        _tmp_dir: tmp_dir,
    }
}

#[test]
fn set_node_property_on_nonexistent_node_returns_error() {
    let db = spawn_test_db();

    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: "does-not-exist".to_string(),
            key: "gis.annotation.title".to_string(),
            value: serde_json::json!("test"),
        })
        .unwrap();

    let res = db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("SetNodeProperty result");
    match res {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("matched no node"),
                "expected 'matched no node' error, got: {msg}"
            );
        }
        other => panic!("expected DbResult::Error for nonexistent node_id, got {other:?}"),
    }
}

// --- HS-P6-CALLOUT: a dimension_callout node's property bag round-trips ---

/// Build the minimal verse -> fractal -> petal hierarchy and return the petal ID.
fn seed_hierarchy(db: &TestDb) -> String {
    db.cmd_tx
        .send(DbCommand::CreateVerse {
            name: "callout-test-verse".to_string(),
        })
        .unwrap();
    let verse_id = match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreateVerse result")
    {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreateFractal {
            verse_id,
            name: "callout-test-fractal".to_string(),
        })
        .unwrap();
    let fractal_id = match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreateFractal result")
    {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreatePetal {
            fractal_id,
            name: "callout-test-petal".to_string(),
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreatePetal result")
    {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    }
}

/// Create a node in `petal_id` at local `position` and return its node ID.
fn create_node(db: &TestDb, petal_id: &str, name: &str, position: [f32; 3]) -> String {
    db.cmd_tx
        .send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: name.to_string(),
            position,
            correlation_id: None,
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("CreateNode result")
    {
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
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("SetNodeProperty result")
    {
        DbResult::NodePropertySet { .. } => {}
        other => panic!("expected NodePropertySet, got {other:?}"),
    }
}

/// Load a node's custom properties via the real `GetNodeProperties` command.
fn get_properties(db: &TestDb, node_id: &str) -> serde_json::Value {
    db.cmd_tx
        .send(DbCommand::GetNodeProperties {
            node_id: node_id.to_string(),
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("GetNodeProperties result")
    {
        DbResult::NodePropertiesLoaded { properties, .. } => properties,
        other => panic!("expected NodePropertiesLoaded, got {other:?}"),
    }
}

/// Hard-delete a node via the real `DeleteNode` command.
fn delete_node(db: &TestDb, node_id: &str) {
    db.cmd_tx
        .send(DbCommand::DeleteNode {
            node_id: node_id.to_string(),
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("DeleteNode result")
    {
        DbResult::NodeDeleted { .. } => {}
        other => panic!("expected NodeDeleted, got {other:?}"),
    }
}

/// Reload the whole hierarchy and report whether `node_id` is present anywhere in it.
fn hierarchy_contains_node(db: &TestDb, node_id: &str) -> bool {
    db.cmd_tx.send(DbCommand::LoadHierarchy).unwrap();
    match db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("LoadHierarchy result")
    {
        DbResult::HierarchyLoaded { verses } => verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .flat_map(|p| &p.nodes)
            .any(|n| n.id == node_id),
        other => panic!("expected HierarchyLoaded, got {other:?}"),
    }
}

#[test]
fn a_dimension_callout_node_round_trips_through_the_property_store() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);
    let node_id = create_node(&db, &petal_id, "Dimension 5.00 m", [0.0, 0.0, 0.0]);

    let anchors = serde_json::json!([[0.0, 0.0, 0.0], [3.0, 0.0, 4.0],]);
    set_property(
        &db,
        &node_id,
        "node_kind",
        serde_json::json!("dimension_callout"),
    );
    set_property(&db, &node_id, "dimension.mode", serde_json::json!("tape"));
    set_property(&db, &node_id, "dimension.anchors", anchors.clone());
    set_property(&db, &node_id, "dimension.closed", serde_json::json!(false));

    let props = get_properties(&db, &node_id);
    assert_eq!(props["node_kind"], serde_json::json!("dimension_callout"));
    assert_eq!(props["dimension.mode"], serde_json::json!("tape"));
    assert_eq!(
        props["dimension.anchors"], anchors,
        "f64 anchors must round-trip exactly"
    );
    assert_eq!(props["dimension.closed"], serde_json::json!(false));

    assert!(
        hierarchy_contains_node(&db, &node_id),
        "the live node is present before delete"
    );
    delete_node(&db, &node_id);
    assert!(
        !hierarchy_contains_node(&db, &node_id),
        "a deleted callout node is absent from a live hierarchy load"
    );
}

#[test]
fn delete_node_property_on_nonexistent_node_returns_error() {
    let db = spawn_test_db();

    db.cmd_tx
        .send(DbCommand::DeleteNodeProperty {
            node_id: "does-not-exist".to_string(),
            key: "gis.annotation.title".to_string(),
        })
        .unwrap();

    let res = db
        .res_rx
        .recv_timeout(CMD_TIMEOUT)
        .expect("DeleteNodeProperty result");
    match res {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("matched no node"),
                "expected 'matched no node' error, got: {msg}"
            );
        }
        other => panic!("expected DbResult::Error for nonexistent node_id, got {other:?}"),
    }
}
