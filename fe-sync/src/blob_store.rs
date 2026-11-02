//! Filesystem-backed `BlobStore` implementation (P2P Mycelium Phase A).
//!
//! Blobs are stored as `{root}/{hex[0..2]}/{hex}`, sharded by the first byte of
//! their BLAKE3 hash to avoid dumping thousands of files into one directory.
//!
//! We deliberately do NOT depend on `iroh_blobs::store::fs::Store` yet — that
//! API is async and tightly coupled to iroh tickets. Phase D will swap this
//! implementation for an iroh-blobs store once we need peer fetching.

use fe_runtime::blob_store::{canonical_blob_path, BlobHash, BlobStore, DEFAULT_BLOB_DIR_NAME};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Overrides [`FsBlobStore::default_path`]'s `data/blobs` default, same
/// precedence pattern as the relay's `FE_DB_PATH`/`FE_P2P_DIR` (OF-1/S1b).
const FE_BLOB_DIR_ENV: &str = "FE_BLOB_DIR";

fn finish_create_dir(path: &Path, result: io::Result<()>) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(_) if path.is_dir() => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "create_dir_all {}: {error}",
            path.display()
        )),
    }
}

fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    finish_create_dir(path, fs::create_dir_all(path))
}

/// On-disk content-addressed blob store.
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Create or open a blob store; see AGENTS.md section blob-store-root-permissions.
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        ensure_dir(&root)?;

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

    /// Default blob store location: co-located with this install's own DB,
    /// `data/blobs` (CWD-relative — the GUI binary already
    /// `set_current_dir(packaged_root)`s before this runs), overridable by
    /// `FE_BLOB_DIR` (same override-env-first precedence as the relay's
    /// `FE_DB_PATH`/`FE_P2P_DIR`). OF-1/S1b: the pre-2026-10-14 default was
    /// per-USER while the DB is per-INSTALL, so one machine's installs shared
    /// one blob store and install A's asset GC could delete a blob install
    /// B's DB still referenced — see `legacy_default_path` and
    /// `fe-database::handlers::blob_migration` (fe-sync/src/AGENTS.md
    /// §blob-store-directory-initialization).
    pub fn default_path() -> PathBuf {
        if let Ok(dir) = std::env::var(FE_BLOB_DIR_ENV) {
            return PathBuf::from(dir);
        }
        PathBuf::from("data").join(DEFAULT_BLOB_DIR_NAME)
    }

    /// Pre-2026-10-14 per-user default (`{data_local_dir}/fractalengine/blobs`).
    /// Kept only as the one-time migration source for
    /// `fe-database::handlers::blob_migration::migrate_legacy_blob_store` —
    /// never re-inlined into `default_path` (OF-1/S1b).
    pub fn legacy_default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fractalengine")
            .join(DEFAULT_BLOB_DIR_NAME)
    }

    /// Convenience constructor using `default_path()`.
    pub fn open_default() -> anyhow::Result<Self> {
        Self::new(Self::default_path())
    }

    /// Path layout: the shared canonical `{root}/{hex[0..2]}/{hex}`.
    fn path_for(&self, hash: &BlobHash) -> PathBuf {
        canonical_blob_path(&self.root, hash)
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
        ensure_dir(shard_dir)?;

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

    /// `FE_BLOB_DIR` is process-global; serialize the tests that touch it so
    /// `default_path` reads a stable value (same pattern as
    /// `fe-hexon/src/p2p/config.rs`).
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn create_dir_error_accepts_an_existing_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("blobs");
        std::fs::create_dir(&root).expect("create root");

        finish_create_dir(
            &root,
            Err(io::Error::other("simulated Windows create_dir_all result")),
        )
        .expect("existing directory is usable");
    }

    #[test]
    fn already_exists_error_rejects_an_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("blobs");
        std::fs::write(&root, b"not a directory").expect("create file");

        assert!(finish_create_dir(
            &root,
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "existing file"
            )),
        )
        .is_err());
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

    /// OF-1/S1b: packaged layout puts blobs under `data/`, never
    /// `dirs::data_local_dir()`, when no override is set.
    #[test]
    fn default_path_under_data_dir_with_no_override() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(FE_BLOB_DIR_ENV);
        assert_eq!(FsBlobStore::default_path(), PathBuf::from("data/blobs"));
    }

    #[test]
    fn default_path_honors_fe_blob_dir_override() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(FE_BLOB_DIR_ENV, "/custom/blobs");
        let got = FsBlobStore::default_path();
        std::env::remove_var(FE_BLOB_DIR_ENV);
        assert_eq!(got, PathBuf::from("/custom/blobs"));
    }

    /// The pre-2026-10-14 expression stays exactly as it was, under its new
    /// name — migration's only remaining caller.
    #[test]
    fn legacy_default_path_under_data_local_dir() {
        let p = FsBlobStore::legacy_default_path();
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.contains("fractalengine/blobs"), "got: {s}");
    }

    /// Task 2.5: dirs::data_local_dir() returns Some on all desktop platforms.
    #[test]
    fn data_local_dir_exists() {
        assert!(
            dirs::data_local_dir().is_some(),
            "dirs::data_local_dir() returned None on this platform"
        );
    }

    /// Task 2.2: On Unix, blob store root gets 0o700 permissions.
    #[cfg(unix)]
    #[test]
    fn blob_store_sets_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path().join("restricted")).expect("open store");
        let perms = std::fs::metadata(store.root())
            .expect("metadata")
            .permissions();
        assert_eq!(
            perms.mode() & 0o777,
            0o700,
            "blob store root should have 0700 permissions"
        );
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
