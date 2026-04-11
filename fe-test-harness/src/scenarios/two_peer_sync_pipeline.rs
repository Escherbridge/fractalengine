//! Scenario 7: Two-Peer Sync Pipeline
//!
//! Tests the full sync command pipeline across two peers sharing a verse:
//! 1. Alice creates a verse and opens a replica via SyncCommand::OpenVerseReplica
//! 2. Alice writes a row entry to the replica (serialized verse JSON -> blob store -> WriteRowEntry)
//! 3. Bob opens a replica for the same verse (using the namespace_id from Alice's invite)
//! 4. Manually copy the blob from Alice's store to Bob's store (simulating network fetch)
//! 5. Bob writes the same row entry to his replica
//! 6. Verify both peers have the blob and no panics occurred
//! 7. Both peers close their replicas
//!
//! This exercises the full sync infrastructure pipeline for two peers.

use anyhow::Result;
use fe_database::hash_to_hex;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_sync::messages::SyncCommand;

use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;

    // 1. Alice seeds and creates a verse
    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    alice.send(DbCommand::CreateVerse {
        name: "Sync Pipeline Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, .. } => id.clone(),
        _ => unreachable!(),
    };

    // 2. Alice generates an invite so Bob can get the namespace_id
    alice.send(DbCommand::GenerateVerseInvite {
        verse_id: verse_id.clone(),
        include_write_cap: true,
        expiry_hours: 24,
    });
    let invite_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseInviteGenerated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let invite_string = match &invite_result {
        DbResult::VerseInviteGenerated { invite_string, .. } => invite_string.clone(),
        _ => unreachable!(),
    };

    // Parse the invite to extract namespace_id and namespace_secret
    let invite = fe_database::invite::VerseInvite::from_invite_string(&invite_string)?;
    let namespace_id = invite.namespace_id.clone();
    let namespace_secret = invite.namespace_secret.clone();

    // 3. Alice opens a verse replica via sync command
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: namespace_id.clone(),
            namespace_secret: namespace_secret.clone(),
        })
        .map_err(|e| anyhow::anyhow!("Failed to send OpenVerseReplica to Alice: {e}"))?;

    // Give the sync thread a moment to process
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Alice writes a row entry (serialized verse JSON -> blob store -> WriteRowEntry)
    let row_data = serde_json::json!({
        "verse_id": verse_id,
        "name": "Sync Pipeline Verse",
        "created_by": "alice",
    });
    let row_bytes = serde_json::to_vec(&row_data)?;
    let alice_hash = alice.blob_store.add_blob(&row_bytes)?;

    alice
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "verse".into(),
            record_id: verse_id.clone(),
            content_hash: alice_hash,
        })
        .map_err(|e| anyhow::anyhow!("Failed to send WriteRowEntry to Alice: {e}"))?;

    std::thread::sleep(std::time::Duration::from_millis(500));

    // 5. Bob seeds and joins the verse via invite
    bob.send(DbCommand::Seed);
    bob.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    bob.send(DbCommand::JoinVerseByInvite {
        invite_string: invite_string.clone(),
    });
    bob.wait_for(
        |r| matches!(r, DbResult::VerseJoined { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 6. Bob opens a replica for the same verse
    bob.sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: namespace_id.clone(),
            namespace_secret: namespace_secret.clone(),
        })
        .map_err(|e| anyhow::anyhow!("Failed to send OpenVerseReplica to Bob: {e}"))?;

    std::thread::sleep(std::time::Duration::from_millis(500));

    // 7. Manually copy the blob from Alice's store to Bob's store (simulating network fetch)
    let alice_blob_path = alice
        .blob_store
        .get_blob_path(&alice_hash)
        .ok_or_else(|| anyhow::anyhow!("Alice's blob store missing the row data blob"))?;
    let blob_bytes = std::fs::read(&alice_blob_path)?;
    let bob_hash = bob.blob_store.add_blob(&blob_bytes)?;

    // Verify hashes match
    if bob_hash != alice_hash {
        return Ok(TestResult::fail(
            "two_peer_sync_pipeline",
            &format!(
                "Blob hash mismatch after copy: Alice={}, Bob={}",
                hash_to_hex(&alice_hash),
                hash_to_hex(&bob_hash)
            ),
        ));
    }

    // 8. Bob writes the same row entry to his replica
    bob.sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "verse".into(),
            record_id: verse_id.clone(),
            content_hash: bob_hash,
        })
        .map_err(|e| anyhow::anyhow!("Failed to send WriteRowEntry to Bob: {e}"))?;

    std::thread::sleep(std::time::Duration::from_millis(500));

    // 9. Verify both peers have the blob
    if !alice.blob_store.has_blob(&alice_hash) {
        return Ok(TestResult::fail(
            "two_peer_sync_pipeline",
            "Alice's blob store lost the row data blob",
        ));
    }

    if !bob.blob_store.has_blob(&bob_hash) {
        return Ok(TestResult::fail(
            "two_peer_sync_pipeline",
            "Bob's blob store does not have the row data blob",
        ));
    }

    // 10. Close replicas on both peers
    alice
        .sync_cmd_tx
        .send(SyncCommand::CloseVerseReplica {
            verse_id: verse_id.clone(),
        })
        .map_err(|e| anyhow::anyhow!("Failed to send CloseVerseReplica to Alice: {e}"))?;

    bob.sync_cmd_tx
        .send(SyncCommand::CloseVerseReplica {
            verse_id: verse_id.clone(),
        })
        .map_err(|e| anyhow::anyhow!("Failed to send CloseVerseReplica to Bob: {e}"))?;

    std::thread::sleep(std::time::Duration::from_millis(200));

    tracing::info!(
        "Two-peer sync pipeline verified: verse_id={}, blob_hash={}",
        verse_id,
        hash_to_hex(&alice_hash)
    );

    drop(alice);
    drop(bob);
    Ok(TestResult::pass("two_peer_sync_pipeline"))
}
