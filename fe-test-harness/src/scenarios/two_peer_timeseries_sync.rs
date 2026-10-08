//! Scenario: Two-Peer Timeseries Union Sync (A12 — union idempotency through
//! the real transport).
//!
//! Proves the `iot_reading` union CRDT end-to-end on the A2 two-peer loopback
//! setup (see fe-database `src/AGENTS.md` §replication-mode): the publisher
//! emits each reading as a per-row replication entry — the shape
//! `insert_readings_with_replication` produces (one `ReplicationEvent` per
//! accepted row, keyed by that row's `reading_id`) — and the joining peer
//! applies it through the real inbound path (A3 role gate → per-table mode
//! dispatch → union apply).
//!
//! Legs, each proven by READ-BACK from bob's **durable** store:
//!
//! 1. a reading crosses and applies;
//! 2. the identical row re-delivered is idempotent — still exactly one row;
//! 3. a conflicting payload for the same `reading_id` never overwrites the fact;
//! 4. an empty-entry tombstone (`Doc::del`) is `NotApplicable` — the fact stays
//!    (a timeseries row is never tombstoned);
//! 5. a second reading with an *earlier* timestamp delivered later still joins
//!    the set — the union is delivery-order independent.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sync::messages::SyncCommand;

use crate::peer::TestPeer;
use crate::TestResult;

/// How long to wait for a row to cross the real transport.
const SYNC_TIMEOUT: Duration = Duration::from_secs(45);

/// Publish one row the way the emission seam does: blob-store the row bytes,
/// then hand the content hash to the sync thread as a `WriteRowEntry`.
fn publish_row(peer: &TestPeer, verse_id: &str, record_id: &str, row_bytes: &[u8]) -> Result<()> {
    let content_hash = peer.blob_store.add_blob(row_bytes)?;
    peer.sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.to_string(),
            table: "iot_reading".into(),
            record_id: record_id.to_string(),
            content_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send WriteRowEntry for {record_id}: {e}"))?;
    Ok(())
}

/// Wait for the joining peer's apply outcome echo for an `iot_reading` row.
fn wait_row_outcome(peer: &TestPeer) -> Result<ReplicatedRowOutcome> {
    match peer.wait_for(
        |r| matches!(r, DbResult::ReplicatedRowApplied { ref table, .. } if table == "iot_reading"),
        SYNC_TIMEOUT,
    )? {
        DbResult::ReplicatedRowApplied { outcome, .. } => Ok(outcome),
        other => anyhow::bail!("unexpected result while waiting for the row outcome: {other:?}"),
    }
}

/// READ-BACK straight from a peer's durable store.
fn read_readings(peer: &TestPeer, where_clause: &str) -> Result<Vec<serde_json::Value>> {
    peer.send(DbCommand::RawQuery {
        sql: format!("SELECT * FROM iot_reading{where_clause}"),
        vars: HashMap::new(),
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::QueryResult { data } => Ok(data),
        other => anyhow::bail!("unexpected read-back result: {other:?}"),
    }
}

/// One `iot_reading` row exactly as the ingestion seam serializes it.
fn reading_row(
    reading_id: &str,
    value: f64,
    recorded_at: &str,
    recorded_at_ms: i64,
) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "reading_id": reading_id,
        "node_id": "node-a12",
        "petal_id": "petal-a12",
        "metric": "temperature_c",
        "value": value,
        "units": "C",
        "recorded_at": recorded_at,
        "recorded_at_ms": recorded_at_ms,
        "hlc_timestamp": recorded_at_ms,
        "source_did": "did:key:z6MkSensorA12",
    }))?)
}

pub fn run() -> Result<TestResult> {
    const NAME: &str = "two_peer_timeseries_sync";
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
        name: "Timeseries Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        Duration::from_secs(30),
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

    std::thread::sleep(Duration::from_millis(500));

    // 3. The verse manifest converges first (bootstrap admission on bob's
    //    side), so the reading rows meet the A3 gate with a resolvable verse
    //    (alice = created_by = Owner).
    let verse_row = serde_json::json!({
        "verse_id": verse_id,
        "name": "Timeseries Verse",
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
        |r| matches!(r, DbResult::ReplicatedRowApplied { ref table, .. } if table == "verse"),
        SYNC_TIMEOUT,
    )?;
    match &outcome {
        DbResult::ReplicatedRowApplied { outcome, .. }
            if *outcome == ReplicatedRowOutcome::Applied => {}
        DbResult::ReplicatedRowApplied { outcome, .. } => {
            return Ok(TestResult::fail(
                NAME,
                &format!("verse manifest did not converge on bob (outcome: {outcome:?})"),
            ));
        }
        _ => unreachable!(),
    }
    bob.send(DbCommand::RawQuery {
        sql: "SELECT verse_id FROM verse LIMIT 1".into(),
        vars: HashMap::new(),
    });
    let manifest = bob.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        Duration::from_secs(30),
    )?;
    match manifest {
        DbResult::QueryResult { data } if data.is_empty() => {
            return Ok(TestResult::fail(
                NAME,
                "READ-BACK failed: bob's store never converged the verse manifest",
            ));
        }
        DbResult::QueryResult { .. } => {}
        other => anyhow::bail!("unexpected manifest read-back result: {other:?}"),
    }

    // --- Leg 1: a reading crosses the real transport and applies. ---------
    let reading_a = reading_row(
        "reading-a12",
        21.5,
        "2026-07-15T10:00:00Z",
        1_752_580_800_000,
    )?;
    publish_row(&alice, &verse_id, "reading-a12", &reading_a)?;
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("the reading did not apply on bob (outcome: {outcome:?})"),
        ));
    }
    let rows = read_readings(&bob, " WHERE reading_id = 'reading-a12'")?;
    if rows.len() != 1 || rows[0]["value"].as_f64() != Some(21.5) {
        return Ok(TestResult::fail(
            NAME,
            &format!("READ-BACK failed: expected one 21.5 reading, got {rows:?}"),
        ));
    }

    // --- Leg 2: identical re-delivery is idempotent (no duplicate). -------
    publish_row(&alice, &verse_id, "reading-a12", &reading_a)?;
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("re-delivery reported a non-idempotent outcome: {outcome:?}"),
        ));
    }
    let rows = read_readings(&bob, "")?;
    if rows.len() != 1 {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "READ-BACK failed: re-delivery duplicated the reading ({} rows)",
                rows.len()
            ),
        ));
    }

    // --- Leg 3: a conflicting payload for the same key never overwrites. --
    let conflicting = reading_row(
        "reading-a12",
        99.0,
        "2026-07-15T10:00:00Z",
        1_752_580_800_000,
    )?;
    publish_row(&alice, &verse_id, "reading-a12", &conflicting)?;
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("conflicting re-delivery reported an unexpected outcome: {outcome:?}"),
        ));
    }
    let rows = read_readings(&bob, " WHERE reading_id = 'reading-a12'")?;
    if rows.len() != 1 || rows[0]["value"].as_f64() != Some(21.5) {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "union violated: an existing reading was overwritten by a later payload — {rows:?}"
            ),
        ));
    }

    // --- Leg 4: an empty-entry tombstone never removes a timeseries fact. --
    let tombstone_hash = alice.blob_store.add_blob(b"")?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "iot_reading".into(),
            record_id: "reading-a12".into(),
            content_hash: tombstone_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send the tombstone WriteRowEntry: {e}"))?;
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::NotApplicable {
        return Ok(TestResult::fail(
            NAME,
            &format!("a row tombstone on a timeseries key was not NotApplicable: {outcome:?}"),
        ));
    }
    let rows = read_readings(&bob, " WHERE reading_id = 'reading-a12'")?;
    if rows.len() != 1 || rows[0]["value"].as_f64() != Some(21.5) {
        return Ok(TestResult::fail(
            NAME,
            &format!("a tombstone retracted a timeseries fact — {rows:?}"),
        ));
    }

    // --- Leg 5: union is delivery-order independent. ----------------------
    // An *earlier*-timestamped reading arrives after the later one; the set
    // must hold both, not "the newest".
    let reading_b = reading_row(
        "reading-a12-b",
        19.0,
        "2026-07-15T09:00:00Z",
        1_752_577_200_000,
    )?;
    publish_row(&alice, &verse_id, "reading-a12-b", &reading_b)?;
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("the out-of-order reading did not apply (outcome: {outcome:?})"),
        ));
    }
    let rows = read_readings(&bob, "")?;
    if rows.len() != 2 {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "READ-BACK failed: the union should hold both readings, got {}",
                rows.len()
            ),
        ));
    }
    let mut values: Vec<f64> = rows.iter().filter_map(|r| r["value"].as_f64()).collect();
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite values"));
    if values != vec![19.0, 21.5] {
        return Ok(TestResult::fail(
            NAME,
            &format!("READ-BACK mismatch: expected [19.0, 21.5], got {values:?}"),
        ));
    }

    // 6. Clean close on both sides.
    for peer in [&alice, &bob] {
        peer.sync_cmd_tx
            .send(SyncCommand::CloseVerseReplica {
                verse_id: verse_id.clone(),
            })
            .map_err(|e| anyhow::anyhow!("failed to send CloseVerseReplica: {e}"))?;
    }
    std::thread::sleep(Duration::from_millis(200));

    drop(bob);
    drop(alice);
    Ok(TestResult::pass(NAME))
}
