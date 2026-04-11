//! Scenario 5: Two-Peer Blob Exchange
//!
//! Two peers exchange a blob by manually copying it (simulating what the sync
//! thread would do over the network):
//! 1. Alice spawns, seeds DB, imports a GLB file -> gets blob hash
//! 2. Bob spawns
//! 3. Read the blob bytes from Alice's blob store, write them to Bob's blob store
//! 4. Verify Bob's blob store now has the same hash and identical bytes
//! 5. Verify Bob can construct the same `blob://` URL
//!
//! This proves the blob store is content-addressable and portable across peers.

use anyhow::Result;
use fe_database::hash_to_hex;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::fixtures::create_minimal_glb;
use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;

    // 1. Alice spawns and seeds her DB
    let alice = TestPeer::spawn("alice", tmp.path())?;
    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // Get Alice's petal_id from hierarchy
    alice.send(DbCommand::LoadHierarchy);
    let hierarchy = alice.wait_for(
        |r| matches!(r, DbResult::HierarchyLoaded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let petal_id = match &hierarchy {
        DbResult::HierarchyLoaded { verses } => verses
            .first()
            .and_then(|v| v.fractals.first())
            .and_then(|f| f.petals.first())
            .map(|p| p.id.clone())
            .ok_or_else(|| anyhow::anyhow!("No petal found after seed"))?,
        _ => unreachable!(),
    };

    // 2. Create a minimal GLB file and import it via Alice
    let glb_bytes = create_minimal_glb();
    let glb_path = tmp.path().join("exchange_test.glb");
    std::fs::write(&glb_path, &glb_bytes)?;

    alice.send(DbCommand::ImportGltf {
        petal_id,
        name: "ExchangeTestCube".into(),
        file_path: glb_path.to_str().unwrap().into(),
        position: [0.0, 0.0, 0.0],
    });

    let import_result = alice.wait_for(
        |r| matches!(r, DbResult::GltfImported { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let alice_asset_path = match &import_result {
        DbResult::GltfImported { asset_path, .. } => asset_path.clone(),
        _ => unreachable!(),
    };

    // Extract hash from Alice's asset_path
    let alice_hash_hex = alice_asset_path
        .strip_prefix("blob://")
        .and_then(|s| s.strip_suffix(".glb"))
        .ok_or_else(|| {
            anyhow::anyhow!("Unexpected asset_path format: {}", alice_asset_path)
        })?;

    let alice_hash = fe_database::hash_from_hex(alice_hash_hex)?;

    // 3. Bob spawns
    let bob = TestPeer::spawn("bob", tmp.path())?;

    // 4. Read blob bytes from Alice's store, write to Bob's store
    let alice_blob_path = alice
        .blob_store
        .get_blob_path(&alice_hash)
        .ok_or_else(|| anyhow::anyhow!("Alice's blob store missing the imported blob"))?;
    let blob_bytes = std::fs::read(&alice_blob_path)?;

    let bob_hash = bob.blob_store.add_blob(&blob_bytes)?;

    // 5. Verify Bob's hash matches Alice's hash
    if bob_hash != alice_hash {
        return Ok(TestResult::fail(
            "two_peer_blob_exchange",
            &format!(
                "Hash mismatch: Alice={}, Bob={}",
                hash_to_hex(&alice_hash),
                hash_to_hex(&bob_hash)
            ),
        ));
    }

    // 6. Verify Bob's blob store has the blob
    if !bob.blob_store.has_blob(&bob_hash) {
        return Ok(TestResult::fail(
            "two_peer_blob_exchange",
            "Bob's blob store does not contain the blob after add_blob",
        ));
    }

    // 7. Verify Bob's bytes are identical
    let bob_blob_path = bob
        .blob_store
        .get_blob_path(&bob_hash)
        .ok_or_else(|| anyhow::anyhow!("Bob's blob store missing blob after add"))?;
    let bob_bytes = std::fs::read(&bob_blob_path)?;

    if bob_bytes != blob_bytes {
        return Ok(TestResult::fail(
            "two_peer_blob_exchange",
            "Bob's blob bytes do not match Alice's blob bytes",
        ));
    }

    // 8. Verify Bob can construct the same blob:// URL
    let bob_asset_path = format!("blob://{}.glb", hash_to_hex(&bob_hash));
    if bob_asset_path != alice_asset_path {
        return Ok(TestResult::fail(
            "two_peer_blob_exchange",
            &format!(
                "Asset path mismatch: Alice='{}', Bob='{}'",
                alice_asset_path, bob_asset_path
            ),
        ));
    }

    tracing::info!(
        "Two-peer blob exchange verified: hash={}, path={}",
        hash_to_hex(&bob_hash),
        bob_asset_path
    );

    drop(alice);
    drop(bob);
    Ok(TestResult::pass("two_peer_blob_exchange"))
}
