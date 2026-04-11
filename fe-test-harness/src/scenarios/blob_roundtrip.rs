//! Scenario 1: Blob Store Roundtrip
//!
//! Verifies that importing a GLB file via `ImportGltf`:
//! 1. Writes the bytes to the blob store
//! 2. Returns an `asset_path` using the `blob://` scheme
//! 3. The BLAKE3 hash in the path matches the original file bytes
//! 4. The blob store contains the exact original bytes

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::fixtures::create_minimal_glb;
use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let peer = TestPeer::spawn("alice", tmp.path())?;

    // 1. Seed to get a petal_id
    peer.send(DbCommand::Seed);
    let _seed_result = peer.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // We need a petal_id. Load hierarchy to find it.
    peer.send(DbCommand::LoadHierarchy);
    let hierarchy = peer.wait_for(
        |r| matches!(r, DbResult::HierarchyLoaded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let petal_id = match &hierarchy {
        DbResult::HierarchyLoaded { verses } => {
            verses
                .first()
                .and_then(|v| v.fractals.first())
                .and_then(|f| f.petals.first())
                .map(|p| p.id.clone())
                .ok_or_else(|| anyhow::anyhow!("No petal found after seed"))?
        }
        _ => unreachable!(),
    };

    // 2. Create a minimal GLB file
    let glb_bytes = create_minimal_glb();
    let glb_path = tmp.path().join("test_cube.glb");
    std::fs::write(&glb_path, &glb_bytes)?;

    // 3. Import the GLB
    peer.send(DbCommand::ImportGltf {
        petal_id: petal_id.clone(),
        name: "TestCube".into(),
        file_path: glb_path.to_str().unwrap().into(),
        position: [0.0, 0.0, 0.0],
    });

    let import_result = peer.wait_for(
        |r| matches!(r, DbResult::GltfImported { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let (asset_path, _node_id) = match &import_result {
        DbResult::GltfImported {
            asset_path,
            node_id,
            ..
        } => (asset_path.clone(), node_id.clone()),
        _ => unreachable!(),
    };

    // 4. Verify asset_path uses blob:// scheme
    if !asset_path.starts_with("blob://") {
        return Ok(TestResult::fail(
            "blob_roundtrip",
            &format!(
                "asset_path does not start with blob://: {}",
                asset_path
            ),
        ));
    }

    // 5. Extract the hash from the asset_path (format: blob://{hash}.glb)
    let hash_part = asset_path
        .strip_prefix("blob://")
        .unwrap()
        .strip_suffix(".glb")
        .ok_or_else(|| {
            anyhow::anyhow!("asset_path does not end with .glb: {}", asset_path)
        })?;

    // 6. Verify BLAKE3 hash matches
    let expected_hash = blake3::hash(&glb_bytes);
    let expected_hex = hex::encode(expected_hash.as_bytes());

    if hash_part != expected_hex {
        return Ok(TestResult::fail(
            "blob_roundtrip",
            &format!(
                "BLAKE3 hash mismatch: path has '{}', expected '{}'",
                hash_part, expected_hex
            ),
        ));
    }

    // 7. Verify blob store has the content
    let blob_hash = fe_database::hash_from_hex(hash_part)?;
    if !peer.blob_store.has_blob(&blob_hash) {
        return Ok(TestResult::fail(
            "blob_roundtrip",
            "blob store does not contain the imported blob",
        ));
    }

    // 8. Verify the bytes on disk match the original
    if let Some(blob_path) = peer.blob_store.get_blob_path(&blob_hash) {
        let stored_bytes = std::fs::read(&blob_path)?;
        if stored_bytes != glb_bytes {
            return Ok(TestResult::fail(
                "blob_roundtrip",
                "blob store bytes do not match original GLB bytes",
            ));
        }
    }

    drop(peer);
    Ok(TestResult::pass("blob_roundtrip"))
}
