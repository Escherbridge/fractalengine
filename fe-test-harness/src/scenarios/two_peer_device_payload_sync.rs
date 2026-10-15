//! Scenario 17: Two-Peer Device-Payload Flow — see fe-test-harness/src/AGENTS.md
//! §scenarios (CDL-SPEC10 A6).
//!
//! Alice declares a `device_format`, creates a fractal/petal/node scaffold and
//! two generic `asset` rows, then ingests a `device_payload` referencing both
//! assets through the real `DbCommand::InsertDevicePayloads` door. Bob, joined
//! to the same verse replica, receives the `device_format`, the two anchored
//! `asset` rows and the `device_payload` row over the real transport (union
//! CRDT — A3's R5 referential-confinement gate requires the petal/fractal/
//! node/device_format to already be locally known on bob's side, which is why
//! the scaffold is published and confirmed BEFORE the payload ingest).
//!
//! Legs:
//! 1. `device_format`/`asset`×2/`device_payload` all cross and apply, in that
//!    order (READ-BACK from bob's durable store; `asset_ids` keep order).
//! 2. bob's blob miss for asset 1's content, `SyncCommand::FetchBlob`, and
//!    byte-identical content served by `content_hash` once present — real
//!    P2P blob content transfer is still a Phase-D stub
//!    (`fe-sync/src/sync_thread.rs::handle_fetch_blob`, "peer discovery not
//!    yet implemented"), so the byte motion itself mirrors the established
//!    `two_peer_blob_exchange.rs` precedent (a direct store-to-store copy);
//!    the command/event wiring (`FetchBlob` → `BlobReady`) is exercised for
//!    real once the content is locally present.
//! 3. a re-delivered `device_payload` row (same `payload_id`) does not
//!    duplicate.
//!
//! The fractal/petal/node/device_format rows are published the way
//! `two_peer_timeseries_sync.rs`/`two_peer_tombstone_sync.rs` publish their
//! manual rows (`publish_row`: blob-store the row bytes, then
//! `SyncCommand::WriteRowEntry`) rather than through a replicating DbCommand,
//! because none of `CreateFractal`/`CreatePetal`/`CreateNode`/
//! `RegisterDeviceFormat` carry a replication sender in this harness (or, for
//! `RegisterDeviceFormat`, in the real `fe-database` handler either — see
//! fe-test-harness `src/AGENTS.md` §scenarios row 17's open-issue note). Only
//! `InsertDevicePayloads` replicates on its own (it explicitly emits the
//! referenced `asset` rows, then the `device_payload` row, through
//! `crate::replicate_row_with_petal` — CDL-SPEC10 A3).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use fe_runtime::messages::{
    DbCommand, DbResult, DevicePayloadInput, ReplicatedRowOutcome, SourceClass,
};
use fe_sdk::device_format::DeviceFormatDescriptor;
use fe_sync::messages::{SyncCommand, SyncEvent};

use crate::peer::TestPeer;
use crate::TestResult;

const NAME: &str = "two_peer_device_payload_sync";

/// How long to wait for a row to cross the real transport.
const SYNC_TIMEOUT: Duration = Duration::from_secs(45);

/// Publish one row the way the emission seam does (M2/F7 shape, reused by
/// every two-peer scenario that hand-constructs a row): blob-store the row
/// bytes, then hand the content hash to the sync thread as a `WriteRowEntry`.
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
            table: table.to_string(),
            record_id: record_id.to_string(),
            content_hash,
        })
        .map_err(|e| {
            anyhow::anyhow!("failed to send WriteRowEntry for {table}/{record_id}: {e}")
        })?;
    Ok(())
}

/// Wait for `peer`'s apply-outcome echo for one specific `(table, record_id)`
/// — matching on the key, not just the table, so a multi-row leg (two
/// `asset` rows) cannot pick up the wrong echo out of order.
fn wait_row_outcome(peer: &TestPeer, table: &str, record_id: &str) -> Result<ReplicatedRowOutcome> {
    match peer.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied { table: t, record_id: rid, .. }
                    if t == table && rid == record_id
            )
        },
        SYNC_TIMEOUT,
    )? {
        DbResult::ReplicatedRowApplied { outcome, .. } => Ok(outcome),
        other => {
            anyhow::bail!("unexpected result while waiting for {table}/{record_id}: {other:?}")
        }
    }
}

/// READ-BACK straight from a peer's durable store.
fn raw_query(
    peer: &TestPeer,
    sql: &str,
    vars: HashMap<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>> {
    peer.send(DbCommand::RawQuery {
        sql: sql.to_string(),
        vars,
        correlation_id: None,
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::QueryResult { data, .. } => Ok(data),
        other => anyhow::bail!("unexpected read-back result: {other:?}"),
    }
}

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

    // 1. Alice creates the verse, then her own local fractal/petal/node
    //    scaffold and device-format declaration (all local-only so far —
    //    nothing has crossed the wire yet).
    alice.send(DbCommand::CreateVerse {
        name: "Device Verse".into(),
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

    alice.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "Device Fractal".into(),
    });
    let fractal_id = match alice.wait_for(
        |r| matches!(r, DbResult::FractalCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::FractalCreated { id, .. } => id,
        _ => unreachable!(),
    };

    alice.send(DbCommand::CreatePetal {
        fractal_id: fractal_id.clone(),
        name: "Device Petal".into(),
    });
    let petal_id = match alice.wait_for(
        |r| matches!(r, DbResult::PetalCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::PetalCreated { id, .. } => id,
        _ => unreachable!(),
    };

    alice.send(DbCommand::CreateNode {
        petal_id: petal_id.clone(),
        name: "Device Node".into(),
        position: [0.0, 0.0, 0.0],
        correlation_id: None,
    });
    let node_id = match alice.wait_for(
        |r| matches!(r, DbResult::NodeCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::NodeCreated { id, .. } => id,
        _ => unreachable!(),
    };

    let format_id = "acme.sensor/frame@1".to_string();
    let mime = "application/octet-stream".to_string();
    alice.send(DbCommand::RegisterDeviceFormat {
        verse_id: verse_id.clone(),
        descriptor: DeviceFormatDescriptor {
            format_id: format_id.clone(),
            mime: mime.clone(),
            schema: None,
            description: "A6 harness scenario test frame".into(),
        },
        declared_by: alice_did.clone(),
        correlation_id: None,
    });
    match alice.wait_for(
        |r| {
            matches!(
                r,
                DbResult::DeviceFormatRegistered { .. } | DbResult::DeviceFormatRejected { .. }
            )
        },
        Duration::from_secs(30),
    )? {
        DbResult::DeviceFormatRegistered { .. } => {}
        DbResult::DeviceFormatRejected { reason, .. } => {
            return Ok(TestResult::fail(
                NAME,
                &format!("alice's own format registration was rejected: {reason}"),
            ));
        }
        _ => unreachable!(),
    }

    // 2. Open the verse replica on both sides (bob dials alice).
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

    // 3. Converge the verse manifest first (bootstrap admission on bob's
    //    side, A3) — alice (created_by) then resolves as Owner for every
    //    row she authors from here on.
    let now = chrono::Utc::now().to_rfc3339();
    let verse_row = serde_json::json!({
        "verse_id": verse_id,
        "name": "Device Verse",
        "created_by": alice_did,
        "created_at": now,
        "namespace_id": hex::encode(fe_database::derive_namespace_id(&secret_bytes)),
        "default_access": "viewer",
    });
    publish_row(
        &alice,
        &verse_id,
        "verse",
        &verse_id,
        &serde_json::to_vec(&verse_row)?,
    )?;
    let outcome = wait_row_outcome(&bob, "verse", &verse_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("verse manifest did not converge on bob (outcome: {outcome:?})"),
        ));
    }

    // 4. Publish the fractal/petal/node scaffold bob needs so the R5
    //    referential-confinement gate (A3) can resolve the petal → verse
    //    scope and the device-payload anchor once the payload arrives.
    let fractal_row = serde_json::json!({
        "fractal_id": fractal_id,
        "verse_id": verse_id,
        "owner_did": alice_did,
        "name": "Device Fractal",
        "created_at": now,
    });
    publish_row(
        &alice,
        &verse_id,
        "fractal",
        &fractal_id,
        &serde_json::to_vec(&fractal_row)?,
    )?;
    let outcome = wait_row_outcome(&bob, "fractal", &fractal_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("fractal did not converge on bob (outcome: {outcome:?})"),
        ));
    }

    let petal_row = serde_json::json!({
        "petal_id": petal_id,
        "name": "Device Petal",
        "node_id": alice_did,
        "created_at": now,
        "fractal_id": fractal_id,
    });
    publish_row(
        &alice,
        &verse_id,
        "petal",
        &petal_id,
        &serde_json::to_vec(&petal_row)?,
    )?;
    let outcome = wait_row_outcome(&bob, "petal", &petal_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("petal did not converge on bob (outcome: {outcome:?})"),
        ));
    }

    let node_row = serde_json::json!({
        "node_id": node_id,
        "petal_id": petal_id,
        "display_name": "Device Node",
        "position": { "type": "Point", "coordinates": [0.0, 0.0] },
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
        "created_at": now,
    });
    publish_row(
        &alice,
        &verse_id,
        "node",
        &node_id,
        &serde_json::to_vec(&node_row)?,
    )?;
    let outcome = wait_row_outcome(&bob, "node", &node_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("node did not converge on bob (outcome: {outcome:?})"),
        ));
    }

    // 5. Publish the device_format declaration. The admission gate requires
    //    Manager+ at the verse scope — alice (Owner) satisfies it.
    let format_row = serde_json::json!({
        "format_id": format_id,
        "verse_id": verse_id,
        "mime": mime,
        "declared_by": alice_did,
        "created_at": now,
    });
    publish_row(
        &alice,
        &verse_id,
        "device_format",
        &format_id,
        &serde_json::to_vec(&format_row)?,
    )?;
    let outcome = wait_row_outcome(&bob, "device_format", &format_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("device_format did not converge on bob (outcome: {outcome:?})"),
        ));
    }

    // 6. Alice stores two blobs and registers them as generic `asset` rows
    //    anchored to her petal (local-only metadata — `insert_device_payloads`
    //    is what replicates them, below).
    let blob1 = b"device payload blob one".to_vec();
    let blob2 = b"device payload blob two".to_vec();
    let hash1 = alice.blob_store.add_blob(&blob1)?;
    let hash2 = alice.blob_store.add_blob(&blob2)?;
    let asset_id1 = "asset-dp1".to_string();
    let asset_id2 = "asset-dp2".to_string();
    for (asset_id, hash, bytes) in [(&asset_id1, &hash1, &blob1), (&asset_id2, &hash2, &blob2)] {
        let mut vars = HashMap::new();
        vars.insert("asset_id".into(), serde_json::json!(asset_id));
        vars.insert("name".into(), serde_json::json!(format!("{asset_id}.bin")));
        vars.insert("content_type".into(), serde_json::json!(mime));
        vars.insert("size_bytes".into(), serde_json::json!(bytes.len() as i64));
        vars.insert(
            "content_hash".into(),
            serde_json::json!(fe_database::hash_to_hex(hash)),
        );
        vars.insert("owner_did".into(), serde_json::json!(alice_did));
        vars.insert("petal_id".into(), serde_json::json!(petal_id));
        vars.insert(
            "now".into(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );
        raw_query(
            &alice,
            "CREATE asset CONTENT { \
             asset_id: $asset_id, name: $name, content_type: $content_type, \
             size_bytes: $size_bytes, data: NONE, created_at: $now, \
             content_hash: $content_hash, owner_did: $owner_did, petal_id: $petal_id }",
            vars,
        )?;
    }

    // 7. Alice ingests the device payload through the real door —
    //    `insert_device_payloads` validates everything, writes durably, then
    //    replicates the two asset rows first and the payload row second.
    alice.send(DbCommand::InsertDevicePayloads {
        petal_id: petal_id.clone(),
        verse_id: Some(verse_id.clone()),
        source_did: alice_did.clone(),
        source_class: SourceClass::Device,
        payload: DevicePayloadInput {
            node_id: node_id.clone(),
            format_id: format_id.clone(),
            asset_ids: vec![asset_id1.clone(), asset_id2.clone()],
            recorded_at: None,
            // A client retry id is a ULID (the door refuses anything else).
            payload_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
        },
        correlation_id: None,
    });
    let payload_id = match alice.wait_for(
        |r| {
            matches!(
                r,
                DbResult::DevicePayloadsInserted { .. } | DbResult::DevicePayloadsRejected { .. }
            )
        },
        Duration::from_secs(30),
    )? {
        DbResult::DevicePayloadsInserted { payload_id, .. } => payload_id,
        DbResult::DevicePayloadsRejected { reason, .. } => {
            return Ok(TestResult::fail(
                NAME,
                &format!("alice's own device-payload ingest was rejected: {reason}"),
            ));
        }
        _ => unreachable!(),
    };

    // 8. Bob receives the two asset rows and the device_payload row (union).
    let outcome = wait_row_outcome(&bob, "asset", &asset_id1)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("asset 1 did not apply on bob (outcome: {outcome:?})"),
        ));
    }
    let outcome = wait_row_outcome(&bob, "asset", &asset_id2)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("asset 2 did not apply on bob (outcome: {outcome:?})"),
        ));
    }
    let outcome = wait_row_outcome(&bob, "device_payload", &payload_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("device_payload did not apply on bob (outcome: {outcome:?})"),
        ));
    }

    // READ-BACK from bob's durable store: one row, asset_ids keep their order.
    let mut vars = HashMap::new();
    vars.insert("pid".into(), serde_json::json!(payload_id));
    let rows = raw_query(
        &bob,
        "SELECT * FROM device_payload WHERE payload_id = $pid",
        vars,
    )?;
    if rows.len() != 1 {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "READ-BACK failed: expected 1 device_payload row on bob, got {}",
                rows.len()
            ),
        ));
    }
    let bob_asset_ids: Vec<String> = rows[0]["asset_ids"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if bob_asset_ids != vec![asset_id1.clone(), asset_id2.clone()] {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "READ-BACK mismatch: expected ordered asset_ids [{asset_id1}, {asset_id2}], got {bob_asset_ids:?}"
            ),
        ));
    }

    // 9. Bob's blob miss for asset 1 triggers FetchBlob; serve byte-identical
    //    content by content_hash once present (see the module doc's note on
    //    the Phase-D stub).
    if bob.blob_store.has_blob(&hash1) {
        return Ok(TestResult::fail(
            NAME,
            "bob already had asset 1's blob — the miss precondition is void",
        ));
    }
    bob.sync_cmd_tx
        .send(SyncCommand::FetchBlob {
            hash: hash1,
            verse_id: verse_id.clone(),
        })
        .map_err(|e| anyhow::anyhow!("failed to send FetchBlob (miss) to bob: {e}"))?;

    let alice_path = alice
        .blob_store
        .get_blob_path(&hash1)
        .ok_or_else(|| anyhow::anyhow!("alice is missing her own asset 1 blob"))?;
    let alice_bytes = std::fs::read(&alice_path)?;
    let bob_hash = bob.blob_store.add_blob(&alice_bytes)?;
    if bob_hash != hash1 {
        return Ok(TestResult::fail(
            NAME,
            "bob's recomputed hash does not match alice's content_hash",
        ));
    }

    bob.sync_cmd_tx
        .send(SyncCommand::FetchBlob {
            hash: hash1,
            verse_id: verse_id.clone(),
        })
        .map_err(|e| anyhow::anyhow!("failed to send FetchBlob (hit) to bob: {e}"))?;
    match bob.wait_sync_event(
        |e| matches!(e, SyncEvent::BlobReady { hash } if *hash == hash1),
        Duration::from_secs(10),
    )? {
        SyncEvent::BlobReady { .. } => {}
        other => anyhow::bail!("unexpected sync event while waiting for BlobReady: {other:?}"),
    }
    let bob_bytes = std::fs::read(
        bob.blob_store
            .get_blob_path(&hash1)
            .ok_or_else(|| anyhow::anyhow!("bob is missing the blob after the fetch"))?,
    )?;
    if bob_bytes != alice_bytes {
        return Ok(TestResult::fail(
            NAME,
            "bob's served bytes differ from alice's original content",
        ));
    }

    // 10. A re-delivered payload row does not duplicate.
    let mut vars = HashMap::new();
    vars.insert("pid".into(), serde_json::json!(payload_id));
    let alice_rows = raw_query(
        &alice,
        "SELECT * FROM device_payload WHERE payload_id = $pid",
        vars,
    )?;
    let Some(alice_row) = alice_rows.into_iter().next() else {
        return Ok(TestResult::fail(
            NAME,
            "alice's own device_payload row is missing",
        ));
    };
    publish_row(
        &alice,
        &verse_id,
        "device_payload",
        &payload_id,
        &serde_json::to_vec(&alice_row)?,
    )?;
    let outcome = wait_row_outcome(&bob, "device_payload", &payload_id)?;
    if outcome != ReplicatedRowOutcome::Applied {
        return Ok(TestResult::fail(
            NAME,
            &format!("re-delivery reported a non-idempotent outcome: {outcome:?}"),
        ));
    }
    let mut vars = HashMap::new();
    vars.insert("pid".into(), serde_json::json!(payload_id));
    let rows = raw_query(
        &bob,
        "SELECT * FROM device_payload WHERE payload_id = $pid",
        vars,
    )?;
    if rows.len() != 1 {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "re-delivery duplicated the device_payload row ({} rows)",
                rows.len()
            ),
        ));
    }

    // 11. Clean close on both sides.
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
