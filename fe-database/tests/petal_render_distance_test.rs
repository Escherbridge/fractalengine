//! Durable-path tests for `petal.render_distance` (p2p_asset_streaming S6):
//! Editor+ authorization, exact round-trips through the real DB-thread
//! dispatch loop, and that the hierarchy loader carries the column. Mirrors
//! `visibility_groups_test.rs`'s harness. Own binary (process-global HLC).

use fe_database::{spawn_db_thread_with_sync_and_lifecycle, BlobStoreHandle};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult, VerseHierarchyData};
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

struct Ids {
    petal: String,
}

/// A DB thread over a store directory the TEST owns; `Drop` joins the thread.
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
    fn ask(&self, cmd: DbCommand) -> DbResult {
        self.send(cmd);
        self.recv()
    }

    fn create_hierarchy(&self) -> Ids {
        let verse = match self.ask(DbCommand::CreateVerse { name: "V".into() }) {
            DbResult::VerseCreated { id, .. } => id,
            o => panic!("expected VerseCreated, got {o:?}"),
        };
        let fractal = match self.ask(DbCommand::CreateFractal {
            verse_id: verse.clone(),
            name: "F".into(),
        }) {
            DbResult::FractalCreated { id, .. } => id,
            o => panic!("expected FractalCreated, got {o:?}"),
        };
        let petal = match self.ask(DbCommand::CreatePetal {
            fractal_id: fractal.clone(),
            name: "P".into(),
        }) {
            DbResult::PetalCreated { id, .. } => id,
            o => panic!("expected PetalCreated, got {o:?}"),
        };
        Ids { petal }
    }

    fn set_distance(
        &self,
        petal_id: &str,
        render_distance: Option<f32>,
        auth: CallerAuth,
    ) -> DbResult {
        self.ask(DbCommand::SetPetalRenderDistance {
            petal_id: petal_id.into(),
            render_distance,
            auth,
            correlation_id: None,
        })
    }

    fn get_distance(&self, petal_id: &str) -> Option<f32> {
        match self.ask(DbCommand::GetPetalRenderDistance {
            petal_id: petal_id.into(),
            correlation_id: None,
        }) {
            DbResult::PetalRenderDistanceLoaded {
                render_distance, ..
            } => render_distance,
            o => panic!("expected PetalRenderDistanceLoaded, got {o:?}"),
        }
    }

    fn hierarchy(&self) -> Vec<VerseHierarchyData> {
        loop {
            self.send(DbCommand::LoadHierarchy);
            match self.recv() {
                DbResult::HierarchyLoaded { verses } => return verses,
                _ => continue,
            }
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

fn find_petal(verses: &[VerseHierarchyData], petal_id: &str) -> Option<f32> {
    verses
        .iter()
        .flat_map(|v| &v.fractals)
        .flat_map(|f| &f.petals)
        .find(|p| p.id == petal_id)
        .and_then(|p| p.render_distance)
}

#[test]
fn set_then_load_hierarchy_carries_the_value() {
    let dir = tempfile::tempdir().expect("temp dir");
    let h = Harness::open(dir.path());
    let ids = h.create_hierarchy();
    assert_eq!(h.get_distance(&ids.petal), None, "fresh petal inherits");

    match h.set_distance(&ids.petal, Some(1500.0), editor()) {
        DbResult::PetalRenderDistanceLoaded {
            render_distance: Some(v),
            ..
        } => assert_eq!(v, 1500.0),
        o => panic!("expected PetalRenderDistanceLoaded, got {o:?}"),
    }
    assert_eq!(h.get_distance(&ids.petal), Some(1500.0));

    let verses = h.hierarchy();
    assert_eq!(
        find_petal(&verses, &ids.petal),
        Some(1500.0),
        "HierarchyLoaded carries the column (crud.rs loader)"
    );
}

#[test]
fn clearing_restores_inherit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let h = Harness::open(dir.path());
    let ids = h.create_hierarchy();
    h.set_distance(&ids.petal, Some(2000.0), editor());
    h.set_distance(&ids.petal, None, editor());
    assert_eq!(h.get_distance(&ids.petal), None);
    let verses = h.hierarchy();
    assert_eq!(find_petal(&verses, &ids.petal), None);
}

#[test]
fn a_viewer_cannot_set_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let h = Harness::open(dir.path());
    let ids = h.create_hierarchy();
    match h.set_distance(&ids.petal, Some(500.0), viewer()) {
        DbResult::Error(_) | DbResult::RequestFailed { .. } => {}
        o => panic!("viewer must be denied, got {o:?}"),
    }
    assert_eq!(h.get_distance(&ids.petal), None, "row untouched");
}

#[test]
fn out_of_range_values_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let h = Harness::open(dir.path());
    let ids = h.create_hierarchy();
    for bad in [f32::NAN, -1.0, 50_001.0] {
        match h.set_distance(&ids.petal, Some(bad), editor()) {
            DbResult::Error(_) | DbResult::RequestFailed { .. } => {}
            o => panic!("{bad} must be refused, got {o:?}"),
        }
    }
    assert_eq!(h.get_distance(&ids.petal), None, "no bad value ever lands");
}
