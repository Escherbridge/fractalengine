//! OF-2: the DB thread is the only SurrealKV opener; in-process readers (the
//! API gateway) use the clone `spawn_db_thread_shared` hands back — never a
//! second open, which the store lock refuses (Windows os error 33). Driven
//! through the real DB thread over an on-disk store. See src/AGENTS.md
//! §shared-handle. Own binary (process-global HLC).

use fe_database::{repo, spawn_db_thread_shared, BlobStoreHandle, DbThread};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

struct Spawned {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    db_thread: DbThread,
}

fn spawn_on(dir: &Path) -> Spawned {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
    let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
    let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
    let db_path = dir.join("fe.db").to_string_lossy().to_string();
    let db_thread = spawn_db_thread_shared(
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
    .expect("spawn db thread on the temp store");
    let first = res_rx.recv_timeout(TIMEOUT).expect("first DB result");
    assert!(
        matches!(first, DbResult::Started),
        "first result is Started, got {first:?}"
    );
    Spawned {
        cmd_tx,
        res_rx,
        db_thread,
    }
}

/// Query verse rows on a separate thread + runtime (the API gateway's shape),
/// bounded so a stalled router fails the test instead of hanging it.
fn read_verse_on_foreign_runtime(
    reader: repo::Db,
    verse_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let (answer_tx, answer_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("reader runtime");
        let answer = runtime.block_on(async move {
            let mut response = reader
                .query("SELECT verse_id, name FROM verse WHERE verse_id = $id")
                .bind(("id", verse_id))
                .await
                .map_err(|error| error.to_string())?;
            response
                .take::<Vec<serde_json::Value>>(0)
                .map_err(|error| error.to_string())
        });
        let _ = answer_tx.send(answer);
    });
    answer_rx
        .recv_timeout(TIMEOUT)
        .expect("shared reader answered instead of stalling")
}

#[test]
fn api_reader_reads_db_thread_writes_through_the_shared_handle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_on(dir.path());

    spawned
        .cmd_tx
        .send(DbCommand::CreateVerse {
            name: "Shared Verse".into(),
        })
        .expect("send CreateVerse");
    let verse_id = match spawned.res_rx.recv_timeout(TIMEOUT).expect("create reply") {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    // The DB thread is now parked in its blocking command recv; the reader
    // must still be served (its router lives on the DB runtime's workers).
    let rows = read_verse_on_foreign_runtime(spawned.db_thread.shared_db.clone(), verse_id.clone())
        .expect("shared reader query succeeds — no second open, no os error 33");
    assert_eq!(rows.len(), 1, "exactly the created verse: {rows:?}");
    assert_eq!(rows[0]["verse_id"], verse_id.as_str());
    assert_eq!(rows[0]["name"], "Shared Verse");
}

#[test]
fn shared_reader_errors_instead_of_hanging_after_db_thread_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_on(dir.path());
    let lingering_reader = spawned.db_thread.shared_db.clone();
    // Non-vacuous: the same clone answers while the DB thread lives, so the
    // error below can only come from the thread having exited.
    let live = read_verse_on_foreign_runtime(lingering_reader.clone(), "any".into());
    assert_eq!(live, Ok(Vec::new()), "clone serves reads before shutdown");

    spawned
        .cmd_tx
        .send(DbCommand::Shutdown)
        .expect("send Shutdown");
    let stopped = spawned.res_rx.recv_timeout(TIMEOUT).expect("stop reply");
    assert!(matches!(stopped, DbResult::Stopped), "got {stopped:?}");
    spawned
        .db_thread
        .join_handle
        .join()
        .expect("DB thread exits cleanly");

    // The engine died with the DB runtime: a reader outliving it (relay
    // graceful shutdown while the API thread still serves) gets an error,
    // not a hang.
    let outcome = read_verse_on_foreign_runtime(lingering_reader, "any".into());
    assert!(
        outcome.is_err(),
        "reader must error once the DB thread is gone, got {outcome:?}"
    );
}
