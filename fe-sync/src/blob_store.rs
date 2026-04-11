//! Filesystem-backed `BlobStore` implementation (P2P Mycelium Phase A).
//!
//! Blobs are stored as `{root}/{hex[0..2]}/{hex}`, sharded by the first byte of
//! their BLAKE3 hash to avoid dumping thousands of files into one directory.
//!
//! We deliberately do NOT depend on `iroh_blobs::store::fs::Store` yet — that
//! API is async and tightly coupled to iroh tickets. Phase D will swap this
//! implementation for an iroh-blobs store once we need peer fetching.

use fe_runtime::blob_store::{BlobHash, BlobStore};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// On-disk content-addressed blob store.
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Create (or open) a blob store rooted at `root`.
    ///
    /// The root directory is created if it does not already exist. On Unix
    /// systems we would normally set `0700` permissions — on Windows this is
    /// a no-op for now (see TODO in source).
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&root)
            .map_err(|e| anyhow::anyhow!("create_dir_all {}: {e}", root.display()))?;

        // TODO(phase-d): on Unix, set 0700 on root via std::os::unix::fs::PermissionsExt
        // to restrict access to the current user. Windows ACLs handle this differently
        // — deferred until we ship beyond local-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&root) {
                let mut perms = meta.permissions();
                perms.set_mode(0o700);
                let _ = fs::set_permissions(&root, perms);
            }
        }

        Ok(Self { root })
    }

    /// Default blob store location:
    /// `{dirs::data_local_dir()}/fractalengine/blobs/`.
    pub fn default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fractalengine")
            .join("blobs")
    }

    /// Convenience constructor using `default_path()`.
    pub fn open_default() -> anyhow::Result<Self> {
        Self::new(Self::default_path())
    }

    /// Path layout: `{root}/{first_byte_hex}/{full_hex}`.
    fn path_for(&self, hash: &BlobHash) -> PathBuf {
        let hex = hex::encode(hash);
        let shard = &hex[0..2];
        self.root.join(shard).join(&hex)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl BlobStore for FsBlobStore {
    fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
        let hash: BlobHash = *blake3::hash(bytes).as_bytes();
        let path = self.path_for(&hash);

        if path.exists() {
            // De-duplication: same content hash = same file, nothing to do.
            return Ok(hash);
        }

        let shard_dir = path.parent().expect("path_for always has a parent");
        fs::create_dir_all(shard_dir)
            .map_err(|e| anyhow::anyhow!("create_dir_all {}: {e}", shard_dir.display()))?;

        // Atomic write: write to a temp file in the same directory, then rename.
        // Same-directory rename is atomic on all mainstream filesystems, so
        // readers never observe a partially-written blob.
        let tmp_path = shard_dir.join(format!("{}.tmp", hex::encode(hash)));
        {
            let mut f = fs::File::create(&tmp_path)
                .map_err(|e| anyhow::anyhow!("create {}: {e}", tmp_path.display()))?;
            f.write_all(bytes)
                .map_err(|e| anyhow::anyhow!("write {}: {e}", tmp_path.display()))?;
            f.sync_all()
                .map_err(|e| anyhow::anyhow!("sync {}: {e}", tmp_path.display()))?;
        }
        match fs::rename(&tmp_path, &path) {
            Ok(()) => {}
            Err(_e) if path.exists() => {
                // Another writer beat us — our content is identical (same hash).
                // Clean up temp file and succeed.
                let _ = fs::remove_file(&tmp_path);
                tracing::debug!(hash = %hex::encode(hash), "Blob already written by concurrent writer");
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(anyhow::anyhow!(
                    "rename {} -> {}: {e}",
                    tmp_path.display(),
                    path.display()
                ));
            }
        }

        Ok(hash)
    }

    fn get_blob_path(&self, hash: &BlobHash) -> Option<PathBuf> {
        let path = self.path_for(hash);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    fn has_blob(&self, hash: &BlobHash) -> bool {
        self.path_for(hash).exists()
    }

    fn remove_blob(&self, hash: &BlobHash) -> anyhow::Result<()> {
        let path = self.path_for(hash);
        if !path.exists() {
            return Ok(());
        }
        fs::remove_file(&path).map_err(|e| anyhow::anyhow!("remove {}: {e}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::blob_store::hash_to_hex;

    #[test]
    fn round_trip_add_get_remove() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(tmp.path().join("blobs")).expect("open store");

        let bytes = b"the mycelial network remembers";
        let hash = store.add_blob(bytes).expect("add");

        // BLAKE3 hash matches the reference computation.
        let expected: BlobHash = *blake3::hash(bytes).as_bytes();
        assert_eq!(hash, expected);

        assert!(store.has_blob(&hash));
        let path = store.get_blob_path(&hash).expect("path present");
        assert!(path.exists());

        // Bytes on disk equal the input.
        let on_disk = std::fs::read(&path).expect("read");
        assert_eq!(on_disk, bytes);

        // Path includes the shard + full hex.
        let hex = hash_to_hex(&hash);
        let pstr = path.to_string_lossy();
        assert!(pstr.contains(&hex[0..2]));
        assert!(pstr.contains(&hex));

        store.remove_blob(&hash).expect("remove");
        assert!(!store.has_blob(&hash));
        assert!(store.get_blob_path(&hash).is_none());
    }

    #[test]
    fn add_same_bytes_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(tmp.path().to_path_buf()).expect("open");
        let bytes = b"dedup me";
        let h1 = store.add_blob(bytes).expect("add1");
        let h2 = store.add_blob(bytes).expect("add2");
        assert_eq!(h1, h2);
    }

    #[test]
    fn remove_missing_is_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(tmp.path().to_path_buf()).expect("open");
        let hash: BlobHash = [7u8; 32];
        store.remove_blob(&hash).expect("should not error");
    }

    #[test]
    fn get_blob_path_returns_none_for_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(tmp.path().to_path_buf()).expect("open");
        let hash: BlobHash = [0u8; 32];
        assert!(store.get_blob_path(&hash).is_none());
        assert!(!store.has_blob(&hash));
    }

    #[test]
    fn default_path_under_data_local_dir() {
        let p = FsBlobStore::default_path();
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.contains("fractalengine/blobs"), "got: {s}");
    }

    #[test]
    fn distinct_content_produces_distinct_hashes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(tmp.path().to_path_buf()).expect("open");
        let a = store.add_blob(b"alpha").expect("add a");
        let b = store.add_blob(b"beta").expect("add b");
        assert_ne!(a, b);
        assert!(store.has_blob(&a));
        assert!(store.has_blob(&b));
    }
}
