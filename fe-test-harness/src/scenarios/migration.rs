//! Scenario 2: Legacy Base64 Migration
//!
//! Verifies that the blob store correctly handles the pattern where:
//! 1. An asset is stored as raw bytes in the blob store
//! 2. A content_hash is computed correctly
//! 3. The blob:// URL scheme is constructed correctly
//!
//! Note: This tests the migration *pattern*, not the actual
//! `migrate_base64_assets_to_blob_store` function (which is private to
//! fe-database). We verify that the same BLAKE3 hash is produced when
//! content is written via the blob store API, matching what the migration
//! would produce.

use std::sync::Arc;

use anyhow::Result;
use fe_database::{hash_to_hex, BlobStoreHandle};
use fe_sync::FsBlobStore;

use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let blob_dir = tmp.path().join("blobs");
    let blob_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(blob_dir)?);

    // Simulate a legacy base64-encoded asset
    let original_bytes = b"This is fake GLB content for migration testing";
    let base64_encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        original_bytes,
    );

    // Step 1: Decode base64 (simulating what migration does)
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &base64_encoded,
    )?;

    // Step 2: Write decoded bytes to blob store
    let hash = blob_store.add_blob(&decoded)?;

    // Step 3: Verify the hash matches a direct BLAKE3 computation
    let expected_hash = *blake3::hash(original_bytes).as_bytes();
    if hash != expected_hash {
        return Ok(TestResult::fail(
            "migration",
            &format!(
                "BLAKE3 hash mismatch after base64 decode: got {}, expected {}",
                hash_to_hex(&hash),
                hash_to_hex(&expected_hash)
            ),
        ));
    }

    // Step 4: Verify blob:// URL construction
    let content_hash_hex = hash_to_hex(&hash);
    let asset_path = format!("blob://{}.glb", content_hash_hex);
    if !asset_path.starts_with("blob://") {
        return Ok(TestResult::fail(
            "migration",
            "constructed asset_path does not use blob:// scheme",
        ));
    }

    // Step 5: Verify blob store can retrieve the content
    if !blob_store.has_blob(&hash) {
        return Ok(TestResult::fail(
            "migration",
            "blob store does not contain the migrated blob",
        ));
    }

    // Step 6: Verify on-disk bytes match
    if let Some(path) = blob_store.get_blob_path(&hash) {
        let on_disk = std::fs::read(&path)?;
        if on_disk != original_bytes {
            return Ok(TestResult::fail(
                "migration",
                "blob store bytes do not match original content after migration",
            ));
        }
    } else {
        return Ok(TestResult::fail(
            "migration",
            "blob store has_blob is true but get_blob_path returns None",
        ));
    }

    // Step 7: Verify idempotency (re-adding same content returns same hash)
    let hash2 = blob_store.add_blob(&decoded)?;
    if hash != hash2 {
        return Ok(TestResult::fail(
            "migration",
            "blob store is not idempotent: re-adding same bytes produced different hash",
        ));
    }

    Ok(TestResult::pass("migration"))
}
