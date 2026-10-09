//! Scenario: Two-Peer Tombstone Sync (A5 — tombstone-through-transport).
//!
//! Proves tombstone dominance (N-4) through the **real** transport and the
//! real inbound apply path, on the A2 two-peer loopback setup: alice writes
//! a live node row → bob applies it; alice publishes the empty-entry
//! tombstone (`Doc::del`) → bob converges the row to deleted; alice
//! re-publishes the stale live row → bob refuses the resurrection
//! (`SkippedTombstoned`). Every leg is proven by **READ-BACK from bob's
//! durable store**, plus the `ReplicatedRowApplied` outcome echo.
//!
//! Also crosses the A3 gate on the live path: by the node rows bob's store
//! already holds the replicated verse manifest, and alice (its `created_by`)
//! resolves to Owner at the verse scope — a non-admitted author would see
//! `Denied` instead of these outcomes.

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sync::messages::SyncCommand;

use crate::peer::TestPeer;
use crate::TestResult;

/// How long to wait for each row to cross the real transport.
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;

    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;

    let alice_addr = alice.sync_node_addr.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "alice is offline — the real-transport scenario requires an online endpoint"
        )
    })?;
    bob.sync_node_addr.clone().ok_or_else(|| {
        anyhow::anyhow!("bob is offline — the real-transport scenario requires an online endpoint")
    })?;

    // 1. Alice creates the verse (durable row + namespace secret).
    alice.send(DbCommand::CreateVerse {
        name: "Tombstone Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;
    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, .. } => id.clone(),
        _ => unreachable!(),
    };
    let alice_did = alice.keypair.to_did_key();

    let ns_secret_hex = alice
        .namespace_secret(&verse_id)
        .ok_or_else(|| anyhow::anyhow!("verse secret missing from the test secret map"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_hex)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret_bytes));

    // 2. Bob joins the namespace (dialing alice), alice opens hers passively.
    bob.sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex.clone(),
            namespace_secret: Some(ns_secret_hex.clone()),
            bootstrap_peers: vec![alice_addr],
        })
        .map_err(|e| anyhow::anyhow!("failed to send OpenVerseReplica to bob: {e}"))?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex,
            namespace_secret: Some(ns_secret_hex),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow::anyhow!("failed to send OpenVerseReplica to alice: {e}"))?;

    // Small settle window for the swarm to connect (bob dials alice; alice
    // serves). Not a correctness wait — each row has its own outcome wait.
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Helper: read rows back from a peer's durable store (A2's READ-BACK form).
    fn read_back(peer: &TestPeer, sql: &str, vid: &str) -> Result<Vec<serde_json::Value>> {
        peer.send(DbCommand::RawQuery {
            correlation_id: None,
            sql: sql.to_string(),
            vars: [("vid".to_string(), serde_json::json!(vid))]
                .into_iter()
                .collect(),
        });
        let result = peer.wait_for(
            |r| matches!(r, DbResult::QueryResult { .. }),
            std::time::Duration::from_secs(30),
        )?;
        match result {
            DbResult::QueryResult { data, .. } => Ok(data),
            other => anyhow::bail!("unexpected read-back result: {other:?}"),
        }
    }

    // 3. The verse manifest crosses first (bootstrap admission on bob's
    //    side), so the later node rows hit the A3 gate with a resolvable
    //    verse (alice = created_by = Owner).
    let verse_row = serde_json::json!({
        "verse_id": verse_id,
        "name": "Tombstone Verse",
        "created_by": alice_did,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "namespace_id": hex::encode(fe_database::derive_namespace_id(&secret_bytes)),
        "default_access": "viewer",
    });
    let verse_bytes = serde_json::to_vec(&verse_row)?;
    let verse_hash = alice.blob_store.add_blob(&verse_bytes)?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "verse".into(),
            record_id: verse_id.clone(),
            content_hash: verse_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send WriteRowEntry to alice: {e}"))?;
    let outcome = bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied { ref table, .. } if table == "verse"
            )
        },
        SYNC_TIMEOUT,
    )?;
    match &outcome {
        DbResult::ReplicatedRowApplied { outcome, .. }
            if *outcome == ReplicatedRowOutcome::Applied => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                "two_peer_tombstone_sync",
                &format!("verse manifest did not converge on bob (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }
    let rows = read_back(
        &bob,
        "SELECT verse_id FROM verse WHERE verse_id = $vid LIMIT 1",
        &verse_id,
    )?;
    if rows.is_empty() {
        return Ok(TestResult::fail(
            "two_peer_tombstone_sync",
            "READ-BACK failed: bob's store never converged the verse manifest",
        ));
    }

    // 4. Leg 1: a live node row crosses and applies.
    let node_row = serde_json::json!({
        "node_id": "node-a5",
        "petal_id": "petal-a5",
        "display_name": "A5 Node",
        "position": { "type": "Point", "coordinates": [3.0, 4.0] },
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    let node_bytes = serde_json::to_vec(&node_row)?;
    let node_hash = alice.blob_store.add_blob(&node_bytes)?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "node".into(),
            record_id: "node-a5".into(),
            content_hash: node_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send WriteRowEntry to alice: {e}"))?;
    let outcome = bob.wait_for(
        |r| matches!(r, DbResult::ReplicatedRowApplied { ref table, .. } if table == "node"),
        SYNC_TIMEOUT,
    )?;
    match &outcome {
        DbResult::ReplicatedRowApplied { outcome, .. }
            if *outcome == ReplicatedRowOutcome::Applied => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                "two_peer_tombstone_sync",
                &format!("live node row did not apply on bob (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }
    let rows = read_back(
        &bob,
        "SELECT tombstone FROM node WHERE node_id = 'node-a5' LIMIT 1",
        &verse_id,
    )?;
    if rows.is_empty() {
        return Ok(TestResult::fail(
            "two_peer_tombstone_sync",
            "READ-BACK failed: the live node row never converged on bob",
        ));
    }
    if rows[0]["tombstone"].is_object() {
        return Ok(TestResult::fail(
            "two_peer_tombstone_sync",
            "READ-BACK mismatch: the node converged pre-tombstoned",
        ));
    }

    // 5. Leg 2: the empty-entry tombstone (`Doc::del`) converges bob's row to
    //    deleted. The wire form carries no payload, so the DB thread must
    //    recognize the empty entry as the deletion marker.
    let tombstone_hash = alice.blob_store.add_blob(b"")?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "node".into(),
            record_id: "node-a5".into(),
            content_hash: tombstone_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send tombstone WriteRowEntry to alice: {e}"))?;
    let outcome = bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied {
                    ref table,
                    ref outcome,
                    ..
                } if table == "node"
                    && (*outcome == ReplicatedRowOutcome::AppliedTombstone
                        || *outcome == ReplicatedRowOutcome::Failed)
            )
        },
        SYNC_TIMEOUT,
    )?;
    match &outcome {
        DbResult::ReplicatedRowApplied { outcome, .. }
            if *outcome == ReplicatedRowOutcome::AppliedTombstone => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                "two_peer_tombstone_sync",
                &format!("incoming tombstone did not converge the row (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }
    let rows = read_back(
        &bob,
        "SELECT tombstone FROM node WHERE node_id = 'node-a5' LIMIT 1",
        &verse_id,
    )?;
    if !rows.first().is_some_and(|r| r["tombstone"].is_object()) {
        return Ok(TestResult::fail(
            "two_peer_tombstone_sync",
            "READ-BACK failed: the node is not durably tombstoned on bob",
        ));
    }

    // 6. Leg 3 (N-4): a stale live row (a replica that never saw the delete)
    //    must NOT resurrect the tombstoned node — SkippedTombstoned, and the
    //    READ-BACK still shows the tombstone.
    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "node".into(),
            record_id: "node-a5".into(),
            content_hash: node_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send stale-live WriteRowEntry to alice: {e}"))?;
    let outcome = bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied {
                    ref table,
                    ref outcome,
                    ..
                } if table == "node"
                    && (*outcome == ReplicatedRowOutcome::SkippedTombstoned
                        || *outcome == ReplicatedRowOutcome::Failed
                        || *outcome == ReplicatedRowOutcome::Applied)
            )
        },
        SYNC_TIMEOUT,
    )?;
    match &outcome {
        DbResult::ReplicatedRowApplied { outcome, .. }
            if *outcome == ReplicatedRowOutcome::SkippedTombstoned => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                "two_peer_tombstone_sync",
                &format!("stale live row resurrected the tombstone — N-4 violated (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }
    let rows = read_back(
        &bob,
        "SELECT tombstone FROM node WHERE node_id = 'node-a5' LIMIT 1",
        &verse_id,
    )?;
    if !rows.first().is_some_and(|r| r["tombstone"].is_object()) {
        return Ok(TestResult::fail(
            "two_peer_tombstone_sync",
            "READ-BACK mismatch: the tombstone did not survive the stale live write",
        ));
    }

    // 7. Clean close on both sides.
    for peer in [&alice, &bob] {
        peer.sync_cmd_tx
            .send(SyncCommand::CloseVerseReplica {
                verse_id: verse_id.clone(),
            })
            .map_err(|e| anyhow::anyhow!("failed to send CloseVerseReplica: {e}"))?;
    }
    std::thread::sleep(std::time::Duration::from_millis(200));

    drop(bob);
    drop(alice);
    Ok(TestResult::pass("two_peer_tombstone_sync"))
}
