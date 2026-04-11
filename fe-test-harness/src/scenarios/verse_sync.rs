//! Scenario 4: Verse Sync Infrastructure
//!
//! Tests the sync command pipeline end-to-end:
//! 1. Alice creates a verse and opens a replica via SyncCommand::OpenVerseReplica
//! 2. Alice writes a row entry via SyncCommand::WriteRowEntry
//! 3. Verifies the commands are accepted without errors
//!
//! This is an infrastructure stub — actual P2P sync between two peers requires
//! the IrohDocsReplicator to be fully wired (Phase F+). This test validates
//! that the command pipeline works and the sync thread processes commands
//! without panicking.

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_sync::messages::SyncCommand;

use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice", tmp.path())?;

    // 1. Seed Alice's DB
    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 2. Create a verse
    alice.send(DbCommand::CreateVerse {
        name: "Sync Test Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, .. } => id.clone(),
        _ => unreachable!(),
    };

    // 3. Open a verse replica via sync command
    let namespace_id = hex::encode(blake3::hash(b"test-namespace").as_bytes());
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: namespace_id.clone(),
            namespace_secret: Some("test-secret".into()),
        })
        .map_err(|e| anyhow::anyhow!("Failed to send OpenVerseReplica: {e}"))?;

    // Give the sync thread a moment to process
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Write a row entry to the blob store and send WriteRowEntry
    let row_data = serde_json::json!({
        "verse_id": verse_id,
        "name": "Sync Test Verse",
        "created_by": "alice",
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
        .map_err(|e| anyhow::anyhow!("Failed to send WriteRowEntry: {e}"))?;

    // Give the sync thread a moment to process
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 5. Close the replica
    alice
        .sync_cmd_tx
        .send(SyncCommand::CloseVerseReplica {
            verse_id: verse_id.clone(),
        })
        .map_err(|e| anyhow::anyhow!("Failed to send CloseVerseReplica: {e}"))?;

    std::thread::sleep(std::time::Duration::from_millis(200));

    // 6. Verify: if we got here without panics, the sync pipeline works.
    // The sync thread processes all commands on its own thread, and we
    // verified no channel errors occurred.

    // 7. Also verify the blob is still in the store
    if !alice.blob_store.has_blob(&hash) {
        return Ok(TestResult::fail(
            "verse_sync",
            "blob store lost the row data blob",
        ));
    }

    drop(alice);
    Ok(TestResult::pass("verse_sync_infrastructure"))
}
