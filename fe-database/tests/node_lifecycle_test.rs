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

    // A repeat on the same tombstoned root acknowledges the command but must
    // not append another durable action or re-emit lifecycle side effects.
    h.send(DbCommand::CascadeTombstoneNode {
        node_id: root.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    assert!(
        h.life_rx.try_recv().is_err(),
        "idempotent cascade must not emit a duplicate lifecycle event"
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

// --- T5-LOCK (D-A5): a promoted stamp's position is path-locked ---

#[test]
fn stamp_translate_is_rejected_but_rotation_and_scale_persist() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    h.send(DbCommand::PromoteInstance {
        petal_id: petal.clone(),
        path_id: "path".into(),
        instance_index: 1,
        auth: editor(),
    });
    let stamp = match h.recv() {
        DbResult::NodePromoted { node_id, .. } => node_id,
        o => panic!("expected NodePromoted, got {o:?}"),
    };
    let transform = |node_id: &str| {
        h.send(DbCommand::GetNodeTransform {
            node_id: node_id.into(),
        });
        match h.recv() {
            DbResult::NodeTransformLoaded {
                position,
                rotation,
                scale,
                ..
            } => (position, rotation, scale),
            o => panic!("expected NodeTransformLoaded, got {o:?}"),
        }
    };
    let (origin, _, _) = transform(&stamp);

    // Moving it is refused before the op-log commit: the row is unchanged.
    h.send(DbCommand::UpdateNodeTransform {
        node_id: stamp.clone(),
        position: [origin[0] + 4.0, origin[1], origin[2]],
        rotation: [0.0, 1.0, 0.0],
        scale: [3.0, 3.0, 3.0],
    });
    let (position, rotation, scale) = transform(&stamp);
    assert_eq!(position, origin, "a stamp must not translate");
    assert_eq!(rotation, [0.0, 0.0, 0.0], "a rejected move writes nothing");
    assert_eq!(scale, [1.0, 1.0, 1.0]);

    // Same position, new rotation/scale: persisted.
    h.send(DbCommand::UpdateNodeTransform {
        node_id: stamp.clone(),
        position: origin,
        rotation: [0.0, 1.0, 0.0],
        scale: [2.0, 2.0, 2.0],
    });
    let (position, rotation, scale) = transform(&stamp);
    assert_eq!(position, origin);
    assert_eq!(rotation, [0.0, 1.0, 0.0]);
    assert_eq!(scale, [2.0, 2.0, 2.0]);

    // A plain node still moves freely.
    let node = h.create_node(&petal, "plain");
    h.send(DbCommand::UpdateNodeTransform {
        node_id: node.clone(),
        position: [4.0, 0.0, 2.0],
        rotation: [0.0; 3],
        scale: [1.0; 3],
    });
    assert_eq!(transform(&node).0, [4.0, 0.0, 2.0]);
}

// --- T5 parity: a UI delete and an API delete write the same op-log entry ---

impl Harness {
    /// Rows of a read-only SELECT through the DB thread's `RawQuery` path.
    fn select(&self, sql: &str, vars: serde_json::Value) -> Vec<serde_json::Value> {
        let vars = vars
            .as_object()
            .map(|m| m.clone().into_iter().collect())
            .unwrap_or_default();
        self.send(DbCommand::RawQuery {
            sql: sql.into(),
            vars,
            correlation_id: None,
        });
        match self.recv() {
            DbResult::QueryResult { data, .. } => data,
            o => panic!("expected QueryResult, got {o:?}"),
        }
    }
}

/// Sorted top-level keys of a JSON object.
fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {v}"))
        .keys()
        .cloned()
        .collect();
    k.sort();
    k
}

#[test]
fn ui_and_api_tombstones_write_the_same_op_log_entry() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let ui_node = h.create_node(&petal, "via UI");
    let api_node = h.create_node(&petal, "via API");

    // The UI sends `CallerAuth::Local`; REST/MCP send `Identified` (fe-api
    // `endpoint::caller_auth`). Both reach the one tombstone handler.
    for (node, auth) in [(&ui_node, local()), (&api_node, editor())] {
        h.send(DbCommand::TombstoneNode {
            node_id: node.clone(),
            auth,
        });
        assert!(
            matches!(h.recv(), DbResult::NodeDeleted { .. }),
            "{node} tombstoned"
        );
    }

    let entries = h.select(
        "SELECT * FROM op_log WHERE node_id IN $ids AND op_type = 'NodeTombstoned' \
         ORDER BY lamport_clock ASC",
        serde_json::json!({ "ids": [ui_node, api_node] }),
    );
    assert_eq!(entries.len(), 2, "one entry per delete: {entries:?}");
    let (ui, api) = (&entries[0], &entries[1]);
    assert_eq!(ui["node_id"], ui_node.as_str());
    assert_eq!(api["node_id"], api_node.as_str());
    for e in [ui, api] {
        assert_eq!(e["op_type"], "NodeTombstoned", "{e}");
        assert_eq!(e["payload"]["petal_id"], petal.as_str(), "{e}");
    }
    assert_eq!(keys(ui), keys(api), "same op-log row shape");
    assert_eq!(keys(&ui["payload"]), keys(&api["payload"]), "same payload");
    assert_ne!(ui["lamport_clock"], api["lamport_clock"]);
    assert_eq!(api["payload"]["source_did"], "did:key:z6MkEditor");

    // The row markers match too, each stamped with its own op's clock + source.
    let rows = h.select(
        "SELECT node_id, tombstone FROM node WHERE node_id IN $ids",
        serde_json::json!({ "ids": [ui_node, api_node] }),
    );
    assert_eq!(rows.len(), 2);
    for (entry, node) in [(ui, &ui_node), (api, &api_node)] {
        let marker = &rows.iter().find(|r| r["node_id"] == node.as_str()).unwrap()["tombstone"];
        assert_eq!(
            keys(marker),
            ["hlc", "source_did", "tombstoned_at"],
            "{marker}"
        );
        assert_eq!(marker["hlc"], entry["lamport_clock"], "{node}");
        assert_eq!(
            marker["source_did"], entry["payload"]["source_did"],
            "{node}"
        );
    }
}

// --- N-9 (stamped_asset_nodes Ph2): stamps cost zero rows until promoted ---

impl Harness {
    /// Live (non-tombstoned) `node` rows in `petal_id` matching `extra` SQL.
    fn count_live_nodes(&self, petal_id: &str, extra: &str) -> u64 {
        let rows = self.select(
            &format!(
                "SELECT count() AS n FROM node WHERE petal_id = $p AND tombstone = NONE \
                 {extra} GROUP ALL"
            ),
            serde_json::json!({ "p": petal_id }),
        );
        rows.first().and_then(|r| r["n"].as_u64()).unwrap_or(0)
    }
}

#[test]
fn materialized_stamps_add_no_rows_and_one_promotion_adds_exactly_one() {
    let h = Harness::start();
    let petal = h.create_hierarchy();
    let path = h.create_node(&petal, "stamp path");
    let before_stamping = h.count_live_nodes(&petal, "");

    // A 250 m route at 5 m spacing: ~50 stamps the UI materializes from this
    // property bag alone (verse_manager/path_asset_materialize.rs) — no rows.
    h.set_property(
        &path,
        "path_asset",
        serde_json::json!({
            "asset_path": "blob://x.glb",
            "spacing_mode": "fixed_spacing",
            "spacing_value": 5.0,
            "count": 0,
            "tangent_align": true
        }),
    );
    h.set_property(
        &path,
        "gpx_points",
        serde_json::json!([[0.0, 0.0, 0.0, 0.0], [250.0, 0.0, 0.0, 1.0]]),
    );
    let before = h.count_live_nodes(&petal, "");
    assert_eq!(before, before_stamping, "stamping a path writes no rows");
    assert_eq!(
        h.count_live_nodes(&petal, "AND properties.node_kind = 'stamp'"),
        0
    );

    let promote = || {
        h.send(DbCommand::PromoteInstance {
            petal_id: petal.clone(),
            path_id: path.clone(),
            instance_index: 7,
            auth: editor(),
        });
        match h.recv() {
            DbResult::NodePromoted { newly_promoted, .. } => newly_promoted,
            o => panic!("expected NodePromoted, got {o:?}"),
        }
    };
    assert!(promote(), "first selection promotes");
    assert_eq!(
        h.count_live_nodes(&petal, ""),
        before + 1,
        "one promotion adds exactly one row"
    );
    assert_eq!(
        h.count_live_nodes(&petal, "AND properties.node_kind = 'stamp'"),
        1
    );

    assert!(!promote(), "re-promotion is idempotent");
    assert_eq!(h.count_live_nodes(&petal, ""), before + 1, "no second row");
}
