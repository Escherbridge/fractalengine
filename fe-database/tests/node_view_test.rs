//! Durable-path tests for persisted node visibility (`SetNodeView` /
//! `LoadPetalViewState`, hierarchy_visibility_groups Phase 3): Editor+
//! authorization, tombstone refusal, and the "override survives an app
//! restart" acceptance — driven through the real DB thread over an on-disk
//! SurrealDB that is closed and reopened. Own binary (process-global HLC).

use fe_database::{spawn_db_thread_with_sync_and_lifecycle, BlobStoreHandle};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult, NodeViewDto};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

fn editor() -> CallerAuth {
    CallerAuth::identified("did:key:z6MkEditor", "editor")
}
fn viewer() -> CallerAuth {
    CallerAuth::identified("did:key:z6MkViewer", "viewer")
}

/// A DB thread over a store directory the TEST owns, so a second `Harness` can
/// reopen the same files. `Drop` closes the channel and joins the thread,
/// releasing the SurrealKv lock before the next open.
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn open(dir: &Path) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let db_path = dir.join("fe.db").to_string_lossy().to_string();
        let handle = spawn_db_thread_with_sync_and_lifecycle(
            cmd_rx,
            res_tx,
            blob_store,
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
            handle: Some(handle),
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

    fn create_petal(&self) -> String {
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

    fn set_view(
        &self,
        node_id: &str,
        state: Option<&str>,
        groups: Option<Vec<String>>,
        auth: CallerAuth,
        correlation_id: Option<&str>,
    ) -> DbResult {
        self.send(DbCommand::SetNodeView {
            node_id: node_id.into(),
            state: state.map(str::to_string),
            groups,
            auth,
            correlation_id: correlation_id.map(str::to_string),
        });
        self.recv()
    }

    fn petal_view(&self, petal_id: &str) -> Vec<NodeViewDto> {
        self.send(DbCommand::LoadPetalViewState {
            petal_id: petal_id.into(),
        });
        match self.recv() {
            DbResult::PetalViewStateLoaded { entries, .. } => entries,
            o => panic!("expected PetalViewStateLoaded, got {o:?}"),
        }
    }

    fn properties(&self, node_id: &str) -> serde_json::Value {
        self.send(DbCommand::GetNodeProperties {
            node_id: node_id.into(),
        });
        match self.recv() {
            DbResult::NodePropertiesLoaded { properties, .. } => properties,
            o => panic!("expected NodePropertiesLoaded, got {o:?}"),
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.cmd_tx.take());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[test]
fn a_hide_override_is_authorized_and_survives_a_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (petal, hidden, plain) = {
        let h = Harness::open(dir.path());
        let petal = h.create_petal();
        let hidden = h.create_node(&petal, "Hidden");
        let plain = h.create_node(&petal, "Plain");

        // A Viewer cannot persist visibility — and nothing is written.
        match h.set_view(&hidden, Some("hide"), None, viewer(), None) {
            DbResult::Error(e) => assert!(e.contains("denied"), "viewer denied: {e}"),
            o => panic!("viewer SetNodeView must be denied, got {o:?}"),
        }
        assert!(
            h.properties(&hidden).get("view.state").is_none(),
            "a denied write leaves the row untouched"
        );
        assert!(h.petal_view(&petal).is_empty());

        // An Editor can: the echo carries the persisted view + owning petal.
        match h.set_view(
            &hidden,
            Some("hide"),
            Some(vec!["utilities".into()]),
            editor(),
            None,
        ) {
            DbResult::NodeViewSet {
                node_id,
                petal_id,
                state,
                groups,
                ..
            } => {
                assert_eq!(node_id, hidden);
                assert_eq!(petal_id, petal);
                assert_eq!(state.as_deref(), Some("hide"));
                assert_eq!(groups, Some(vec!["utilities".to_string()]));
            }
            o => panic!("editor SetNodeView must succeed, got {o:?}"),
        }
        (petal, hidden, plain)
    }; // DB thread joined, store closed

    // Reopen the SAME store: the override is still there, the untouched node is not listed.
    let h = Harness::open(dir.path());
    let entries = h.petal_view(&petal);
    assert_eq!(
        entries,
        vec![NodeViewDto {
            node_id: hidden.clone(),
            state: Some("hide".into()),
            groups: Some(vec!["utilities".into()]),
        }],
        "override survives restart; {plain} has no view and is not listed"
    );

    // `auto` + an empty group list remove the keys — absent IS the default.
    match h.set_view(&hidden, Some("auto"), Some(Vec::new()), editor(), None) {
        DbResult::NodeViewSet { state, groups, .. } => {
            assert_eq!((state, groups), (None, None));
        }
        o => panic!("expected NodeViewSet, got {o:?}"),
    }
    assert!(h.petal_view(&petal).is_empty());
}

#[test]
fn a_tombstoned_or_invalid_target_is_refused_and_correlated_failures_are_typed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let h = Harness::open(dir.path());
    let petal = h.create_petal();
    let node = h.create_node(&petal, "N");

    // Invalid state never reaches a write; a correlated caller gets RequestFailed.
    match h.set_view(&node, Some("invisible"), None, editor(), Some("api-1")) {
        DbResult::RequestFailed {
            correlation_id,
            error,
        } => {
            assert_eq!(correlation_id, "api-1");
            assert!(error.contains("rejected"), "{error}");
        }
        o => panic!("expected RequestFailed, got {o:?}"),
    }

    h.send(DbCommand::TombstoneNode {
        node_id: node.clone(),
        auth: editor(),
    });
    assert!(matches!(h.recv(), DbResult::NodeDeleted { .. }));
    match h.set_view(&node, Some("hide"), None, editor(), None) {
        DbResult::Error(e) => assert!(e.contains("no live node"), "{e}"),
        o => panic!("a tombstoned node must be refused, got {o:?}"),
    }
}
