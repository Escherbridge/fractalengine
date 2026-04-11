//! Content-addressed blob storage abstraction (P2P Mycelium Phase A).
//!
//! The `BlobStore` trait decouples asset-byte storage from the database layer.
//! It lives in `fe-runtime` (no crate dependencies) so that both `fe-database`
//! and `fe-sync` can depend on it without creating a cycle:
//!
//! ```text
//! fe-runtime (trait)  <---  fe-database (uses handle)
//!        ^
//!        |
//!     fe-sync (FsBlobStore impl)  --->  fe-database (existing dep)
//! ```
//!
//! Hashes are raw BLAKE3 digests (`[u8; 32]`). Hex encoding is provided for DB
//! rows (`content_hash` column) and paths (`blob://{hex}.glb`).

use std::path::PathBuf;
use std::sync::Arc;

/// Raw BLAKE3 hash bytes identifying a blob.
pub type BlobHash = [u8; 32];

/// Shared, thread-safe handle to a `BlobStore` implementation.
///
/// The DB thread, the sync thread, and Bevy systems all share the same handle
/// so every byte of every asset flows through one content-addressed layer.
pub type BlobStoreHandle = Arc<dyn BlobStore>;

/// Abstraction over a local content-addressed blob store.
///
/// Implementations must be `Send + Sync` — they are shared between threads via
/// `Arc<dyn BlobStore>`. All methods are synchronous; async implementations
/// should block the current thread (the DB thread already owns a tokio runtime,
/// and the sync thread will manage its own when needed).
pub trait BlobStore: Send + Sync {
    /// Write `bytes` to the store and return its BLAKE3 hash.
    ///
    /// Idempotent: calling with the same bytes twice is a no-op on the second
    /// call (content addressing guarantees de-duplication).
    fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash>;

    /// Return the absolute path to the blob file on disk, if present.
    /// Returns `None` if the blob is not in the store.
    fn get_blob_path(&self, hash: &BlobHash) -> Option<PathBuf>;

    /// Check whether a blob is stored locally.
    fn has_blob(&self, hash: &BlobHash) -> bool;

    /// Remove a blob from the store. No-op if the blob is absent.
    fn remove_blob(&self, hash: &BlobHash) -> anyhow::Result<()>;
}

/// Encode a `BlobHash` as a lowercase hex string suitable for DB rows and URLs.
pub fn hash_to_hex(hash: &BlobHash) -> String {
    hex::encode(hash)
}

/// Decode a hex-encoded BLAKE3 hash back into raw bytes.
pub fn hash_from_hex(s: &str) -> anyhow::Result<BlobHash> {
    let bytes = hex::decode(s).map_err(|e| anyhow::anyhow!("invalid hex hash {s:?}: {e}"))?;
    if bytes.len() != 32 {
        anyhow::bail!("expected 32-byte BLAKE3 hash, got {} bytes", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// In-memory mock `BlobStore` used by tests and the migration fallback path.
///
/// Publicly exposed (not feature-gated) because both `fe-database` and
/// `fe-sync` use it in their integration tests, and the overhead of an unused
/// `HashMap` in the binary is negligible.
pub mod mock {
    use super::{BlobHash, BlobStore};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Purely in-memory `BlobStore`. `get_blob_path` always returns `None`
    /// (there is no on-disk file) — callers that need bytes should use
    /// a dedicated `bytes_for(hash)` accessor on the concrete type.
    #[derive(Default)]
    pub struct MockBlobStore {
        inner: Mutex<HashMap<BlobHash, Vec<u8>>>,
    }

    impl MockBlobStore {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(HashMap::new()),
            }
        }

        /// Test helper: fetch stored bytes by hash.
        pub fn bytes_for(&self, hash: &BlobHash) -> Option<Vec<u8>> {
            self.inner.lock().ok()?.get(hash).cloned()
        }

        /// Test helper: count stored blobs.
        pub fn len(&self) -> usize {
            self.inner.lock().map(|m| m.len()).unwrap_or(0)
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    impl BlobStore for MockBlobStore {
        fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
            let hash: BlobHash = *blake3_lite(bytes).as_bytes();
            self.inner
                .lock()
                .map_err(|e| anyhow::anyhow!("MockBlobStore poisoned: {e}"))?
                .insert(hash, bytes.to_vec());
            Ok(hash)
        }

        fn get_blob_path(&self, _hash: &BlobHash) -> Option<PathBuf> {
            // Mock is memory-only — there is no real path.
            None
        }

        fn has_blob(&self, hash: &BlobHash) -> bool {
            self.inner
                .lock()
                .map(|m| m.contains_key(hash))
                .unwrap_or(false)
        }

        fn remove_blob(&self, hash: &BlobHash) -> anyhow::Result<()> {
            self.inner
                .lock()
                .map_err(|e| anyhow::anyhow!("MockBlobStore poisoned: {e}"))?
                .remove(hash);
            Ok(())
        }
    }

    /// Minimal BLAKE3 wrapper so the mock does not pull in the full `blake3`
    /// crate as a direct dependency of `fe-runtime`. We duplicate the
    /// computation via `std::hash`-style compression — but for real hashing
    /// we need BLAKE3, so route through a tiny local helper.
    ///
    /// NOTE: We cannot avoid BLAKE3 here because the mock must produce the
    /// same hash as `FsBlobStore` so tests can cross-check. `fe-runtime`
    /// therefore takes a `blake3` dep. If this dep weight becomes an issue,
    /// move the mock to `fe-sync` instead.
    fn blake3_lite(bytes: &[u8]) -> blake3::Hash {
        blake3::hash(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockBlobStore;
    use super::*;

    #[test]
    fn hash_hex_roundtrip() {
        let hash: BlobHash = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
            0xcc, 0xdd, 0xee, 0xff,
        ];
        let hex = hash_to_hex(&hash);
        assert_eq!(hex.len(), 64);
        let back = hash_from_hex(&hex).expect("roundtrip");
        assert_eq!(hash, back);
    }

    #[test]
    fn hash_from_hex_rejects_wrong_length() {
        assert!(hash_from_hex("deadbeef").is_err());
        assert!(hash_from_hex("").is_err());
    }

    #[test]
    fn hash_from_hex_rejects_non_hex() {
        assert!(hash_from_hex("zzzz").is_err());
    }

    #[test]
    fn mock_blob_store_round_trip() {
        let store = MockBlobStore::new();
        let bytes = b"hello mycelium";
        let hash = store.add_blob(bytes).expect("add");

        assert!(store.has_blob(&hash));
        assert!(
            store.get_blob_path(&hash).is_none(),
            "mock has no on-disk path"
        );
        assert_eq!(store.bytes_for(&hash).as_deref(), Some(&bytes[..]));

        // Idempotency: same bytes -> same hash, no duplication.
        let hash2 = store.add_blob(bytes).expect("add again");
        assert_eq!(hash, hash2);
        assert_eq!(store.len(), 1);

        store.remove_blob(&hash).expect("remove");
        assert!(!store.has_blob(&hash));
        assert!(store.is_empty());
    }

    #[test]
    fn mock_blob_store_hash_matches_blake3() {
        let store = MockBlobStore::new();
        let bytes = b"deterministic";
        let hash = store.add_blob(bytes).expect("add");
        let expected: BlobHash = *blake3::hash(bytes).as_bytes();
        assert_eq!(hash, expected);
    }

    #[test]
    fn mock_blob_store_remove_missing_is_noop() {
        let store = MockBlobStore::new();
        let hash: BlobHash = [0u8; 32];
        store.remove_blob(&hash).expect("no error on missing");
    }
}
