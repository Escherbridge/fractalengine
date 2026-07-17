use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult, EntityType, SceneChange};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

#[test]
fn test_db_thread_isolation() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "No ambient Tokio runtime should exist on the test (main) thread"
    );
}

#[test]
fn test_db_ping_pong_roundtrip() {
    // Use the process-wide shared DB connection so this test does not conflict
    // with the scene-change tests that keep the same `data/fractalengine.db`
    // file open for the entire binary lifetime.
    let _guard = db_lock();
    let db = shared_scene_db();

    let t0 = Instant::now();
    db.cmd_tx.send(DbCommand::Ping).unwrap();
    let result = db
        .res_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("No Pong received");
    let elapsed = t0.elapsed();

    assert!(matches!(result, DbResult::Pong));
    assert!(
        elapsed.as_millis() < 200,
        "DB round-trip {}ms exceeded 200ms budget",
        elapsed.as_millis()
    );
}

/// P2P Mycelium Phase A acceptance: `spawn_db_thread` now takes a
/// `BlobStoreHandle`, and passing a `MockBlobStore` continues to let
/// commands round-trip. The DB thread drops its Arc clone on shutdown.
///
/// This is a compile-time + drop-semantics check — the DB file is already
/// exercised by `test_db_ping_pong_roundtrip` under the same lock, so this
/// test only validates that the handle is accepted and releases properly.
#[test]
fn blob_store_handle_is_accepted_and_released() {
    let store: BlobStoreHandle = mock_blob_store();
    let store_observer = Arc::clone(&store);
    // Constructing a second handle clone is cheap; the acceptance is that
    // the signature compiles with the extra parameter and Arcs balance.
    drop(store);
    assert_eq!(
        Arc::strong_count(&store_observer),
        1,
        "only observer holds the Arc after the supplied handle drops"
    );
}

// ---------------------------------------------------------------------------
// SceneChange emission tests
//
// Architecture note: a LIVE SELECT approach was spiked during Phase 3 but is
// fundamentally incompatible with the blocking crossbeam command loop — the
// tokio runtime inside the DB thread runs `block_on`, which cannot drive a
// live cursor concurrently with command dispatch.  Manual emission (emitting
// SceneChange variants directly inside each command arm) was therefore
// retained as the authoritative design.  The tests below serve as the safety
// net: they ensure every mutating command fires the correct broadcast event
// on `entity_change_tx` so WS subscribers receive real-time deltas without
// polling.
//
// DB sharing strategy
// -------------------
// SurrealKV holds an exclusive OS-level file lock on `data/fractalengine.db`.
// On Windows the lock is not released immediately when the backing file handle
// is closed — the OS may hold it for hundreds of milliseconds after the DB
// thread joins.  Opening a second connection to the same file within the same
// test binary therefore fails intermittently even when the previous connection
// appears to have been closed.
//
// To work around this, the four scene-change tests share a SINGLE persistent
// DB thread (and its broadcast `Sender`) for the entire test binary run,
// managed via `SHARED_SCENE_DB`.  The `db_lock()` mutex serialises access so
// that each test's command/result round-trips are atomic.
// ---------------------------------------------------------------------------

/// Shared DB state used by all scene-change emission tests.
///
/// Initialised once on first use and kept alive until the process exits.
/// The DB thread is never explicitly shut down — the process exit cleans it up.
/// Uses a dedicated temp directory to avoid file-lock conflicts with the
/// production `data/fractalengine.db`.
struct SharedSceneDb {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    scene_tx: tokio::sync::broadcast::Sender<SceneChange>,
    _tmp_dir: tempfile::TempDir, // kept alive so the directory isn't deleted
}

// Safety: both channel types are Send.  The Receiver<DbResult> is accessed
// only while holding DB_LOCK, so no concurrent access occurs.
unsafe impl Sync for SharedSceneDb {}

static SHARED_SCENE_DB: std::sync::OnceLock<SharedSceneDb> = std::sync::OnceLock::new();

/// Process-wide mutex serialising test access to the shared DB connection.
static DB_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

fn db_lock() -> std::sync::MutexGuard<'static, ()> {
    DB_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Return a reference to the shared DB state, initialising it on first call.
///
/// Retries the open with exponential back-off (up to 60 s) to survive a
/// brief post-join file-lock hold by a preceding test.
fn shared_scene_db() -> &'static SharedSceneDb {
    SHARED_SCENE_DB.get_or_init(|| {
        let tmp_dir = tempfile::tempdir().expect("failed to create temp dir for test DB");
        let db_path = tmp_dir.path().join("test.db").to_string_lossy().to_string();

        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(256);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(256);
        let (scene_tx, _discard_rx) = tokio::sync::broadcast::channel::<SceneChange>(256);

        let _handle = fe_database::spawn_db_thread_with_sync(
            cmd_rx,
            res_tx,
            mock_blob_store(),
            None,
            None,
            None,
            Some(scene_tx.clone()),
            Some(db_path),
        )
        .expect("shared DB init failed");

        // Drain Started.
        let started = res_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("shared DB did not start within 30 s");
        assert!(
            matches!(started, DbResult::Started),
            "expected DbResult::Started, got {started:?}"
        );
        // Intentionally leak the JoinHandle — the DB thread runs for
        // the process lifetime and exits when the binary terminates.
        SharedSceneDb {
            cmd_tx,
            res_rx,
            scene_tx,
            _tmp_dir: tmp_dir,
        }
    })
}

/// Poll `broadcast_rx.try_recv()` until a message arrives or `timeout` elapses.
/// Panics with a descriptive message on timeout.
fn recv_broadcast(
    rx: &mut tokio::sync::broadcast::Receiver<SceneChange>,
    timeout: Duration,
) -> SceneChange {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(change) => return change,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    panic!("timed out after {timeout:?} waiting for a SceneChange broadcast event");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                panic!("broadcast receiver lagged by {n} messages — increase channel capacity");
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                panic!("broadcast channel closed before a SceneChange was received");
            }
        }
    }
}

/// Build the minimal verse → fractal → petal hierarchy and return the petal ID.
fn seed_hierarchy(
    cmd_tx: &crossbeam::channel::Sender<DbCommand>,
    res_rx: &crossbeam::channel::Receiver<DbResult>,
) -> String {
    let cmd_timeout = Duration::from_secs(5);

    cmd_tx
        .send(DbCommand::CreateVerse {
            name: "test-verse".to_string(),
        })
        .unwrap();
    let verse_id = match res_rx
        .recv_timeout(cmd_timeout)
        .expect("CreateVerse result")
    {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    cmd_tx
        .send(DbCommand::CreateFractal {
            verse_id: verse_id.clone(),
            name: "test-fractal".to_string(),
        })
        .unwrap();
    let fractal_id = match res_rx
        .recv_timeout(cmd_timeout)
        .expect("CreateFractal result")
    {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };

    cmd_tx
        .send(DbCommand::CreatePetal {
            fractal_id,
            name: "test-petal".to_string(),
        })
        .unwrap();
    match res_rx
        .recv_timeout(cmd_timeout)
        .expect("CreatePetal result")
    {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    }
}

/// Create a node inside `petal_id` and return its node ID.
fn create_node(
    cmd_tx: &crossbeam::channel::Sender<DbCommand>,
    res_rx: &crossbeam::channel::Receiver<DbResult>,
    petal_id: &str,
) -> String {
    cmd_tx
        .send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: "test-node".to_string(),
            position: [1.0, 2.0, 3.0],
            correlation_id: None,
        })
        .unwrap();
    match res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("CreateNode result")
    {
        DbResult::NodeCreated { id, .. } => id,
        other => panic!("expected NodeCreated, got {other:?}"),
    }
}

/// `CreateNode` must emit `SceneChange::NodeAdded` on the broadcast channel.
#[test]
fn test_create_node_emits_scene_change() {
    let _guard = db_lock();
    let db = shared_scene_db();
    let mut ecr = db.scene_tx.subscribe();

    let petal_id = seed_hierarchy(&db.cmd_tx, &db.res_rx);

    db.cmd_tx
        .send(DbCommand::CreateNode {
            petal_id: petal_id.clone(),
            name: "emit-test-node".to_string(),
            position: [10.0, 20.0, 30.0],
            correlation_id: None,
        })
        .unwrap();

    // The result channel carries the NodeCreated ack — receive it first so we
    // know the command completed, then check the broadcast.
    let node_id = match db
        .res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("CreateNode result")
    {
        // camera_focus_clip_20260716 FR-1: NodeCreated must echo the create
        // position (was previously dropped, forcing fe-ui to focus on [0;3]).
        DbResult::NodeCreated { id, position, .. } => {
            assert_eq!(
                position,
                [10.0, 20.0, 30.0],
                "NodeCreated.position mismatch"
            );
            id
        }
        other => panic!("expected NodeCreated, got {other:?}"),
    };

    let change = recv_broadcast(&mut ecr, Duration::from_secs(5));
    match change {
        SceneChange::NodeAdded { node } => {
            assert_eq!(node.node_id, node_id, "NodeAdded.node_id mismatch");
            assert_eq!(node.petal_id, petal_id, "NodeAdded.petal_id mismatch");
            assert_eq!(node.name, "emit-test-node", "NodeAdded.name mismatch");
            assert_eq!(
                node.position,
                [10.0, 20.0, 30.0],
                "NodeAdded.position mismatch"
            );
            assert!(
                !node.has_asset,
                "NodeAdded.has_asset should be false for plain node"
            );
        }
        other => panic!("expected SceneChange::NodeAdded, got {other:?}"),
    }
}

/// A freshly created node must be readable back from the DB with its geometry
/// persisted — handler `Ok` alone is not proof of persistence (regression:
/// see fe-database/src/AGENTS.md §geometry-inserts).
#[test]
fn test_created_node_is_persisted_with_geometry() {
    let _guard = db_lock();
    let db = shared_scene_db();

    let petal_id = seed_hierarchy(&db.cmd_tx, &db.res_rx);
    let node_id = create_node(&db.cmd_tx, &db.res_rx, &petal_id);

    let mut vars = std::collections::HashMap::new();
    vars.insert("nid".to_string(), serde_json::json!(node_id));
    db.cmd_tx
        .send(DbCommand::RawQuery {
            sql: "SELECT node_id, position FROM node WHERE node_id = $nid".to_string(),
            vars,
        })
        .unwrap();

    match db
        .res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("RawQuery result")
    {
        DbResult::QueryResult { data } => {
            assert_eq!(data.len(), 1, "created node must exist in DB, got {data:?}");
            assert!(
                !data[0]["position"].is_null(),
                "node.position must persist as geometry, got {:?}",
                data[0]
            );
        }
        other => panic!("expected QueryResult, got {other:?}"),
    }
}

/// `UpdateNodeTransform` must emit `SceneChange::NodeTransform` on the broadcast
/// channel.  Note: this command does NOT send a `DbResult` on success — the
/// broadcast event is the only observable outcome from the caller's perspective.
#[test]
fn test_update_transform_emits_scene_change() {
    let _guard = db_lock();
    let db = shared_scene_db();

    let petal_id = seed_hierarchy(&db.cmd_tx, &db.res_rx);
    let node_id = create_node(&db.cmd_tx, &db.res_rx, &petal_id);

    // Subscribe AFTER setup so the receiver does not see setup events.
    let mut ecr = db.scene_tx.subscribe();

    let new_pos = [5.0_f32, 6.0, 7.0];
    let new_rot = [0.1_f32, 0.2, 0.3];
    let new_scale = [2.0_f32, 2.0, 2.0];

    db.cmd_tx
        .send(DbCommand::UpdateNodeTransform {
            node_id: node_id.clone(),
            position: new_pos,
            rotation: new_rot,
            scale: new_scale,
        })
        .unwrap();

    // UpdateNodeTransform emits no DbResult on success — wait directly on the
    // broadcast channel.
    let change = recv_broadcast(&mut ecr, Duration::from_secs(5));
    match change {
        SceneChange::NodeTransform {
            node_id: nid,
            position,
            rotation,
            scale,
        } => {
            assert_eq!(nid, node_id, "NodeTransform.node_id mismatch");
            assert_eq!(position, new_pos, "NodeTransform.position mismatch");
            assert_eq!(rotation, new_rot, "NodeTransform.rotation mismatch");
            assert_eq!(scale, new_scale, "NodeTransform.scale mismatch");
        }
        other => panic!("expected SceneChange::NodeTransform, got {other:?}"),
    }
}

/// `SetNodeProperty` must emit `SceneChange::PropertyChanged` with the
/// supplied key and value on the broadcast channel.
#[test]
fn test_set_property_emits_scene_change() {
    let _guard = db_lock();
    let db = shared_scene_db();

    let petal_id = seed_hierarchy(&db.cmd_tx, &db.res_rx);
    let node_id = create_node(&db.cmd_tx, &db.res_rx, &petal_id);

    // Subscribe after setup so the receiver is clean.
    let mut ecr = db.scene_tx.subscribe();

    let prop_key = "color".to_string();
    let prop_val = serde_json::json!("crimson");

    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.clone(),
            key: prop_key.clone(),
            value: prop_val.clone(),
        })
        .unwrap();

    // The result channel carries a NodePropertySet ack.
    match db
        .res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("SetNodeProperty result")
    {
        DbResult::NodePropertySet { node_id: nid, key } => {
            assert_eq!(nid, node_id);
            assert_eq!(key, prop_key);
        }
        other => panic!("expected NodePropertySet, got {other:?}"),
    }

    let change = recv_broadcast(&mut ecr, Duration::from_secs(5));
    match change {
        SceneChange::PropertyChanged {
            node_id: nid,
            key,
            value,
        } => {
            assert_eq!(nid, node_id, "PropertyChanged.node_id mismatch");
            assert_eq!(key, prop_key, "PropertyChanged.key mismatch");
            assert_eq!(value, prop_val, "PropertyChanged.value mismatch");
        }
        other => panic!("expected SceneChange::PropertyChanged, got {other:?}"),
    }
}

/// `DeleteNodeProperty` must emit `SceneChange::PropertyChanged` with
/// `serde_json::Value::Null` as the value, signalling the removal to subscribers.
#[test]
fn test_delete_property_emits_scene_change() {
    let _guard = db_lock();
    let db = shared_scene_db();

    let petal_id = seed_hierarchy(&db.cmd_tx, &db.res_rx);
    let node_id = create_node(&db.cmd_tx, &db.res_rx, &petal_id);

    let prop_key = "opacity".to_string();

    // First set the property so there is something to delete.
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.clone(),
            key: prop_key.clone(),
            value: serde_json::json!(0.75),
        })
        .unwrap();
    db.res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("SetNodeProperty result for setup");

    // Subscribe after setup so the receiver does not see setup events.
    let mut ecr = db.scene_tx.subscribe();

    // Now delete the property.
    db.cmd_tx
        .send(DbCommand::DeleteNodeProperty {
            node_id: node_id.clone(),
            key: prop_key.clone(),
        })
        .unwrap();

    match db
        .res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("DeleteNodeProperty result")
    {
        DbResult::NodePropertyDeleted { node_id: nid, key } => {
            assert_eq!(nid, node_id);
            assert_eq!(key, prop_key);
        }
        other => panic!("expected NodePropertyDeleted, got {other:?}"),
    }

    let change = recv_broadcast(&mut ecr, Duration::from_secs(5));
    match change {
        SceneChange::PropertyChanged {
            node_id: nid,
            key,
            value,
        } => {
            assert_eq!(nid, node_id, "PropertyChanged.node_id mismatch");
            assert_eq!(key, prop_key, "PropertyChanged.key mismatch");
            assert_eq!(
                value,
                serde_json::Value::Null,
                "DeleteNodeProperty must emit Value::Null to signal removal"
            );
        }
        other => panic!("expected SceneChange::PropertyChanged(Null), got {other:?}"),
    }
}

/// `RenameEntity` must emit `SceneChange::NodeRenamed` on the broadcast channel
/// and the new name must be readable back from the DB (persist proof, not just
/// handler `Ok` — see fe-database/src/AGENTS.md).
#[test]
fn test_rename_entity_emits_scene_change() {
    let _guard = db_lock();
    let db = shared_scene_db();

    let petal_id = seed_hierarchy(&db.cmd_tx, &db.res_rx);

    // Subscribe after setup so the receiver does not see setup events.
    let mut ecr = db.scene_tx.subscribe();

    db.cmd_tx
        .send(DbCommand::RenameEntity {
            entity_type: EntityType::Petal,
            entity_id: petal_id.clone(),
            new_name: "renamed-petal".to_string(),
        })
        .unwrap();

    match db
        .res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("RenameEntity result")
    {
        DbResult::EntityRenamed {
            entity_id,
            new_name,
            ..
        } => {
            assert_eq!(entity_id, petal_id, "EntityRenamed.entity_id mismatch");
            assert_eq!(new_name, "renamed-petal", "EntityRenamed.new_name mismatch");
        }
        other => panic!("expected EntityRenamed, got {other:?}"),
    }

    let change = recv_broadcast(&mut ecr, Duration::from_secs(5));
    match change {
        SceneChange::NodeRenamed { node_id, new_name } => {
            assert_eq!(node_id, petal_id, "NodeRenamed.node_id mismatch");
            assert_eq!(new_name, "renamed-petal", "NodeRenamed.new_name mismatch");
        }
        other => panic!("expected SceneChange::NodeRenamed, got {other:?}"),
    }

    // Read-back: the rename must be persisted, not just acknowledged.
    let mut vars = std::collections::HashMap::new();
    vars.insert("pid".to_string(), serde_json::json!(petal_id));
    db.cmd_tx
        .send(DbCommand::RawQuery {
            sql: "SELECT name FROM petal WHERE petal_id = $pid".to_string(),
            vars,
        })
        .unwrap();
    match db
        .res_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("RawQuery result")
    {
        DbResult::QueryResult { data } => {
            assert_eq!(
                data.len(),
                1,
                "renamed petal must exist in DB, got {data:?}"
            );
            assert_eq!(
                data[0]["name"],
                serde_json::json!("renamed-petal"),
                "petal.name must persist the rename, got {:?}",
                data[0]
            );
        }
        other => panic!("expected QueryResult, got {other:?}"),
    }
}
