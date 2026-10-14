//! Bevy `AssetReader` backed by a `BlobStoreHandle` (P2P Mycelium Phase B).
//!
//! When registered as the `"blob"` asset source, Bevy resolves paths like
//! `blob://{hex_hash}.glb` through this reader. The `read` method extracts the
//! hex hash from the filename stem, looks up the blob on disk via
//! `BlobStore::get_blob_path`, and returns the file bytes -- unless the KTX2
//! intake guard refuses them (src/AGENTS.md §ktx2-guard).

use bevy::asset::io::{AssetReader, AssetReaderError, PathStream, Reader, VecReader};
use std::path::Path;
use std::sync::Arc;

use crate::blob_store::{hash_from_hex, BlobHash, BlobStoreHandle};

/// Callback invoked when a blob is not found locally.
///
/// The callback receives the missing blob's hash so that the sync layer can
/// attempt a peer fetch.  Set by `fe-sync` at startup — `fe-runtime` knows
/// nothing about the sync crate, avoiding a dependency cycle.
pub type OnMissCallback = Arc<dyn Fn(BlobHash) + Send + Sync>;

/// An [`AssetReader`] that reads files from a content-addressed blob store.
///
/// Register with Bevy via:
/// ```ignore
/// app.register_asset_source("blob", AssetSourceBuilder::new(move || {
///     Box::new(BlobAssetReader::new(blob_store.clone()))
/// }));
/// ```
pub struct BlobAssetReader {
    blob_store: BlobStoreHandle,
    on_miss: Option<OnMissCallback>,
}

impl BlobAssetReader {
    pub fn new(blob_store: BlobStoreHandle) -> Self {
        Self {
            blob_store,
            on_miss: None,
        }
    }

    /// Create a reader with an `on_miss` callback that fires when a blob
    /// lookup returns `NotFound`.
    pub fn with_on_miss(blob_store: BlobStoreHandle, on_miss: OnMissCallback) -> Self {
        Self {
            blob_store,
            on_miss: Some(on_miss),
        }
    }

    /// Parse the hex hash from a path like `{64-char-hex}.glb`.
    fn resolve_blob_path(&self, path: &Path) -> Result<std::path::PathBuf, AssetReaderError> {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))?;

        let hash = hash_from_hex(stem).map_err(|_| {
            tracing::warn!(
                "BlobAssetReader: invalid hex hash in path: {}",
                path.display()
            );
            AssetReaderError::NotFound(path.to_path_buf())
        })?;

        self.blob_store.get_blob_path(&hash).ok_or_else(|| {
            tracing::warn!(
                "BlobAssetReader: blob not found for hash {} (path: {})",
                stem,
                path.display()
            );
            AssetReaderError::NotFound(path.to_path_buf())
        })
    }
}

impl AssetReader for BlobAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        match self.resolve_blob_path(path) {
            Ok(blob_path) => {
                let bytes = std::fs::read(&blob_path).map_err(|e| {
                    tracing::error!(
                        "BlobAssetReader: failed to read blob at {}: {e}",
                        blob_path.display()
                    );
                    AssetReaderError::Io(Arc::new(e))
                })?;
                // Every blob:// load converges here, whatever put the bytes in
                // the store (REST, MCP, peer sync, hexon install, import).
                let extension = path.extension().and_then(|e| e.to_str());
                crate::ktx2_guard::check_asset_bytes(&bytes, extension).map_err(|refusal| {
                    tracing::warn!("BlobAssetReader: refused {}: {refusal}", path.display());
                    AssetReaderError::Io(Arc::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        refusal,
                    )))
                })?;
                Ok(VecReader::new(bytes))
            }
            Err(e) => {
                // Fire the on_miss callback so the sync thread can attempt
                // a peer fetch.  We still return NotFound — the asset will
                // be available on a subsequent load after the fetch completes.
                if let Some(ref cb) = self.on_miss {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(hash) = hash_from_hex(stem) {
                            tracing::debug!(
                                hash = %stem,
                                "BlobAssetReader: miss — notifying sync layer"
                            );
                            cb(hash);
                        }
                    }
                }
                Err(e)
            }
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Blob store does not support .meta files.
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_store::hash_to_hex;

    #[test]
    fn resolve_blob_path_returns_err_for_bad_hex() {
        let store: BlobStoreHandle =
            std::sync::Arc::new(crate::blob_store::mock::MockBlobStore::new());
        let reader = BlobAssetReader::new(store);
        let result = reader.resolve_blob_path(Path::new("not-a-hash.glb"));
        assert!(result.is_err());
    }

    /// A real on-disk store under the temp dir, laid out canonically.
    struct DirBlobStore(std::path::PathBuf);

    impl crate::blob_store::BlobStore for DirBlobStore {
        fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
            let hash: BlobHash = *blake3::hash(bytes).as_bytes();
            let path = crate::blob_store::canonical_blob_path(&self.0, &hash);
            std::fs::create_dir_all(path.parent().expect("sharded path"))?;
            std::fs::write(path, bytes)?;
            Ok(hash)
        }
        fn get_blob_path(&self, hash: &BlobHash) -> Option<std::path::PathBuf> {
            let path = crate::blob_store::canonical_blob_path(&self.0, hash);
            path.exists().then_some(path)
        }
        fn has_blob(&self, hash: &BlobHash) -> bool {
            self.get_blob_path(hash).is_some()
        }
        fn remove_blob(&self, _hash: &BlobHash) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// The choke point: a blob that arrived by any route is refused at load
    /// time when it carries a KTX2 texture, and served when it does not.
    #[test]
    fn blob_reads_refuse_ktx2_glbs_and_serve_plain_ones() {
        use crate::blob_store::BlobStore as _;
        use crate::ktx2_guard::tests::build_glb;
        let root = std::env::temp_dir().join(format!(
            "fe-ktx2-guard-reader-{}-{}",
            std::process::id(),
            nanos_suffix()
        ));
        let store = Arc::new(DirBlobStore(root.clone()));
        let reader = BlobAssetReader::new(store.clone());

        let plain = build_glb(r#"{"asset":{"version":"2.0"}}"#, None);
        let ktx2 = build_glb(
            r#"{"images":[{"uri":"t.png","mimeType":"image/ktx2"}]}"#,
            None,
        );
        // A JSON glTF stored as a `.glb` blob: bevy's gltf parser sniffs the
        // content, so the `.glb` name must not let it skip the guard.
        let ktx2_json = br#"{"images":[{"uri":"data:image%2Fktx2%3Bbase64,AAAA"}]}"#;
        let plain_path = format!("{}.glb", hash_to_hex(&store.add_blob(&plain).unwrap()));
        let ktx2_path = format!("{}.glb", hash_to_hex(&store.add_blob(&ktx2).unwrap()));
        let json_path = format!("{}.glb", hash_to_hex(&store.add_blob(ktx2_json).unwrap()));

        let read_error = |path: &str| match bevy::tasks::block_on(reader.read(Path::new(path))) {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        let plain_error = read_error(&plain_path);
        let refusals = [read_error(&ktx2_path), read_error(&json_path)];
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(plain_error, None, "a plain GLB must load");
        for (label, refusal) in ["KTX2 GLB", "KTX2 JSON glTF named .glb"]
            .iter()
            .zip(refusals)
        {
            let refusal = refusal.unwrap_or_else(|| panic!("{label} must be refused"));
            assert!(refusal.contains("RUSTSEC-2026-0344"), "{label}: {refusal}");
        }
    }

    fn nanos_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    #[test]
    fn resolve_blob_path_returns_err_for_missing_blob() {
        let store: BlobStoreHandle =
            std::sync::Arc::new(crate::blob_store::mock::MockBlobStore::new());
        let reader = BlobAssetReader::new(store);
        let fake_hash = hash_to_hex(&[0u8; 32]);
        let result = reader.resolve_blob_path(Path::new(&format!("{fake_hash}.glb")));
        assert!(result.is_err());
    }
}
