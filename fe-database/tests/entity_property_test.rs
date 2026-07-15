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
