//! Bevy `AssetReader` backed by a `BlobStoreHandle` (P2P Mycelium Phase B).
//!
//! When registered as the `"blob"` asset source, Bevy resolves paths like
//! `blob://{hex_hash}.glb` through this reader. The `read` method extracts the
//! hex hash from the filename stem, looks up the blob on disk via
//! `BlobStore::get_blob_path`, and returns the file bytes.

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
