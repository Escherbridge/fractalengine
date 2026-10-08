//! Scenario: Two-Peer Shard Fabric (A13 mode-switching + A14 capacity-aware
//! placement through the real transport).
//!
//! Proves the M2/F6 sharded hybrid timeseries fabric end-to-end on the A2
//! two-peer loopback setup: per-verse timeseries settings ride the verse
//! manifest, shard ledger rows (`__shards/*`) and peer declarations
//! (`__peers/*`) cross the real doc transport, and placement is
//! capacity-aware and mode-aware.
//!
//! Legs, each proven by READ-BACK (the shard ledger via
//! `GetShardLedger` → `SyncEvent::ShardLedger`, the durable store via
//! `RawQuery`):
//!
//! 1. **A13 balanced R=2**: a reading published under a `balanced`/R=2 verse
//!    plans two hosts (both peers), publishes its `__shards` ledger row, and
//!    the ledger + the reading both converge on the joining peer.
//! 2. **A13 mode switch to `mirror`**: re-publishing the verse manifest with
//!    `ts_mode = mirror` flips both fabrics; the next shard plans ALL peers
//!    and its ledger records `mirror` — the mode-switch the settings surface
//!    drives.
//! 3. **A13/A14 `sharded` + capacity routing**: the joining peer declares a
//!    1-byte capacity; a new shard's plan excludes it (1 host, the creator)
//!    and the joining peer does NOT retain a later reading for that shard
//!    (transfer routing, receive side).
//! 4. **A14 smallest peer never caps the total**: with the joining peer
//!    capped at ~2 shards' worth of bytes, many one-host shards are ALL
//!    placed — the fleet total far exceeds the smallest declaration.
//! 5. **A14 a shard is never left homeless**: with both peers at ~zero
//!    declared capacity, a new shard still lands somewhere (least-utilized
//!    last resort) — capacity is a planning hint, not a wall.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sync::messages::{SyncCommand, SyncEvent};

use crate::peer::TestPeer;
use crate::TestResult;

/// How long to wait for ledger/declaration convergence over the real transport.
const SYNC_TIMEOUT: Duration = Duration::from_secs(45);
/// Grace window for a reading row to cross after its shard ledger row has
/// demonstrably converged (the receive-side retention decision has no
/// `DbResult` echo — a skipped row is silent by design).
const ROUTE_GRACE: Duration = Duration::from_secs(3);

/// Bucket width for this verse: 60s, so consecutive minutes map to distinct
/// buckets and one anchor's readings spread across many shards.
const BUCKET_MS: i64 = 60_000;
/// 2026-07-15T10:00:00Z in epoch ms — bucket index 29_209_680.
const BASE_MS: i64 = 1_752_580_800_000;

/// Publish one row the way the emission seam does: blob-store the row bytes,
/// then hand the content hash to the sync thread as a `WriteRowEntry`.
fn publish_row(
    peer: &TestPeer,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
) -> Result<()> {
    let content_hash = peer.blob_store.add_blob(row_bytes)?;
    peer.sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.to_string(),
            table: table.into(),
            record_id: record_id.to_string(),
            content_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to send WriteRowEntry for {record_id}: {e}"))?;
    Ok(())
}

/// One `iot_reading` row exactly as the ingestion seam serializes it (one
/// reading per minute; each lands in its own bucket → its own shard).
fn reading_row(reading_id: &str, value: f64, minute_offset: i64) -> Result<Vec<u8>> {
    let recorded_at_ms = BASE_MS + minute_offset * BUCKET_MS;
    Ok(serde_json::to_vec(&serde_json::json!({
        "reading_id": reading_id,
        "node_id": "node-f6",
        "petal_id": "petal-f6",
        "metric": "temperature_c",
        "value": value,
        "units": "C",
        "recorded_at": "2026-07-15T10:00:00Z",
        "recorded_at_ms": recorded_at_ms,
        "hlc_timestamp": recorded_at_ms,
        "source_did": "did:key:z6MkSensorF6",
    }))?)
}

/// The verse manifest row, carrying the `ts_*` settings the fabrics learn.
fn verse_manifest(
    verse_id: &str,
    name: &str,
    alice_did: &str,
    ts_mode: &str,
    ts_r: u32,
) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "verse_id": verse_id,
        "name": name,
        "created_by": alice_did,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "default_access": "viewer",
        "ts_mode": ts_mode,
        "ts_replication_factor": ts_r,
        "ts_bucket_width_ms": BUCKET_MS,
    }))?)
}

/// Ask a peer's sync thread for its fabric dump (`GetShardLedger` →
/// `SyncEvent::ShardLedger`) and parse the JSON.
fn ledger_dump(peer: &TestPeer, verse_id: &str) -> Result<serde_json::Value> {
    peer.sync_cmd_tx
        .send(SyncCommand::GetShardLedger {
            verse_id: verse_id.to_string(),
        })
        .map_err(|e| anyhow::anyhow!("failed to send GetShardLedger: {e}"))?;
    let evt = peer.wait_sync_event(
        |e| matches!(e, SyncEvent::ShardLedger { .. } if matches!(e, SyncEvent::ShardLedger { verse_id: v, .. } if v == verse_id)),
        Duration::from_secs(30),
    )?;
    match evt {
        SyncEvent::ShardLedger { ledger_json, .. } => {
            serde_json::from_str(&ledger_json).map_err(|e| anyhow::anyhow!("bad ledger JSON: {e}"))
        }
        _ => unreachable!(),
    }
}

/// Poll the fabric dump until `probe(dump)` passes (transport convergence),
/// with SYNC_TIMEOUT.
fn poll_ledger<F: Fn(&serde_json::Value) -> bool>(
    peer: &TestPeer,
    verse_id: &str,
    probe: F,
) -> Result<serde_json::Value> {
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let dump = ledger_dump(peer, verse_id)?;
        if probe(&dump) {
            return Ok(dump);
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("shard ledger never converged: {}", dump);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// READ-BACK readings straight from a peer's durable store.
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

pub fn run() -> Result<TestResult> {
    const NAME: &str = "two_peer_shard_fabric";
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
        name: "Shard Fabric Verse".into(),
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
    let bob_did = bob.keypair.to_did_key();

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

    // 3. The verse manifest converges first, carrying `ts_mode=balanced` and
    //    R=2 (A13 settings ride the manifest; the fabrics learn from both
    //    directions of verse-doc traffic).
    let manifest = verse_manifest(&verse_id, "Shard Fabric Verse", &alice_did, "balanced", 2)?;
    publish_row(&alice, &verse_id, "verse", &verse_id, &manifest)?;
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
    // Both fabrics must have learned balanced/R=2 from the manifest.
    for (peer, who) in [(&alice, "alice"), (&bob, "bob")] {
        let dump = poll_ledger(peer, &verse_id, |d| {
            d["settings"]["mode"].as_str() == Some("balanced")
        })
        .map_err(|e| anyhow::anyhow!("{who}'s fabric never learned balanced mode: {e}"))?;
        let _ = dump;
    }
    // Both peers' declarations must be in both fabrics (published on open).
    for (peer, who) in [(&alice, "alice"), (&bob, "bob")] {
        poll_ledger(peer, &verse_id, |d| {
            d["peers"][alice_did.as_str()].is_object() && d["peers"][bob_did.as_str()].is_object()
        })
        .map_err(|e| anyhow::anyhow!("{who}'s fabric never saw both peer declarations: {e}"))?;
    }

    // --- Leg 1 (A13): balanced R=2 plans both peers, ledger crosses. ------
    let reading_a = reading_row("reading-f6-a", 21.5, 0)?;
    publish_row(&alice, &verse_id, "iot_reading", "reading-f6-a", &reading_a)?;
    // The reading itself applies on bob (bob hosts the shard → Retain).
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 1: the reading did not apply on bob (outcome: {outcome:?})"),
        ));
    }
    // The shard ledger entry crossed and recorded both hosts at R=2.
    let shard_a = "petal-f6/node-f6/29209680";
    let dump = poll_ledger(&bob, &verse_id, |d| {
        d["shards"][shard_a].is_object()
            && d["shards"][shard_a]["hosts"]
                .as_array()
                .map(|h| h.len() == 2)
                .unwrap_or(false)
    })
    .map_err(|e| anyhow::anyhow!("leg 1: bob's ledger never converged shard {shard_a}: {e}"))?;
    {
        let hosts: Vec<&str> = dump["shards"][shard_a]["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        if !hosts.contains(&alice_did.as_str()) || !hosts.contains(&bob_did.as_str()) {
            return Ok(TestResult::fail(
                NAME,
                &format!("leg 1: balanced R=2 must plan both peers, got hosts {hosts:?}"),
            ));
        }
        if dump["shards"][shard_a]["mode"].as_str() != Some("balanced")
            || dump["shards"][shard_a]["replication_factor"].as_u64() != Some(2)
        {
            return Ok(TestResult::fail(
                NAME,
                "leg 1: the ledger must record the mode and R at placement",
            ));
        }
    }
    let rows = read_readings(&bob, " WHERE reading_id = 'reading-f6-a'")?;
    if rows.len() != 1 || rows[0]["value"].as_f64() != Some(21.5) {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 1: READ-BACK failed on bob's durable store — {rows:?}"),
        ));
    }

    // --- Leg 2 (A13): mode switch to mirror via the verse manifest. -------
    let manifest_mirror = verse_manifest(&verse_id, "Shard Fabric Verse", &alice_did, "mirror", 1)?;
    publish_row(&alice, &verse_id, "verse", &verse_id, &manifest_mirror)?;
    for (peer, who) in [(&alice, "alice"), (&bob, "bob")] {
        poll_ledger(peer, &verse_id, |d| {
            d["settings"]["mode"].as_str() == Some("mirror")
        })
        .map_err(|e| anyhow::anyhow!("leg 2: {who}'s fabric never switched to mirror: {e}"))?;
    }
    // The next shard plans ALL peers (mirror), recorded in its ledger row.
    let reading_b = reading_row("reading-f6-b", 19.0, 1)?;
    publish_row(&alice, &verse_id, "iot_reading", "reading-f6-b", &reading_b)?;
    let shard_b = "petal-f6/node-f6/29209681";
    let dump = poll_ledger(&bob, &verse_id, |d| {
        d["shards"][shard_b].is_object()
            && d["shards"][shard_b]["hosts"]
                .as_array()
                .map(|h| h.len() == 2)
                .unwrap_or(false)
    })
    .map_err(|e| anyhow::anyhow!("leg 2: bob's ledger never converged shard {shard_b}: {e}"))?;
    if dump["shards"][shard_b]["mode"].as_str() != Some("mirror") {
        return Ok(TestResult::fail(
            NAME,
            "leg 2: a shard planned under mirror must record mirror",
        ));
    }
    let outcome = wait_row_outcome(&bob)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 2: mirror must retain every reading (outcome: {outcome:?})"),
        ));
    }
    let rows = read_readings(&bob, " WHERE reading_id = 'reading-f6-b'")?;
    if rows.len() != 1 || rows[0]["value"].as_f64() != Some(19.0) {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 2: READ-BACK failed on bob's durable store — {rows:?}"),
        ));
    }

    // --- Leg 3 (A13/A14): sharded mode + capacity routing. -----------------
    // Switch the mode to `sharded` (one host per shard)...
    let manifest_sharded =
        verse_manifest(&verse_id, "Shard Fabric Verse", &alice_did, "sharded", 1)?;
    publish_row(&alice, &verse_id, "verse", &verse_id, &manifest_sharded)?;
    for (peer, who) in [(&alice, "alice"), (&bob, "bob")] {
        poll_ledger(peer, &verse_id, |d| {
            d["settings"]["mode"].as_str() == Some("sharded")
        })
        .map_err(|e| anyhow::anyhow!("leg 3: {who}'s fabric never switched to sharded: {e}"))?;
    }
    // ...and starve bob: a 1-byte capacity declaration can fit no shard.
    bob.sync_cmd_tx
        .send(SyncCommand::SetShardDeclaration {
            capacity_bytes: Some(1),
            seeder: false,
        })
        .map_err(|e| anyhow::anyhow!("failed to send SetShardDeclaration to bob: {e}"))?;
    // Alice plans, so HER fabric must see bob's 1-byte declaration cross.
    poll_ledger(&alice, &verse_id, |d| {
        d["peers"][bob_did.as_str()]["capacity_bytes"].as_u64() == Some(1)
    })
    .map_err(|e| anyhow::anyhow!("leg 3: bob's declaration never reached alice's fabric: {e}"))?;

    // The next shard must exclude bob: exactly one host, the creator.
    let reading_c = reading_row("reading-f6-c", 18.0, 2)?;
    publish_row(&alice, &verse_id, "iot_reading", "reading-f6-c", &reading_c)?;
    let shard_c = "petal-f6/node-f6/29209682";
    let dump = poll_ledger(&bob, &verse_id, |d| {
        d["shards"][shard_c].is_object()
            && d["shards"][shard_c]["hosts"]
                .as_array()
                .map(|h| h.len() == 1)
                .unwrap_or(false)
    })
    .map_err(|e| anyhow::anyhow!("leg 3: bob's ledger never converged shard {shard_c}: {e}"))?;
    if dump["shards"][shard_c]["hosts"][0].as_str() != Some(alice_did.as_str()) {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 3: a full 1-byte bob must be excluded — hosts {:?}",
                dump["shards"][shard_c]["hosts"]
            ),
        ));
    }
    // Transfer routing, receive side: once the ledger has converged on bob
    // (proven above), a LATER reading for the same shard must NOT apply on
    // bob — another peer hosts it. (The first reading raced the ledger row;
    // an unknown shard retains, which is the safe default, so its outcome is
    // not asserted.)
    let reading_d = reading_row("reading-f6-d", 17.0, 2)?;
    publish_row(&alice, &verse_id, "iot_reading", "reading-f6-d", &reading_d)?;
    std::thread::sleep(ROUTE_GRACE);
    let rows = read_readings(&bob, " WHERE reading_id = 'reading-f6-d'")?;
    if !rows.is_empty() {
        return Ok(TestResult::fail(
            NAME,
            "leg 3: bob retained a reading for a shard he does not host — transfer routing broken",
        ));
    }

    // --- Leg 4 (A14): the smallest peer never caps the total. -------------
    // Bob caps at ~2 shards' worth of bytes; alice stays unlimited. Eight
    // one-host shards must ALL be placed — bob takes at most two, alice the
    // rest. The fleet total is bounded by the SUM, never the MINIMUM.
    bob.sync_cmd_tx
        .send(SyncCommand::SetShardDeclaration {
            capacity_bytes: Some(600),
            seeder: false,
        })
        .map_err(|e| anyhow::anyhow!("failed to send bob's widened declaration: {e}"))?;
    poll_ledger(&alice, &verse_id, |d| {
        d["peers"][bob_did.as_str()]["capacity_bytes"].as_u64() == Some(600)
    })
    .map_err(|e| anyhow::anyhow!("leg 4: bob's widened declaration never reached alice: {e}"))?;
    for minute in 3..11 {
        let reading = reading_row(&format!("reading-f6-m{minute}"), 16.0, minute)?;
        publish_row(
            &alice,
            &verse_id,
            "iot_reading",
            &format!("reading-f6-m{minute}"),
            &reading,
        )?;
    }
    let dump = poll_ledger(&alice, &verse_id, |d| {
        (3..11).all(|m| {
            let key = format!("petal-f6/node-f6/{}", 29_209_680 + m);
            d["shards"][key.as_str()].is_object()
                && d["shards"][key.as_str()]["hosts"]
                    .as_array()
                    .map(|h| !h.is_empty())
                    .unwrap_or(false)
        })
    })
    .map_err(|e| anyhow::anyhow!("leg 4: alice's ledger never converged the 8 new shards: {e}"))?;
    // Every one of the 8 shards is placed on exactly one host; count how many
    // landed on bob — must stay ≤ 2 (his 600-byte declaration) while the
    // fleet placed all 8 (total ≈ 8 shards, far past bob's 2).
    let mut bob_hosted = 0usize;
    for m in 3..11 {
        let key = format!("petal-f6/node-f6/{}", 29_209_680 + m);
        let hosts = dump["shards"][key.as_str()]["hosts"].as_array().unwrap();
        if hosts.len() != 1 {
            return Ok(TestResult::fail(
                NAME,
                &format!(
                    "leg 4: shard {key} planned {} hosts under sharded mode",
                    hosts.len()
                ),
            ));
        }
        if hosts[0].as_str() == Some(bob_did.as_str()) {
            bob_hosted += 1;
        }
    }
    if bob_hosted > 2 {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg hosted {bob_hosted} shards past his 600-byte declaration"),
        ));
    }
    // The fleet total (8 placed shards) must exceed what bob's declaration
    // alone could ever hold (~2 shards) — the smallest peer never caps the
    // total: alice absorbed the rest, so nothing was left unplaced.
    let fleet_total = (3..11).count();
    if fleet_total != 8 || bob_hosted == fleet_total {
        return Ok(TestResult::fail(
            NAME,
            "leg 4: the fleet total must exceed the smallest peer's capacity — placement capped",
        ));
    }

    // --- Leg 5 (A14): a shard is never left homeless. ----------------------
    // Both peers declare ~zero capacity; a brand-new shard still gets a home
    // (the least-utilized last resort) — capacity is a planning hint, not a wall.
    alice
        .sync_cmd_tx
        .send(SyncCommand::SetShardDeclaration {
            capacity_bytes: Some(1),
            seeder: false,
        })
        .map_err(|e| anyhow::anyhow!("failed to send alice's starved declaration: {e}"))?;
    bob.sync_cmd_tx
        .send(SyncCommand::SetShardDeclaration {
            capacity_bytes: Some(1),
            seeder: false,
        })
        .map_err(|e| anyhow::anyhow!("failed to send bob's starved declaration: {e}"))?;
    poll_ledger(&alice, &verse_id, |d| {
        d["peers"][alice_did.as_str()]["capacity_bytes"].as_u64() == Some(1)
            && d["peers"][bob_did.as_str()]["capacity_bytes"].as_u64() == Some(1)
    })
    .map_err(|e| anyhow::anyhow!("leg 5: the starved declarations never converged: {e}"))?;
    let reading_x = reading_row("reading-f6-x", 15.0, 60)?;
    publish_row(&alice, &verse_id, "iot_reading", "reading-f6-x", &reading_x)?;
    let shard_x = "petal-f6/node-f6/29209740";
    let dump = poll_ledger(&alice, &verse_id, |d| {
        d["shards"][shard_x].is_object()
            && d["shards"][shard_x]["hosts"]
                .as_array()
                .map(|h| !h.is_empty())
                .unwrap_or(false)
    })
    .map_err(|e| anyhow::anyhow!("leg 5: the homeless-shard ledger never appeared: {e}"))?;
    let hosts = dump["shards"][shard_x]["hosts"].as_array().unwrap();
    if hosts.is_empty() {
        return Ok(TestResult::fail(
            NAME,
            "leg 5: a shard with no eligible peer was left homeless — capacity must be a hint, not a wall",
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
