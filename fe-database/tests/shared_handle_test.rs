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
    spawn_on_with_identity(dir, None)
}

fn spawn_on_with_identity(dir: &Path, identity: Option<fe_identity::NodeKeypair>) -> Spawned {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
    let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
    let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
    let db_path = dir.join("fe.db").to_string_lossy().to_string();
    let db_thread = spawn_db_thread_shared(
        cmd_rx,
        res_tx,
        blob_store,
        None,
        identity,
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

    // Persist a schema-valid but inadmissible clock, then exercise the real restart handshake.
    let reader = spawned.db_thread.shared_db.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            reader.query("CREATE node_scene_clock CONTENT {node_id:'invalid-startup-clock',hlc:9223372036854775807,publisher:'did:qa',row_fingerprint:'fixture'}")
                .await.unwrap().check().unwrap();
        });
    }).join().unwrap();
    spawned.cmd_tx.send(DbCommand::Shutdown).unwrap();
    assert!(matches!(
        spawned.res_rx.recv_timeout(TIMEOUT).unwrap(),
        DbResult::Stopped
    ));
    spawned.db_thread.join_handle.join().unwrap();
    drop(spawned.db_thread.shared_db);
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(1);
    let (res_tx, res_rx) = crossbeam::channel::bounded(1);
    let failed = spawn_db_thread_shared(
        cmd_rx,
        res_tx,
        Arc::new(MockBlobStore::new()),
        None,
        None,
        None,
        None,
        Some(dir.path().join("fe.db").to_string_lossy().into_owned()),
        None,
    );
    match failed {
        Err(fe_database::DbInitError::SceneClockRecovery(error)) => assert!(!error.is_empty()),
        Err(other) => panic!("expected scene-clock startup refusal, got {other}"),
        Ok(thread) => {
            drop(cmd_tx);
            thread.join_handle.join().unwrap();
            panic!("startup returned a usable shared handle before fatal clock recovery");
        }
    }
    assert!(
        !res_rx
            .try_iter()
            .any(|result| matches!(result, DbResult::Started)),
        "a refused startup never announces Started"
    );
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

    let identity = fe_identity::NodeKeypair::generate();
    let terminal = spawn_on_with_identity(dir.path(), Some(fe_identity::NodeKeypair::from_bytes(&identity.seed_bytes()).unwrap()));
    terminal.cmd_tx.send(DbCommand::CreateVerse { name: "before-terminal".into() }).unwrap();
    let DbResult::VerseCreated {id:before_verse,..}=terminal.res_rx.recv_timeout(TIMEOUT).unwrap() else {panic!("created owned verse")};
    let auth_runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let previous_generation=auth_runtime.block_on(fe_database::api_auth::current_generation(&terminal.db_thread.shared_db)).unwrap();
    let old_owner_jti=fe_database::api_auth::new_jti(&previous_generation).unwrap();
    let old_owner=fe_identity::mint_api_token(&identity,&format!("VERSE#{before_verse}"),"owner",3600,&old_owner_jti).unwrap();
    terminal.cmd_tx.send(DbCommand::MintApiToken {scope:format!("VERSE#{before_verse}"),max_role:"viewer".into(),ttl_hours:1,label:None,client_did:None,correlation_id:None}).unwrap();
    let DbResult::ApiTokenMinted {jti:revoked_jti,..}=terminal.res_rx.recv_timeout(TIMEOUT).unwrap() else {panic!("persisted token mint")};
    terminal.cmd_tx.send(DbCommand::RevokeApiToken {jti:revoked_jti.clone(),sub:identity.to_did_key()}).unwrap();
    assert!(matches!(terminal.res_rx.recv_timeout(TIMEOUT).unwrap(),DbResult::ApiTokenRevoked {..}));
    terminal.cmd_tx.send(DbCommand::Shutdown).unwrap();
    assert!(matches!(terminal.res_rx.recv_timeout(TIMEOUT).unwrap(),DbResult::Stopped));
    terminal.db_thread.join_handle.join().unwrap();
    drop(terminal.db_thread.shared_db);
    let terminal=spawn_on_with_identity(dir.path(),Some(fe_identity::NodeKeypair::from_bytes(&identity.seed_bytes()).unwrap()));
    assert!(auth_runtime.block_on(fe_database::api_token_store::is_token_revoked(&terminal.db_thread.shared_db,&revoked_jti)).unwrap(),"Ordinary revocation survives closing and reopening the real store");
    assert_eq!(auth_runtime.block_on(fe_database::api_auth::current_generation(&terminal.db_thread.shared_db)).unwrap(),previous_generation,"Ordinary restart does not invalidate unrelated credentials");
    let reader = terminal.db_thread.shared_db.clone();
    let jobs = fe_runtime::maintenance::HostJobs::default();
    jobs.freeze().unwrap();
    let mut services: Vec<_> = ["api", "sync", "network", "plugin", "replication", "distributed-query", "scene-bridge", "transform-bridge", "listener-fanout"]
        .into_iter().map(|name| fe_runtime::service_lifecycle::OwnedService::spawn(name, |stop| {
            while !stop.is_requested() { std::thread::sleep(Duration::from_millis(1)); }
        })).collect();
    assert!(fe_runtime::maintenance::QuiescenceProof::after_joined(&mut services, &jobs).is_err(), "a running service cannot issue reset proof");
    for service in &services { service.request_stop(); }
    let deadline = std::time::Instant::now() + TIMEOUT;
    let proof = loop {
        if let Some(proof) = fe_runtime::maintenance::QuiescenceProof::after_joined(&mut services, &jobs).unwrap() { break proof; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let replay = proof.clone();
    for _ in 0..65 { terminal.cmd_tx.send(DbCommand::Ping).unwrap(); }
    terminal.cmd_tx.send(DbCommand::TerminalDevelopmentReset {
        request_id: "real-worker-reset".into(), confirmation: "RESET".into(), proof,
    }).unwrap();
    terminal.cmd_tx.send(DbCommand::CreateVerse { name: "late-after-terminal".into() }).unwrap();
    let mut observed = false;
    let mut drained = 0;
    loop {
        match terminal.res_rx.recv_timeout(TIMEOUT).unwrap() {
            DbResult::Pong => drained += 1,
            DbResult::TerminalDevelopmentReset { request_id, cleared, error } => {
                assert_eq!(request_id, "real-worker-reset");
                assert!(cleared, "{error:?}");
                assert!(error.is_none(), "{error:?}");
                assert!(!observed, "one terminal receipt only");
                observed = true;
            }
            DbResult::Stopped => break,
            DbResult::DatabaseReset { .. } => panic!("terminal reset must not emit the reload-producing legacy result"),
            other => panic!("unexpected post-barrier result: {other:?}"),
        }
    }
    assert!(observed);
    assert_eq!(drained, 65, "draining bounded results must let the final DB barrier run");
    terminal.db_thread.join_handle.join().expect("terminal DB worker joins");
    assert!(terminal.cmd_tx.send(DbCommand::Ping).is_err());
    assert!(read_verse_on_foreign_runtime(reader, "any".into()).is_err());
    drop(terminal.db_thread.shared_db);

    let reopened = spawn_on_with_identity(dir.path(), Some(fe_identity::NodeKeypair::from_bytes(&identity.seed_bytes()).unwrap()));
    reopened.cmd_tx.send(DbCommand::LoadHierarchy).unwrap();
    match reopened.res_rx.recv_timeout(TIMEOUT).unwrap() {
        DbResult::HierarchyLoaded { verses } => {
            assert!(!verses.is_empty(), "default seed is durable after reopen");
            assert!(verses.iter().all(|verse| verse.name != "before-terminal" && verse.name != "late-after-terminal"));
        }
        other => panic!("expected reopened hierarchy, got {other:?}"),
    }
    let current_generation=auth_runtime.block_on(fe_database::api_auth::current_generation(&reopened.db_thread.shared_db)).unwrap();
    assert_ne!(current_generation,previous_generation,"Terminal reset durably advances authentication generation");
    assert!(!fe_database::api_auth::accepts_jti(&current_generation,&old_owner_jti),"Old Owner JWT cannot regain authority after reopen");
    assert!(fe_identity::verify_api_token(&old_owner,&identity.verifying_key()).is_ok(),"Reset retires credential authority without rotating the signing key");
    reopened.cmd_tx.send(DbCommand::CreateVerse {name:"fresh-auth-generation".into()}).unwrap();
    let DbResult::VerseCreated {id:fresh_verse,..}=reopened.res_rx.recv_timeout(TIMEOUT).unwrap() else {panic!("fresh owned verse")};
    reopened.cmd_tx.send(DbCommand::MintApiToken {scope:format!("VERSE#{fresh_verse}"),max_role:"viewer".into(),ttl_hours:1,label:None,client_did:None,correlation_id:None}).unwrap();
    let DbResult::ApiTokenMinted {token,jti,..}=reopened.res_rx.recv_timeout(TIMEOUT).unwrap() else {panic!("fresh token mint")};
    assert!(fe_database::api_auth::accepts_jti(&current_generation,&jti),"New real DB-thread mint uses reopened generation");
    assert_eq!(fe_identity::verify_api_token(&token,&identity.verifying_key()).unwrap().jti,jti);
    reopened.cmd_tx.send(DbCommand::TerminalDevelopmentReset {
        request_id: "replayed-proof".into(), confirmation: "RESET".into(), proof: replay,
    }).unwrap();
    match reopened.res_rx.recv_timeout(TIMEOUT).unwrap() {
        DbResult::TerminalDevelopmentReset { cleared, error, .. } => {
            assert!(!cleared);
            assert!(error.unwrap().contains("already consumed"));
        }
        other => panic!("expected refused proof replay, got {other:?}"),
    }
    assert!(matches!(reopened.res_rx.recv_timeout(TIMEOUT).unwrap(), DbResult::Stopped));
    reopened.db_thread.join_handle.join().unwrap();
}
