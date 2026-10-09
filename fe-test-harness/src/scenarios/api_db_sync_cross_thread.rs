//! Scenario: API↔DB↔Sync cross-thread (F14/T4 — api_mcp_integration_tests
//! FR-5's "cross-thread scenarios" shape).
//!
//! Every other consumer of `fe-api` in this workspace talks to an `ApiState`
//! wired to either a mock DB-command emulator (`fe-api/tests/mcp_integration.rs`)
//! or an in-memory `db_reader` (`ApiHarness`). Neither exercises the real
//! seam this scenario targets: a **real** `ApiState`, with NO `db_reader`,
//! bridged over its `api_cmd_tx` crossbeam channel to a **real** `TestPeer`'s
//! DB thread — the exact fallback path `fe-api/src/iot.rs::ingest_readings`
//! takes on a platform where a second SurrealKV file handle isn't available
//! (F24) — and that peer's **real** sync thread, which auto-publishes the
//! write to a second peer over real (loopback) iroh transport.
//!
//! Flow: REST IoT ingest (API, channel-bridged) → alice's DB thread applies
//! and emits a `ReplicationEvent` (the same seam
//! `insert_readings_with_replication` already uses) → alice's sync thread
//! publishes it → bob (joined to the same verse replica) observes
//! `SyncEvent::RowApplied` and his own DB thread applies it → READ-BACK from
//! bob's durable store proves the row that crossed the API is the row that
//! landed on the other peer.
//!
//! `/api/v1/query` is NOT used for the read-back: it hard-requires
//! `db_reader` (fe-api/src/rest.rs::run_local_query), which this scenario
//! deliberately leaves unset to force the DB-thread channel contract FR-5
//! names. Reading back through a route that cannot run with that
//! contract active would prove nothing about the seam under test; the
//! read-back therefore uses the harness's own established pattern
//! (`DbCommand::RawQuery`, see `two_peer_replica_sync.rs` step 7).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use fe_api::server::{build_router, ApiState};
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sync::messages::{SyncCommand, SyncEvent};

use crate::peer::TestPeer;
use crate::TestResult;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub fn run() -> Result<TestResult> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async())
}

async fn run_async() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice-api-bridge", tmp.path())?;
    let bob = TestPeer::spawn("bob-api-bridge", tmp.path())?;

    let alice_addr = alice
        .sync_node_addr
        .clone()
        .ok_or_else(|| anyhow::anyhow!("alice is offline — this scenario needs real transport"))?;
    bob.sync_node_addr
        .clone()
        .ok_or_else(|| anyhow::anyhow!("bob is offline — this scenario needs real transport"))?;

    // 1. Bootstrap fixtures directly on alice's DB thread (not the seam under
    //    test — every other scenario sets up the same way).
    alice.send(DbCommand::CreateVerse {
        name: "CrossThread Verse".into(),
    });
    let verse_id = match alice.wait_for(|r| matches!(r, DbResult::VerseCreated { .. }), WAIT)? {
        DbResult::VerseCreated { id, .. } => id,
        _ => unreachable!(),
    };
    alice.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "CrossThread Fractal".into(),
    });
    let fractal_id = match alice.wait_for(|r| matches!(r, DbResult::FractalCreated { .. }), WAIT)? {
        DbResult::FractalCreated { id, .. } => id,
        _ => unreachable!(),
    };
    alice.send(DbCommand::CreatePetal {
        fractal_id: fractal_id.clone(),
        name: "CrossThread Petal".into(),
    });
    let petal_id = match alice.wait_for(|r| matches!(r, DbResult::PetalCreated { .. }), WAIT)? {
        DbResult::PetalCreated { id, .. } => id,
        _ => unreachable!(),
    };
    alice.send(DbCommand::CreateNode {
        petal_id: petal_id.clone(),
        name: "Sensor Anchor".into(),
        position: [0.0, 0.0, 0.0],
        correlation_id: None,
    });
    let node_id = match alice.wait_for(|r| matches!(r, DbResult::NodeCreated { .. }), WAIT)? {
        DbResult::NodeCreated { id, .. } => id,
        _ => unreachable!(),
    };

    // 2. Open the verse replica on both sides (A2 shape) — bob joins alice.
    let ns_secret_hex = alice
        .namespace_secret(&verse_id)
        .ok_or_else(|| anyhow::anyhow!("verse secret missing from the test secret map"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_hex)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret_bytes));

    bob.sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex.clone(),
            namespace_secret: Some(ns_secret_hex.clone()),
            bootstrap_peers: vec![alice_addr],
        })
        .map_err(|e| anyhow::anyhow!("OpenVerseReplica to bob: {e}"))?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex,
            namespace_secret: Some(ns_secret_hex),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow::anyhow!("OpenVerseReplica to alice: {e}"))?;
    // Give both replicas a moment to come fully online before the API write
    // (mirrors two_peer_replica_sync.rs; WriteRowEntry for a not-yet-open
    // local replica is retained/republished rather than dropped, but giving
    // the open a moment keeps this scenario's timing comparable to its
    // sibling rather than relying on that retry path).
    std::thread::sleep(std::time::Duration::from_millis(300));

    // 3. A REAL ApiState with NO db_reader — api_cmd_tx is bridged below to
    //    alice's REAL db_cmd_tx/db_result_rx, not a mock emulator.
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded::<ApiCommand>(16);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(4);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(4);
    let state = Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key: alice.keypair.verifying_key(),
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: Some(alice.blob_store.clone()),
        cors_origins: vec![],
        db_reader: None, // forces the DbCommand channel seam FR-5 names
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        replication_tx: None, // the None-db_reader path replicates via alice's OWN DB thread, not this field
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    });
    let router = build_router(state.clone());

    // 4. The bridge: every `ApiCommand::DbRequest` crossing the API boundary
    // is forwarded to alice's real DB thread and the real reply routed back
    // — this IS the "DB thread's channel contract" FR-5 asks the scenario to
    // observe a write through, not a stand-in.
    let bridge_cmd_tx = alice.db_cmd_tx.clone();
    let bridge_result_rx = alice.db_result_rx.clone();
    let bridge = std::thread::spawn(move || {
        while let Ok(cmd) = api_cmd_rx.recv() {
            match cmd {
                ApiCommand::DbRequest { cmd, reply_tx } => {
                    if bridge_cmd_tx.send(cmd).is_err() {
                        break;
                    }
                    match bridge_result_rx.recv_timeout(WAIT) {
                        Ok(result) => {
                            let _ = reply_tx.send(result);
                        }
                        Err(_) => break,
                    }
                }
                // Not exercised by this scenario's flow.
                ApiCommand::GetHierarchy { .. }
                | ApiCommand::SyncForward { .. }
                | ApiCommand::TransformPersist { .. } => {}
            }
        }
    });

    // 5. Drive the write through the REAL REST endpoint, over the REAL
    //    router, authenticated with a REAL token — never a direct DbCommand.
    let petal_scope = fe_database::build_scope(&verse_id, Some(&fractal_id), Some(&petal_id));
    let token = fe_identity::api_token::mint_api_token(
        &alice.keypair,
        &petal_scope,
        "editor",
        3600,
        &ulid::Ulid::new().to_string(),
    )?;
    let body = serde_json::json!({
        "readings": [{ "node_id": node_id, "metric": "temperature_c", "value": 21.5 }]
    });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/petals/{petal_id}/iot/readings"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .expect("build iot ingest request");
    let resp = router.oneshot(req).await.expect("router oneshot");
    let status = resp.status();
    let resp_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read response body");
    let resp_json: serde_json::Value =
        serde_json::from_slice(&resp_bytes).expect("response body is JSON");
    if status != StatusCode::OK || resp_json["ok"] != true || resp_json["accepted"] != 1 {
        return Ok(TestResult::fail(
            "api_db_sync_cross_thread",
            &format!("iot ingest over the bridged API did not succeed: {status} {resp_json}"),
        ));
    }

    // 6. Sync-facing effect asserted at the seam: bob (a real second peer,
    //    joined to the same verse replica) observes the write propagate over
    //    real loopback transport — not inferred, not mocked.
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    let mut saw_reading_row = false;
    while std::time::Instant::now() < deadline && !saw_reading_row {
        match bob
            .sync_evt_rx
            .recv_timeout(std::time::Duration::from_millis(500))
        {
            Ok(SyncEvent::RowApplied { verse_id: v, .. }) if v == verse_id => {
                saw_reading_row = true;
            }
            Ok(_) => continue,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    if !saw_reading_row {
        return Ok(TestResult::fail(
            "api_db_sync_cross_thread",
            "bob never observed the API-originated write over the real transport",
        ));
    }

    // 7. Bob's own DB thread applied it through the single-writer seam.
    let apply_result = bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied { outcome, .. }
                    if *outcome == ReplicatedRowOutcome::Applied
                        || *outcome == ReplicatedRowOutcome::Failed
            )
        },
        SYNC_TIMEOUT,
    )?;
    match apply_result {
        DbResult::ReplicatedRowApplied {
            outcome: ReplicatedRowOutcome::Applied,
            ..
        } => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                "api_db_sync_cross_thread",
                &format!("bob's DB thread did not apply the replicated row (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }

    // 8. READ-BACK from bob's durable store: the row that crossed the
    //    bridged API is the row that landed on the other peer.
    bob.send(DbCommand::RawQuery {
        correlation_id: None,
        sql: "SELECT metric, value, node_id FROM iot_reading WHERE petal_id = $pid LIMIT 1".into(),
        vars: [("pid".to_string(), serde_json::json!(petal_id))]
            .into_iter()
            .collect(),
    });
    let read_back = bob.wait_for(|r| matches!(r, DbResult::QueryResult { .. }), WAIT)?;
    let data = match &read_back {
        DbResult::QueryResult { data, .. } => data,
        _ => unreachable!(),
    };
    let Some(row) = data.first() else {
        return Ok(TestResult::fail(
            "api_db_sync_cross_thread",
            "READ-BACK failed: bob's durable store has no iot_reading row for this petal",
        ));
    };
    if row["metric"].as_str() != Some("temperature_c")
        || row["value"].as_f64() != Some(21.5)
        || row["node_id"].as_str() != Some(node_id.as_str())
    {
        return Ok(TestResult::fail(
            "api_db_sync_cross_thread",
            &format!("READ-BACK mismatch: {row}"),
        ));
    }

    // Clean close: release the last `Arc<ApiState>` clone (the one `router`
    // held was already dropped when `.oneshot()` consumed it above) so
    // `api_cmd_tx` disconnects — only then does the bridge thread's blocking
    // `recv()` return, letting it exit before we join it. Dropping the peers
    // first would tear down their DB threads while the bridge might still be
    // mid-forward, racing a send against a closed channel.
    drop(state);
    let _ = bridge.join();

    for (peer, vid) in [(&alice, verse_id.clone()), (&bob, verse_id.clone())] {
        let _ = peer
            .sync_cmd_tx
            .send(SyncCommand::CloseVerseReplica { verse_id: vid });
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    drop(bob);
    drop(alice);

    Ok(TestResult::pass("api_db_sync_cross_thread"))
}
