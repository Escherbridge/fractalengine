//! D-CL20 per-kind bypass counters (CDL-W4 slice W4-1): each of the six
//! D-CL20-gated mutation handlers calls `record_bypass_for(<its kind>)` as the
//! FIRST statement in its handler body, strictly additive to the legacy path —
//! no `OpLogEntry`, no `commit_operation_dual_emit`, no invented canonical
//! payload (see `fe-database/src/migration/AGENTS.md` §bypass-counter). Own
//! binary (own process) so the process-global `AtomicU64` counters this test
//! reads are never shared with another test binary's assertions — see
//! `fe-database/src/migration/mapping_inventory.rs`'s `D_CL20_BYPASS_COUNTS`.
//!
//! The binary's tests share those process-global counters and libtest runs them on parallel
//! threads, so each test holds [`serial`] for its whole body: a concurrent test's hierarchy
//! creates would otherwise move the "other five did not move" slots mid-assertion.
//!
//! Harness pattern follows `fe-database/tests/node_lifecycle_test.rs:30-60`
//! (real DB thread + `DbCommand` public API over an on-disk SurrealDB), with
//! `spawn_db_thread_shared` (as `fe-database/tests/shared_handle_test.rs`
//! uses) so the test can also read `op_log`'s row count directly through the
//! shared datastore handle, to prove the counter added no op-log row.

use fe_database::migration::mapping_inventory::{bypass_count_for, D_CL20_MUTATION_KINDS};
use fe_database::{spawn_db_thread_shared, BlobStoreHandle, DbThread};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Serializes this binary's tests over the process-global counters they assert on.
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static COUNTERS: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    COUNTERS
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// The local desktop session (N-5): the harness's local user owns every verse
/// it creates, so `Local` resolves to Owner/Editor+ and is authorized for the
/// Editor+-gated `RenameNode`/`DuplicateNode` commands below (mirrors
/// `node_lifecycle_test.rs`'s `local()`).
fn local() -> CallerAuth {
    CallerAuth::Local
}

/// A minimal valid GLB (glTF Binary), byte-identical in shape to
/// `fe-test-harness/src/fixtures/mod.rs::create_minimal_glb` (inlined here
/// rather than taking a new dev-dependency on fe-test-harness, which is
/// outside this slice's owned files — see campaign plan W4-1). Non-KTX2, no
/// textures/images/extensions, so `fe_runtime::ktx2_guard::check_asset_bytes`
/// (which `import_gltf_handler` runs before the bypass path) passes it
/// (critic tick 5, C8-iii: the fixture must be a non-KTX2 GLB).
fn minimal_glb_bytes() -> Vec<u8> {
    let json = br#"{"asset":{"version":"2.0"}}"#;
    let json_padded_len = (json.len() + 3) & !3;
    let total_len = 12 + 8 + json_padded_len;
    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(b"glTF");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&(total_len as u32).to_le_bytes());
    buf.extend_from_slice(&(json_padded_len as u32).to_le_bytes());
    buf.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    buf.extend_from_slice(json);
    buf.resize(total_len, 0x20);
    buf
}

/// A running DB thread + its command/result channels + a clone of its
/// datastore handle (`spawn_db_thread_shared`, `fe-database/tests/shared_handle_test.rs`
/// pattern), over a temp on-disk DB. `Drop` closes the command channel
/// (breaking the DB loop) and joins the thread *before* the temp dir is
/// removed, releasing the SurrealKV file lock first (one-open-per-process,
/// `558197c`).
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    db_thread: Option<DbThread>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn start() -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("fe.db").to_string_lossy().to_string();

        // `spawn_db_thread_shared` refuses to run inside a Tokio runtime (it owns its own);
        // these tests are `#[tokio::test]`, so spawn it from a plain OS thread.
        let db_thread = std::thread::spawn(move || {
            spawn_db_thread_shared(
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
            .map_err(|e| format!("{e:?}"))
        })
        .join()
        .expect("db spawner thread")
        .expect("spawn db thread");

        let h = Harness {
            cmd_tx: Some(cmd_tx),
            res_rx,
            db_thread: Some(db_thread),
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

    /// Current `op_log` row count, read directly off the shared datastore
    /// handle (no second SurrealKV open — see `shared_handle_test.rs`).
    async fn op_log_count(&self) -> usize {
        let rows: Vec<serde_json::Value> = self
            .db_thread
            .as_ref()
            .unwrap()
            .shared_db
            .query("SELECT count() AS n FROM op_log GROUP ALL")
            .await
            .expect("count op_log")
            .take(0)
            .expect("count rows");
        rows.first().and_then(|row| row["n"].as_u64()).unwrap_or(0) as usize
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.cmd_tx.take()); // close the channel → the DB loop breaks
        if let Some(db_thread) = self.db_thread.take() {
            let _ = db_thread.join_handle.join();
        }
        // `_dir` is removed after this, once the file lock is released.
    }
}

/// Every D-CL20 kind's current bypass count, in `D_CL20_MUTATION_KINDS` order —
/// a cheap snapshot to diff "only this kind moved, the other five didn't".
fn snapshot_counts() -> [u64; 6] {
    let mut counts = [0u64; 6];
    for (i, kind) in D_CL20_MUTATION_KINDS.iter().enumerate() {
        counts[i] = bypass_count_for(kind);
    }
    counts
}

/// Assert that exactly `kind`'s slot rose by 1 between `before` and `after`,
/// and every other slot is unchanged. Mutation check: deleting the
/// `record_bypass_for` call for `kind` in its handler turns this red (the
/// "own kind" delta would read 0, not 1).
fn assert_only_this_kind_bypassed(before: [u64; 6], after: [u64; 6], kind: &str) {
    let index = D_CL20_MUTATION_KINDS
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_else(|| panic!("{kind} is not a D_CL20_MUTATION_KINDS entry"));
    for (i, k) in D_CL20_MUTATION_KINDS.iter().enumerate() {
        let delta = after[i] as i64 - before[i] as i64;
        if i == index {
            assert_eq!(delta, 1, "{k}'s own bypass count must rise by exactly 1");
        } else {
            assert_eq!(
                delta, 0,
                "{k}'s bypass count must not move for a {kind} command"
            );
        }
    }
}

#[tokio::test]
async fn each_dcl20_mutation_records_exactly_one_bypass_of_its_own_kind() {
    let _serial = serial().await;
    let h = Harness::start();

    // --- CreateVerse -------------------------------------------------------
    let before = snapshot_counts();
    let op_log_before = h.op_log_count().await;
    h.send(DbCommand::CreateVerse { name: "V".into() });
    let verse_id = match h.recv() {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };
    assert_only_this_kind_bypassed(before, snapshot_counts(), "CreateVerse");
    assert_eq!(
        h.op_log_count().await,
        op_log_before,
        "CreateVerse: the bypass counter added no op_log row"
    );

    // --- CreateFractal -------------------------------------------------------
    let before = snapshot_counts();
    let op_log_before = h.op_log_count().await;
    h.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "F".into(),
    });
    let fractal_id = match h.recv() {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };
    assert_only_this_kind_bypassed(before, snapshot_counts(), "CreateFractal");
    assert_eq!(
        h.op_log_count().await,
        op_log_before,
        "CreateFractal: the bypass counter added no op_log row"
    );

    // --- CreatePetal -------------------------------------------------------
    let before = snapshot_counts();
    let op_log_before = h.op_log_count().await;
    h.send(DbCommand::CreatePetal {
        fractal_id: fractal_id.clone(),
        name: "P".into(),
    });
    let petal_id = match h.recv() {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    };
    assert_only_this_kind_bypassed(before, snapshot_counts(), "CreatePetal");
    assert_eq!(
        h.op_log_count().await,
        op_log_before,
        "CreatePetal: the bypass counter added no op_log row"
    );

    // --- ImportGltf -------------------------------------------------------
    let dir = tempfile::tempdir().expect("glb temp dir");
    let glb_path = dir.path().join("fixture.glb");
    std::fs::write(&glb_path, minimal_glb_bytes()).expect("write glb fixture");
    let before = snapshot_counts();
    let op_log_before = h.op_log_count().await;
    h.send(DbCommand::ImportGltf {
        file_lease: None,
        petal_id: petal_id.clone(),
        name: "Imported".into(),
        file_path: glb_path.to_str().unwrap().into(),
        position: [0.0, 0.0, 0.0],
    });
    let imported_node_id = match h.recv() {
        DbResult::GltfImported { node_id, .. } => node_id,
        other => panic!("expected GltfImported, got {other:?}"),
    };
    assert_only_this_kind_bypassed(before, snapshot_counts(), "ImportGltf");
    assert_eq!(
        h.op_log_count().await,
        op_log_before,
        "ImportGltf: the bypass counter added no op_log row"
    );

    // --- RenameNode -------------------------------------------------------
    let before = snapshot_counts();
    let op_log_before = h.op_log_count().await;
    h.send(DbCommand::RenameNode {
        node_id: imported_node_id.clone(),
        new_name: "Renamed".into(),
        auth: local(),
    });
    match h.recv() {
        DbResult::NodeRenamed { node_id, new_name } => {
            assert_eq!(node_id, imported_node_id);
            assert_eq!(new_name, "Renamed");
        }
        other => panic!("expected NodeRenamed, got {other:?}"),
    }
    assert_only_this_kind_bypassed(before, snapshot_counts(), "RenameNode");
    assert_eq!(
        h.op_log_count().await,
        op_log_before,
        "RenameNode: the bypass counter added no op_log row"
    );

    // --- DuplicateNode -------------------------------------------------------
    let before = snapshot_counts();
    let op_log_before = h.op_log_count().await;
    h.send(DbCommand::DuplicateNode {
        node_id: imported_node_id.clone(),
        auth: local(),
    });
    match h.recv() {
        DbResult::NodeCreated { id, .. } => {
            assert_ne!(id, imported_node_id, "duplicate gets a fresh node id");
        }
        other => panic!("expected NodeCreated (duplicate), got {other:?}"),
    }
    assert_only_this_kind_bypassed(before, snapshot_counts(), "DuplicateNode");
    assert_eq!(
        h.op_log_count().await,
        op_log_before,
        "DuplicateNode: the bypass counter added no op_log row"
    );
}

/// Unhappy path (reachable, consequential): `record_bypass_for("RenameNode")`
/// is the FIRST statement in `rename_node_handler`, before the
/// `tombstone = NONE` lookup — so a rename attempt on an already-tombstoned
/// node still counts as a bypass attempt, and the handler's original error
/// text (unchanged by this slice, `git diff` shows 0 `-` lines) still reaches
/// the caller. Proves "first statement" and "error text unchanged" together.
#[tokio::test]
async fn a_rename_on_a_tombstoned_node_still_records_the_bypass_and_keeps_the_error_text() {
    let _serial = serial().await;
    let h = Harness::start();

    h.send(DbCommand::CreateVerse { name: "V".into() });
    let verse_id = match h.recv() {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };
    h.send(DbCommand::CreateFractal {
        verse_id,
        name: "F".into(),
    });
    let fractal_id = match h.recv() {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };
    h.send(DbCommand::CreatePetal {
        fractal_id,
        name: "P".into(),
    });
    let petal_id = match h.recv() {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    };
    h.send(DbCommand::CreateNode {
        petal_id,
        name: "N".into(),
        position: [0.0, 0.0, 0.0],
        correlation_id: None,
    });
    let node_id = match h.recv() {
        DbResult::NodeCreated { id, .. } => id,
        other => panic!("expected NodeCreated, got {other:?}"),
    };

    h.send(DbCommand::TombstoneNode {
        node_id: node_id.clone(),
        auth: local(),
    });
    match h.recv() {
        DbResult::NodeDeleted {
            node_id: deleted, ..
        } => assert_eq!(deleted, node_id),
        other => panic!("expected NodeDeleted (tombstone), got {other:?}"),
    }

    let before = bypass_count_for("RenameNode");
    h.send(DbCommand::RenameNode {
        node_id: node_id.clone(),
        new_name: "WontStick".into(),
        auth: local(),
    });
    match h.recv() {
        DbResult::Error(e) => {
            assert!(
                e.contains(&format!(
                    "RenameNode matched no live node with node_id = {node_id}"
                )),
                "original error text must survive unchanged, got: {e}"
            );
        }
        other => panic!("expected Error for a rename of a tombstoned node, got {other:?}"),
    }
    assert_eq!(
        bypass_count_for("RenameNode"),
        before + 1,
        "the bypass fires even though the rename itself is refused downstream \
         (record_bypass_for precedes the tombstone lookup)"
    );
}

/// A GLB whose sole image declares `mimeType: "image/ktx2"`: the shape
/// `fe_runtime::ktx2_guard::check_glb` refuses (mirrors `handlers/crud.rs`'s
/// `import_gltf_ktx2_guard_tests::glb_with_ktx2_image`).
fn ktx2_glb_bytes() -> Vec<u8> {
    let json = br#"{"images":[{"bufferView":0,"mimeType":"image/ktx2"}],"bufferViews":[{"buffer":0,"byteLength":4}]}"#;
    let mut json_bytes = json.to_vec();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let bin = [0u8; 4];
    let mut body = Vec::new();
    body.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    body.extend_from_slice(b"JSON");
    body.extend_from_slice(&json_bytes);
    body.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    body.extend_from_slice(b"BIN\0");
    body.extend_from_slice(&bin);
    let mut glb = b"glTF".to_vec();
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&((12 + body.len()) as u32).to_le_bytes());
    glb.extend_from_slice(&body);
    glb
}

/// Plan C8(iii)/C10(b): an import the KTX2 guard refuses still counts as an
/// `ImportGltf` bypass attempt, because `record_bypass_for` precedes the guard;
/// the refusal itself is unchanged (an error naming the guard, no node created).
/// Mutation check: moving the counter line below the guard turns the count red.
#[tokio::test]
async fn a_ktx2_refused_import_still_records_its_bypass() {
    let _serial = serial().await;
    let h = Harness::start();

    h.send(DbCommand::CreateVerse { name: "V".into() });
    let verse_id = match h.recv() {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };
    h.send(DbCommand::CreateFractal {
        verse_id,
        name: "F".into(),
    });
    let fractal_id = match h.recv() {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };
    h.send(DbCommand::CreatePetal {
        fractal_id,
        name: "P".into(),
    });
    let petal_id = match h.recv() {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    };

    let dir = tempfile::tempdir().expect("glb temp dir");
    let glb_path = dir.path().join("ktx2.glb");
    std::fs::write(&glb_path, ktx2_glb_bytes()).expect("write ktx2 glb fixture");
    let before = snapshot_counts();
    h.send(DbCommand::ImportGltf {
        file_lease: None,
        petal_id,
        name: "Refused".into(),
        file_path: glb_path.to_str().unwrap().into(),
        position: [0.0, 0.0, 0.0],
    });
    match h.recv() {
        // Critic tick 7 (AC-35h): the fixture file is named `ktx2.glb` and
        // the error message quotes the path, so the `ktx2` half of this OR
        // passed for ANY refusal of this file, not only the RUSTSEC one —
        // narrowed to the one assertion that actually pins the guard.
        DbResult::Error(e) => assert!(
            e.contains("RUSTSEC-2026-0344"),
            "the refusal must name the KTX2 guard's advisory, got: {e}"
        ),
        other => panic!("expected the KTX2 guard's Error, got {other:?}"),
    }
    assert_only_this_kind_bypassed(before, snapshot_counts(), "ImportGltf");
}
