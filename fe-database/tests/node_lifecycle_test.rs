//! Durable-path integration tests for the node-lifecycle spine
//! (`node_lifecycle_addressing_20260725`): tombstone delete (FR-1), authorization
//! (N-5), cascade (FR-2), lazy promotion (FR-5) and lifecycle events (FR-6),
//! all driven end-to-end through the real DB thread + `DbCommand` public API
//! against an on-disk SurrealDB. Runs in its own binary so it never shares the
//! process-global HLC with the `op_log` unit tests that assert exact clocks.

use fe_database::{spawn_db_thread_with_sync_and_lifecycle, BlobStoreHandle, LifecycleEventSender};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult, LifecycleEvent};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

fn editor() -> CallerAuth {
    CallerAuth::identified("did:key:z6MkEditor", "editor")
}
fn viewer() -> CallerAuth {
    CallerAuth::identified("did:key:z6MkViewer", "viewer")
}
/// The local desktop session (N-5): asserts no role — the DB thread resolves the
/// local user's identity + scope role. In this harness the local user
/// (`"local-node"`, no keypair) owns every verse it creates, so `Local` resolves
/// to Owner and is authorized.
fn local() -> CallerAuth {
    CallerAuth::Local
}

/// A running DB thread + its channels, over a temp on-disk DB. The `Drop` impl
/// closes the command channel (breaking the loop) and joins the thread *before*
/// the temp dir is removed, so the SurrealKv file lock is released first.
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    life_rx: crossbeam::channel::Receiver<LifecycleEvent>,
    handle: Option<std::thread::JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn start() -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let (life_tx, life_rx) = crossbeam::channel::bounded::<LifecycleEvent>(64);
        let life_tx: LifecycleEventSender = life_tx;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("fe.db").to_string_lossy().to_string();

        let handle = spawn_db_thread_with_sync_and_lifecycle(
            cmd_rx,
            res_tx,
            blob_store,
            None,
            None,
            None,
            None,
            Some(db_path),
            Some(life_tx),
        )
        .expect("spawn db thread");

        let h = Harness {
            cmd_tx: Some(cmd_tx),
            res_rx,
            life_rx,
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
    fn recv_life(&self) -> LifecycleEvent {
        self.life_rx
            .recv_timeout(TIMEOUT)
            .expect("lifecycle event within timeout")
    }
    fn drain_life(&self) {
        while self.life_rx.try_recv().is_ok() {}
    }

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

    fn set_parent(&self, node_id: &str, parent_id: &str) {
        self.set_property(node_id, "parent_id", serde_json::json!(parent_id));
    }

    fn set_property(&self, node_id: &str, key: &str, value: serde_json::Value) {
        self.send(DbCommand::SetNodeProperty {
            node_id: node_id.into(),
            key: key.into(),
            value,
        });
        assert!(matches!(self.recv(), DbResult::NodePropertySet { .. }));
    }

    fn get_properties(&self, node_id: &str) -> serde_json::Value {
        self.send(DbCommand::GetNodeProperties {
            node_id: node_id.into(),
        });
        match self.recv() {
            DbResult::NodePropertiesLoaded { properties, .. } => properties,
            o => panic!("expected NodePropertiesLoaded, got {o:?}"),
        }
    }

    fn load_nodes(&self, petal_id: &str) -> Vec<fe_runtime::messages::NodeDto> {
        self.send(DbCommand::LoadNodesByPetal {
            petal_id: petal_id.into(),
        });
        match self.recv() {
            DbResult::NodesLoaded { nodes, .. } => nodes,
            o => panic!("expected NodesLoaded, got {o:?}"),
        }
    }

    fn load_ids(&self, petal_id: &str) -> Vec<String> {
        self.load_nodes(petal_id)
            .into_iter()
            .map(|n| n.node_id)
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.cmd_tx.take()); // close the channel → the DB loop breaks
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // `_dir` is removed after this, once the file lock is released.
    }
}

// --- FR-1 + N-5 + FR-6: tombstone is authorized, sync-safe, event-emitting ---

#[test]
fn tombstone_is_authorized_survives_reload_and_emits_one_event() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");

    // FR-6: create emitted exactly one NodeCreated carrying the fe:// address.
    match h.recv_life() {
        LifecycleEvent::NodeCreated { node_id, address } => {
            assert_eq!(node_id, node);
            assert!(address.starts_with("fe://"), "stable address: {address}");
        }
        o => panic!("expected NodeCreated event, got {o:?}"),
    }
    h.drain_life();

    // N-5: an Anonymous caller is rejected — no mutation, no event.
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: CallerAuth::Anonymous,
    });
    match h.recv() {
        DbResult::Error(e) => assert!(e.contains("denied"), "anonymous denied: {e}"),
        o => panic!("anonymous delete must be denied, got {o:?}"),
    }
    assert!(h.life_rx.try_recv().is_err(), "a denied op emits no event");
    assert!(
        h.load_ids(&petal).contains(&node),
        "node survives a denied delete"
    );

    // N-5: a Viewer is also rejected.
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: viewer(),
    });
    assert!(
        matches!(h.recv(), DbResult::Error(_)),
        "viewer must be denied"
    );
    assert!(h.load_ids(&petal).contains(&node));

    // Editor succeeds → NodeDeleted result + exactly one NodeDeleted event.
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: editor(),
    });
    match h.recv() {
        DbResult::NodeDeleted { node_id, .. } => assert_eq!(node_id, node),
        o => panic!("editor delete must succeed, got {o:?}"),
    }
    match h.recv_life() {
        LifecycleEvent::NodeDeleted { node_id, .. } => assert_eq!(node_id, node),
        o => panic!("expected NodeDeleted event, got {o:?}"),
    }
    assert!(
        h.life_rx.try_recv().is_err(),
        "plain delete emits exactly one event"
    );

    // FR-1: the tombstone survives the reload read (soft delete, not gone).
    assert!(
        !h.load_ids(&petal).contains(&node),
        "tombstoned node is hidden from the scene read"
    );
}

// --- N-5: the local UI issues `CallerAuth::Local` and the DB resolves its role ---

#[test]
fn local_caller_is_authorized_by_db_resolved_role() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");
    h.drain_life();

    // The UI sends `CallerAuth::Local` — it asserts NO role (N-5: no UI-side
    // self-authorization). The DB thread resolves the local user's real role at
    // the node's scope. Here the local user is the verse owner (it created the
    // verse), so the tombstone is authorized and emits exactly one event.
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: local(),
    });
    match h.recv() {
        DbResult::NodeDeleted { node_id, .. } => assert_eq!(node_id, node),
        o => panic!("local owner tombstone must succeed, got {o:?}"),
    }
    match h.recv_life() {
        LifecycleEvent::NodeDeleted { node_id, .. } => assert_eq!(node_id, node),
        o => panic!("expected NodeDeleted event, got {o:?}"),
    }
    assert!(
        h.life_rx.try_recv().is_err(),
        "an authorized local delete emits exactly one event"
    );
    assert!(
        !h.load_ids(&petal).contains(&node),
        "node tombstoned by the DB-resolved local owner"
    );
}

// --- FR-2: cascade tombstones the whole subtree atomically ---

#[test]
fn cascade_tombstones_whole_subtree() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let root = h.create_node(&petal, "root");
    let child = h.create_node(&petal, "child");
    let grand = h.create_node(&petal, "grand");
    h.set_parent(&child, &root);
    h.set_parent(&grand, &child);
    h.drain_life();

    h.send(DbCommand::CascadeTombstoneNode {
        node_id: root.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    // FR-6: a cascade is one op → exactly one NodeDeleted event (rooted).
    assert!(matches!(h.recv_life(), LifecycleEvent::NodeDeleted { .. }));
    assert!(
        h.life_rx.try_recv().is_err(),
        "cascade emits exactly one event"
    );

    let ids = h.load_ids(&petal);
    for n in [&root, &child, &grand] {
        assert!(!ids.contains(n), "{n} must be tombstoned by the cascade");
    }
}

// --- FR-5: promotion is authorized, materializes one node, is idempotent ---

#[test]
fn promote_is_authorized_and_idempotent() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    h.drain_life();

    // N-5: a Viewer cannot promote.
    h.send(DbCommand::PromoteInstance {
        petal_id: petal.clone(),
        path_id: "path".into(),
        instance_index: 0,
        auth: viewer(),
    });
    assert!(
        matches!(h.recv(), DbResult::Error(_)),
        "viewer promote denied"
    );

    // Editor promotes → materializes one addressable node + one NodePromoted event.
    h.send(DbCommand::PromoteInstance {
        petal_id: petal.clone(),
        path_id: "path".into(),
        instance_index: 0,
        auth: editor(),
    });
    match h.recv() {
        DbResult::NodePromoted {
            newly_promoted,
            node_id,
            ..
        } => {
            assert!(newly_promoted, "first promotion materializes a row");
            assert_eq!(node_id, "path#inst-0");
        }
        o => panic!("expected NodePromoted, got {o:?}"),
    }
    assert!(matches!(h.recv_life(), LifecycleEvent::NodePromoted { .. }));
    assert!(
        h.load_ids(&petal).contains(&"path#inst-0".to_string()),
        "promoted node is addressable in its petal"
    );

    // Idempotent: re-promoting the same instance is a no-op with no event.
    h.send(DbCommand::PromoteInstance {
        petal_id: petal.clone(),
        path_id: "path".into(),
        instance_index: 0,
        auth: editor(),
    });
    match h.recv() {
        DbResult::NodePromoted { newly_promoted, .. } => {
            assert!(!newly_promoted, "re-promotion is idempotent")
        }
        o => panic!("expected NodePromoted, got {o:?}"),
    }
    assert!(
        h.life_rx.try_recv().is_err(),
        "idempotent re-promote emits no event"
    );
}

// --- FR-6: a repeat tombstone on an already-deleted node emits no second event ---

#[test]
fn repeat_tombstone_is_idempotent_and_emits_no_second_event() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");
    h.drain_life();

    // First tombstone → succeeds with exactly one NodeDeleted event.
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    assert!(matches!(h.recv_life(), LifecycleEvent::NodeDeleted { .. }));
    assert!(
        h.life_rx.try_recv().is_err(),
        "first delete emits exactly one event"
    );

    // Second tombstone on the already-tombstoned node is an idempotent no-op:
    // still an Ok result, but NO second lifecycle event (empty `tombstoned_ids`
    // gates the emit — FR-6 "exactly one event").
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    assert!(
        h.life_rx.try_recv().is_err(),
        "repeat tombstone on an already-deleted node emits no second event"
    );
}

// --- Step 1: rename is authorized, persists, and emits one event ---

#[test]
fn rename_is_authorized_persists_and_emits_one_event() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");
    h.drain_life();

    // N-5: a Viewer is denied — name untouched, no event.
    h.send(DbCommand::RenameNode {
        node_id: node.clone(),
        new_name: "X".into(),
        auth: viewer(),
    });
    match h.recv() {
        DbResult::Error(e) => assert!(e.contains("denied"), "viewer denied: {e}"),
        o => panic!("viewer rename must be denied, got {o:?}"),
    }
    assert!(
        h.life_rx.try_recv().is_err(),
        "a denied rename emits no event"
    );
    let names: Vec<String> = h.load_nodes(&petal).into_iter().map(|n| n.name).collect();
    assert!(names.contains(&"N".to_string()), "name survives: {names:?}");

    // Editor rename → NodeRenamed result + exactly one NodeRenamed event.
    h.send(DbCommand::RenameNode {
        node_id: node.clone(),
        new_name: "N2".into(),
        auth: editor(),
    });
    match h.recv() {
        DbResult::NodeRenamed { node_id, new_name } => {
            assert_eq!(node_id, node);
            assert_eq!(new_name, "N2");
        }
        o => panic!("expected NodeRenamed, got {o:?}"),
    }
    match h.recv_life() {
        LifecycleEvent::NodeRenamed { node_id, address } => {
            assert_eq!(node_id, node);
            assert!(address.starts_with("fe://"), "stable address: {address}");
        }
        o => panic!("expected NodeRenamed event, got {o:?}"),
    }
    assert!(
        h.life_rx.try_recv().is_err(),
        "rename emits exactly one event"
    );

    // Persisted: the reload read sees the new display_name.
    let names: Vec<String> = h.load_nodes(&petal).into_iter().map(|n| n.name).collect();
    assert!(names.contains(&"N2".to_string()), "renamed: {names:?}");

    // N-5: the local session renames via its DB-resolved (owner) role.
    h.send(DbCommand::RenameNode {
        node_id: node.clone(),
        new_name: "N3".into(),
        auth: local(),
    });
    assert!(
        matches!(h.recv(), DbResult::NodeRenamed { .. }),
        "local owner rename is authorized via the DB-resolved role"
    );
}

#[test]
fn rename_missing_or_tombstoned_node_is_an_error() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");
    h.drain_life();

    // Nonexistent node: scope resolution fails → error, never a silent success.
    h.send(DbCommand::RenameNode {
        node_id: "no-such-node".into(),
        new_name: "X".into(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::Error(_)));

    // Tombstoned node: the live-row UPDATE matches nothing → error, no event.
    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    h.drain_life();
    h.send(DbCommand::RenameNode {
        node_id: node,
        new_name: "X".into(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::Error(_)));
    assert!(h.life_rx.try_recv().is_err());
}

// --- Step 1: duplicate copies the row minus the path-binding identity keys ---

#[test]
fn duplicate_copies_row_minus_path_identity_keys() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");
    h.set_property(&node, "owning_path_id", serde_json::json!("path"));
    h.set_property(&node, "path_id", serde_json::json!("path"));
    h.set_property(&node, "instance_index", serde_json::json!(2));
    h.set_property(&node, "node_kind", serde_json::json!("stamp"));
    h.set_property(&node, "stamp.override.scale", serde_json::json!(2.0));
    h.set_property(&node, "stamp.override.arc_m", serde_json::json!(5.0));
    h.set_property(&node, "annotation", serde_json::json!("keep me"));
    h.drain_life();

    // N-5: a Viewer is denied — no copy, no event.
    h.send(DbCommand::DuplicateNode {
        node_id: node.clone(),
        auth: viewer(),
    });
    assert!(
        matches!(h.recv(), DbResult::Error(_)),
        "viewer duplicate denied"
    );
    assert!(h.life_rx.try_recv().is_err());

    // Editor duplicates → NodeCreated with " (copy)" name and +1 m x/z offset.
    h.send(DbCommand::DuplicateNode {
        node_id: node.clone(),
        auth: editor(),
    });
    let copy_id = match h.recv() {
        DbResult::NodeCreated {
            id,
            petal_id,
            name,
            correlation_id,
            position,
            ..
        } => {
            assert_eq!(name, "N (copy)");
            assert_eq!(position, [1.0, 0.0, 1.0], "+1 m x/z in raw meters (N-1)");
            assert_eq!(correlation_id, None);
            assert_eq!(petal_id, petal);
            id
        }
        o => panic!("expected NodeCreated, got {o:?}"),
    };
    assert!(matches!(h.recv_life(), LifecycleEvent::NodeCreated { .. }));
    assert!(
        h.life_rx.try_recv().is_err(),
        "duplicate emits exactly one event"
    );

    // Properties copied MINUS the path-binding identity keys (a copy is not
    // path-bound — see fe-database/src/AGENTS.md §lifecycle).
    let props = h.get_properties(&copy_id);
    assert_eq!(props["annotation"], serde_json::json!("keep me"));
    assert_eq!(props["stamp.override.scale"], serde_json::json!(2.0));
    for stripped in [
        "owning_path_id",
        "path_id",
        "instance_index",
        "node_kind",
        "stamp.override.arc_m",
    ] {
        assert!(
            props.get(stripped).is_none(),
            "{stripped} must be stripped from the copy: {props:?}"
        );
    }
    // The source keeps its identity untouched.
    let src_props = h.get_properties(&node);
    assert_eq!(src_props["node_kind"], serde_json::json!("stamp"));
}

#[test]
fn duplicate_keeps_non_stamp_node_kind() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "E");
    h.set_property(&node, "node_kind", serde_json::json!("earthwork_region"));
    h.drain_life();

    h.send(DbCommand::DuplicateNode {
        node_id: node,
        auth: editor(),
    });
    let copy_id = match h.recv() {
        DbResult::NodeCreated { id, .. } => id,
        o => panic!("expected NodeCreated, got {o:?}"),
    };
    let props = h.get_properties(&copy_id);
    assert_eq!(
        props["node_kind"],
        serde_json::json!("earthwork_region"),
        "a non-stamp kind carries over to the copy"
    );
}

#[test]
fn duplicate_missing_or_tombstoned_node_is_an_error() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let node = h.create_node(&petal, "N");
    h.drain_life();

    h.send(DbCommand::DuplicateNode {
        node_id: "no-such-node".into(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::Error(_)));

    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    h.drain_life();
    h.send(DbCommand::DuplicateNode {
        node_id: node,
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::Error(_)));
    assert!(h.life_rx.try_recv().is_err());
}

// --- Step 1: descendant count shares the cascade BFS, excludes the root ---

#[test]
fn count_descendants_excludes_root_and_tombstoned_rows() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let root = h.create_node(&petal, "root");
    let child = h.create_node(&petal, "child");
    let grand = h.create_node(&petal, "grand");
    h.set_parent(&child, &root);
    h.set_parent(&grand, &child);
    h.drain_life();

    h.send(DbCommand::CountNodeDescendants {
        node_id: root.clone(),
    });
    match h.recv() {
        DbResult::NodeDescendantCount { node_id, count } => {
            assert_eq!(node_id, root);
            assert_eq!(count, 2, "child + grand, root excluded");
        }
        o => panic!("expected NodeDescendantCount, got {o:?}"),
    }

    // After the cascade the whole subtree is tombstoned → the count is 0.
    h.send(DbCommand::CascadeTombstoneNode {
        node_id: root.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    h.drain_life();
    h.send(DbCommand::CountNodeDescendants { node_id: root });
    match h.recv() {
        DbResult::NodeDescendantCount { count, .. } => assert_eq!(count, 0),
        o => panic!("expected NodeDescendantCount, got {o:?}"),
    }
}

// --- Step 1: promotion writes fe-query's stamp read contract ---

#[test]
fn promotion_writes_the_stamp_read_contract_properties() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    h.drain_life();

    h.send(DbCommand::PromoteInstance {
        petal_id: petal.clone(),
        path_id: "path".into(),
        instance_index: 3,
        auth: editor(),
    });
    let node_id = match h.recv() {
        DbResult::NodePromoted {
            node_id,
            newly_promoted,
            ..
        } => {
            assert!(newly_promoted);
            node_id
        }
        o => panic!("expected NodePromoted, got {o:?}"),
    };

    // Write-side of fe-query/src/spatial_nodes.rs's read contract.
    let props = h.get_properties(&node_id);
    assert_eq!(props["node_kind"], serde_json::json!("stamp"));
    assert_eq!(props["path_id"], serde_json::json!("path"));
    assert_eq!(
        props["owning_path_id"],
        serde_json::json!("path"),
        "the cascade/reflow parent edge is kept"
    );
    assert_eq!(props["instance_index"], serde_json::json!(3));
}

// --- Step 1: a stamp tombstone re-flows its path naming the vanished slot ---

#[test]
fn tombstoning_a_promoted_stamp_reflows_with_deleted_index() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    h.drain_life();

    h.send(DbCommand::PromoteInstance {
        petal_id: petal.clone(),
        path_id: "path".into(),
        instance_index: 2,
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodePromoted { .. }));
    h.drain_life();

    h.send(DbCommand::TombstoneNode {
        node_id: "path#inst-2".into(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    assert!(matches!(h.recv_life(), LifecycleEvent::NodeDeleted { .. }));
    match h.recv_life() {
        LifecycleEvent::PathReflow {
            path_id,
            deleted_index,
        } => {
            assert_eq!(path_id, "path");
            assert_eq!(
                deleted_index,
                Some(2),
                "the reflow names the vanished stamp slot"
            );
        }
        o => panic!("expected PathReflow event, got {o:?}"),
    }
    assert!(h.life_rx.try_recv().is_err());
}
