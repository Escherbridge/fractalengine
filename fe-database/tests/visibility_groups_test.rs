//! Durable-path tests for the petal visibility-Group registry and the
//! persisted verse/fractal/petal eyes (`SetPetalVisibilityGroups` /
//! `GetPetalVisibilityGroups` / `SetScopeHidden`, hierarchy_visibility_groups
//! Phase 4): Editor+ authorization, exact round-trips, hydration through
//! `HierarchyLoaded`, and survival across a DB-thread restart on the same
//! on-disk store. Own binary (process-global HLC).

use fe_database::{spawn_db_thread_with_sync_and_lifecycle, BlobStoreHandle};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{
    CallerAuth, DbCommand, DbResult, ScopeLevel, VerseHierarchyData, VisibilityGroupDto,
};
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
    verse: String,
    fractal: String,
    petal: String,
}

/// A DB thread over a store directory the TEST owns; `Drop` joins the thread
/// so a second `Harness` can reopen the same files.
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
        Ids {
            verse,
            fractal,
            petal,
        }
    }

    fn set_groups(
        &self,
        petal_id: &str,
        groups: Vec<VisibilityGroupDto>,
        auth: CallerAuth,
    ) -> DbResult {
        self.ask(DbCommand::SetPetalVisibilityGroups {
            petal_id: petal_id.into(),
            groups,
            auth,
            correlation_id: None,
        })
    }

    fn groups(&self, petal_id: &str) -> Vec<VisibilityGroupDto> {
        match self.ask(DbCommand::GetPetalVisibilityGroups {
            petal_id: petal_id.into(),
            correlation_id: None,
        }) {
            DbResult::PetalVisibilityGroupsLoaded { groups, .. } => groups,
            o => panic!("expected PetalVisibilityGroupsLoaded, got {o:?}"),
        }
    }

    fn set_hidden(&self, level: ScopeLevel, id: &str, hidden: bool, auth: CallerAuth) -> DbResult {
        self.ask(DbCommand::SetScopeHidden {
            level,
            id: id.into(),
            hidden,
            auth,
            correlation_id: None,
        })
    }

    fn hierarchy(&self) -> Vec<VerseHierarchyData> {
        loop {
            self.send(DbCommand::LoadHierarchy);
            match self.recv() {
                DbResult::HierarchyLoaded { verses } => return verses,
                // A seeded store may interleave unrelated results; keep asking.
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

fn registry() -> Vec<VisibilityGroupDto> {
    vec![
        VisibilityGroupDto {
            id: "utilities".into(),
            name: "Utilities".into(),
            visible: false,
            order: 0.0,
        },
        VisibilityGroupDto {
            id: "phase-2".into(),
            name: "Phase 2".into(),
            visible: true,
            order: 1.5,
        },
    ]
}

/// `(verse, fractal, petal)` view_hidden flags for `ids` in a loaded hierarchy.
fn hidden_flags(verses: &[VerseHierarchyData], ids: &Ids) -> (bool, bool, bool) {
    let verse = verses
        .iter()
        .find(|v| v.id == ids.verse)
        .expect("verse in tree");
    let fractal = verse
        .fractals
        .iter()
        .find(|f| f.id == ids.fractal)
        .expect("fractal in tree");
    let petal = fractal
        .petals
        .iter()
        .find(|p| p.id == ids.petal)
        .expect("petal in tree");
    (verse.view_hidden, fractal.view_hidden, petal.view_hidden)
}

#[test]
fn the_group_registry_is_editor_gated_round_trips_and_survives_a_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let petal = {
        let h = Harness::open(dir.path());
        let ids = h.create_hierarchy();
        assert!(
            h.groups(&ids.petal).is_empty(),
            "a fresh petal has no groups"
        );

        // A Viewer cannot write the registry — and nothing is stored.
        match h.set_groups(&ids.petal, registry(), viewer()) {
            DbResult::Error(e) => assert!(e.contains("denied"), "viewer denied: {e}"),
            o => panic!("viewer SetPetalVisibilityGroups must be denied, got {o:?}"),
        }
        assert!(
            h.groups(&ids.petal).is_empty(),
            "a denied write stores nothing"
        );

        // An Editor can; the echo is the stored registry.
        match h.set_groups(&ids.petal, registry(), editor()) {
            DbResult::PetalVisibilityGroupsLoaded {
                petal_id, groups, ..
            } => {
                assert_eq!(petal_id, ids.petal);
                assert_eq!(groups, registry());
            }
            o => panic!("editor SetPetalVisibilityGroups must succeed, got {o:?}"),
        }
        assert_eq!(h.groups(&ids.petal), registry(), "Get round-trips exactly");
        ids.petal
    };

    let h = Harness::open(dir.path());
    assert_eq!(
        h.groups(&petal),
        registry(),
        "the registry survives a restart"
    );

    // Whole-registry replace: an empty list clears it.
    assert!(matches!(
        h.set_groups(&petal, Vec::new(), editor()),
        DbResult::PetalVisibilityGroupsLoaded { ref groups, .. } if groups.is_empty()
    ));
    assert!(h.groups(&petal).is_empty());
}

#[test]
fn scope_eyes_persist_across_a_restart_and_hydrate_the_hierarchy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ids = {
        let h = Harness::open(dir.path());
        let ids = h.create_hierarchy();
        assert_eq!(hidden_flags(&h.hierarchy(), &ids), (false, false, false));

        match h.set_hidden(ScopeLevel::Fractal, &ids.fractal, true, viewer()) {
            DbResult::Error(e) => assert!(e.contains("denied"), "viewer denied: {e}"),
            o => panic!("viewer SetScopeHidden must be denied, got {o:?}"),
        }
        for (level, id) in [
            (ScopeLevel::Verse, &ids.verse),
            (ScopeLevel::Fractal, &ids.fractal),
            (ScopeLevel::Petal, &ids.petal),
        ] {
            match h.set_hidden(level, id, true, editor()) {
                DbResult::ScopeHiddenSet {
                    level: echoed,
                    id: echoed_id,
                    hidden,
                    ..
                } => assert_eq!(
                    (echoed, echoed_id.as_str(), hidden),
                    (level, id.as_str(), true)
                ),
                o => panic!("editor SetScopeHidden({level:?}) must succeed, got {o:?}"),
            }
        }
        ids
    };

    let h = Harness::open(dir.path());
    assert_eq!(
        hidden_flags(&h.hierarchy(), &ids),
        (true, true, true),
        "every eye survives the restart and reaches HierarchyLoaded"
    );
    assert!(matches!(
        h.set_hidden(ScopeLevel::Fractal, &ids.fractal, false, editor()),
        DbResult::ScopeHiddenSet { hidden: false, .. }
    ));
    assert_eq!(hidden_flags(&h.hierarchy(), &ids), (true, false, true));
}

#[test]
fn a_missing_petal_or_scope_is_a_failure_not_an_empty_answer() {
    let dir = tempfile::tempdir().expect("temp dir");
    let h = Harness::open(dir.path());
    let ghost = "01HZZZZZZZZZZZZZZZZZZZZZZZ";

    assert!(matches!(
        h.set_groups(ghost, registry(), editor()),
        DbResult::Error(_)
    ));
    assert!(matches!(
        h.ask(DbCommand::GetPetalVisibilityGroups {
            petal_id: ghost.into(),
            correlation_id: None,
        }),
        DbResult::Error(_)
    ));
    // A correlated (API) caller gets its typed failure echoing the id.
    match h.ask(DbCommand::GetPetalVisibilityGroups {
        petal_id: ghost.into(),
        correlation_id: Some("api-1".into()),
    }) {
        DbResult::RequestFailed { correlation_id, .. } => assert_eq!(correlation_id, "api-1"),
        o => panic!("expected RequestFailed, got {o:?}"),
    }
    for level in [ScopeLevel::Verse, ScopeLevel::Fractal, ScopeLevel::Petal] {
        assert!(
            matches!(
                h.set_hidden(level, ghost, true, editor()),
                DbResult::Error(_)
            ),
            "{level:?} eye on a missing row must fail"
        );
    }
    // Validation runs before anything else: a duplicate id never writes.
    let ids = h.create_hierarchy();
    let mut dup = registry();
    dup[1].id = dup[0].id.clone();
    match h.set_groups(&ids.petal, dup, editor()) {
        DbResult::Error(e) => assert!(e.contains("rejected"), "{e}"),
        o => panic!("a duplicate-id registry must be rejected, got {o:?}"),
    }
    assert!(h.groups(&ids.petal).is_empty());
}
