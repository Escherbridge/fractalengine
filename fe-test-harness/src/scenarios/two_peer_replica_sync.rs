//! Scenario: Two-Peer Real Replica Sync (A2 — verse-replication-e2e).
//!
//! Two in-process peers with **real iroh endpoints over loopback**: alice
//! writes a verse row into her replica (`Doc::set_bytes` through the
//! `WriteRowEntry` seam), the row syncs to bob's replica, bob's inbound pump
//! forwards it to the sync-thread command loop, and the DB thread applies it
//! via `DbCommand::ApplyReplicatedRow`. Proof is **READ-BACK from bob's
//! durable store** (RawQuery), not log lines — plus the `RowApplied` sync
//! event and the `ReplicatedRowApplied { outcome: Applied }` DB reply.
//!
//! Feasible only since each `TestPeer` gets its own P2P data dir (the redb
//! replica store takes an exclusive file lock — one DocsStack per data dir
//! per process), so both peers' stacks are online in one process.

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sync::messages::{SyncCommand, SyncEvent};

use crate::peer::TestPeer;
use crate::TestResult;

/// How long to wait for the row to cross the real transport (iroh
/// connection setup + entry + content sync over loopback).
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;

    // Two isolated peers — each with its own P2P data dir, so both spawn an
    // online DocsStack (the pre-per-peer-dir state degraded every peer but
    // the first to the offline mock).
    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;

    // Real transport requires both endpoints online with a dialable address.
    let alice_addr = alice.sync_node_addr.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "alice is offline — the real-transport scenario requires an online endpoint"
        )
    })?;
    bob.sync_node_addr.clone().ok_or_else(|| {
        anyhow::anyhow!("bob is offline — the real-transport scenario requires an online endpoint")
    })?;

    // 1. Alice creates the verse (durable row in alice's DB + namespace secret).
    alice.send(DbCommand::CreateVerse {
        name: "Shared Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;
    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, .. } => id.clone(),
        _ => unreachable!(),
    };

    // The write capability for the verse's namespace (the production app
    // reads this from its secret store; the test map stands in).
    let ns_secret_hex = alice
        .namespace_secret(&verse_id)
        .ok_or_else(|| anyhow::anyhow!("verse secret missing from the test secret map"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_hex)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret_bytes));

    // 2. Bob joins the namespace first, dialing alice's NodeAddr — imports
    //    the same write capability (same doc id) and starts syncing toward her.
    bob.sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex.clone(),
            namespace_secret: Some(ns_secret_hex.clone()),
            bootstrap_peers: vec![alice_addr],
        })
        .map_err(|e| anyhow::anyhow!("failed to send OpenVerseReplica to bob: {e}"))?;

    // 3. Alice opens her replica of the same namespace (passive side — her
    //    sync task serves entries to peers that dial her, e.g. bob).
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex,
            namespace_secret: Some(ns_secret_hex),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow::anyhow!("failed to send OpenVerseReplica to alice: {e}"))?;

    // 4. Alice writes the verse row into her replica: serialized row JSON →
    //    blob store → WriteRowEntry (the DB→sync bridge's write seam). The
    //    replicator turns this into `Doc::set_bytes` on the real stack.
    let row_data = serde_json::json!({
        "verse_id": verse_id,
        "name": "Shared Verse",
        "created_by": alice.keypair.to_did_key(),
        "created_at": chrono::Utc::now().to_rfc3339(),
        "namespace_id": hex::encode(fe_database::derive_namespace_id(&secret_bytes)),
    });
    let row_bytes = serde_json::to_vec(&row_data)?;
    let hash = alice.blob_store.add_blob(&row_bytes)?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "verse".into(),
            record_id: verse_id.clone(),
            content_hash: hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send WriteRowEntry to alice: {e}"))?;

    // 5. Bob's inbound pump fires: RowApplied sync event from the sync thread.
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    let mut row_applied_evt = false;
    while std::time::Instant::now() < deadline {
        match bob
            .sync_evt_rx
            .recv_timeout(std::time::Duration::from_millis(500))
        {
            Ok(SyncEvent::RowApplied { verse_id: v, .. }) if v == verse_id => {
                row_applied_evt = true;
                break;
            }
            Ok(_) => continue, // health/started events — keep draining
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                return Ok(TestResult::fail(
                    "two_peer_replica_sync",
                    "bob's sync event channel closed while waiting for RowApplied",
                ));
            }
        }
    }
    if !row_applied_evt {
        return Ok(TestResult::fail(
            "two_peer_replica_sync",
            "bob never received the replicated row over the real transport",
        ));
    }

    // 6. Bob's DB thread applied it through the single-writer seam — the
    //    outcome echo must be `Applied` (not Failed / Skipped / NotApplicable).
    let apply_result = bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied {
                    ref outcome,
                    ..
                } if *outcome == ReplicatedRowOutcome::Applied
                    || *outcome == ReplicatedRowOutcome::Failed
            )
        },
        SYNC_TIMEOUT,
    )?;
    match &apply_result {
        DbResult::ReplicatedRowApplied { outcome, .. }
            if *outcome == ReplicatedRowOutcome::Applied => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                "two_peer_replica_sync",
                &format!("bob's DB thread did not apply the replicated row (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }

    // 7. READ-BACK: the row is in bob's durable store — queried like any
    //    local row, not inferred from events or logs (A2's acceptance form).
    bob.send(DbCommand::RawQuery {
        correlation_id: None,
        sql: "SELECT verse_id, name, created_by FROM verse WHERE verse_id = $vid LIMIT 1".into(),
        vars: [("vid".to_string(), serde_json::json!(verse_id))]
            .into_iter()
            .collect(),
    });
    let read_back = bob.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        std::time::Duration::from_secs(30),
    )?;
    let data = match &read_back {
        DbResult::QueryResult { data, .. } => data,
        _ => unreachable!(),
    };
    if data.is_empty() {
        return Ok(TestResult::fail(
            "two_peer_replica_sync",
            "READ-BACK failed: bob's durable store has no row for the replicated verse",
        ));
    }
    let name = data[0]["name"].as_str().unwrap_or_default();
    if name != "Shared Verse" {
        return Ok(TestResult::fail(
            "two_peer_replica_sync",
            &format!("READ-BACK mismatch: bob's row name is {name:?}, expected \"Shared Verse\""),
        ));
    }

    // 8. Clean close on both sides.
    for (peer, vid) in [(&alice, verse_id.clone()), (&bob, verse_id.clone())] {
        peer.sync_cmd_tx
            .send(SyncCommand::CloseVerseReplica { verse_id: vid })
            .map_err(|e| anyhow::anyhow!("failed to send CloseVerseReplica: {e}"))?;
    }
    std::thread::sleep(std::time::Duration::from_millis(200));

    drop(bob);
    drop(alice);
    Ok(TestResult::pass("two_peer_replica_sync"))
}
