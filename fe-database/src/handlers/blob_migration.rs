//! One-time, copy-only migration of blobs from the legacy per-user store
//! into the per-install store co-located with this DB — see src/AGENTS.md
//! §handlers (blob_migration) and fe-runtime/src/AGENTS.md §packaged-layout
//! (OF-1/S1b).

use std::collections::HashSet;
use std::path::Path;

use fe_runtime::blob_store::canonical_blob_path;

use super::asset_gc::strings;
use crate::repo::Db;
use crate::{hash_from_hex, BlobStore};

/// Marker file name gating a one-time migration pass, same atomic
/// tmp+rename discipline `FsBlobStore::add_blob` uses for a blob write.
const MIGRATED_MARKER_NAME: &str = ".migrated-from-legacy";

/// What one migration pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub copied: u64,
    /// `true` when the marker was already present and nothing ran.
    pub already_migrated: bool,
}

/// Every BLAKE3 hex hash a live row in `db` still names: `asset.content_hash`,
/// `crate_entry.asset_hash`, and a node's `properties.path_asset.asset_path`
/// `blob://{hash}.{ext}` stamp — the same three queries
/// `asset_gc::gc_unreferenced_assets` runs (there, restricted to its doomed
/// subset via `$hashes`), run here over every row so migration knows the
/// full referenced set.
pub async fn referenced_asset_hashes(db: &Db) -> anyhow::Result<HashSet<String>> {
    let mut out: HashSet<String> = strings(db, "SELECT VALUE content_hash FROM asset", None)
        .await?
        .into_iter()
        .collect();
    out.extend(strings(db, "SELECT VALUE asset_hash FROM crate_entry", None).await?);
    // Path stamps name their GLB by `blob://{hash}.{ext}` (fe-sdk `path_asset`), not asset_id.
    out.extend(
        strings(
            db,
            "SELECT VALUE properties.path_asset.asset_path FROM node \
             WHERE properties.path_asset.asset_path != NONE",
            None,
        )
        .await?
        .iter()
        .filter_map(|uri| uri.strip_prefix("blob://")?.split('.').next())
        .map(str::to_ascii_lowercase),
    );
    Ok(out)
}

/// Copy every hash `db` still names from `legacy_root` into `new_store`
/// (rooted at `new_store_root`), when missing there. Never writes to, or
/// removes from, `legacy_root` — read-only to this function for its entire
/// lifetime. No-op if `legacy_root` does not exist, or if
/// `{new_store_root}/.migrated-from-legacy` is already present.
///
/// Idempotent and crash-safe: the marker is written only after every
/// referenced hash has been attempted with no hard I/O error (a read or
/// `add_blob` failure propagates via `?` before the marker is touched), and
/// `add_blob` is itself a no-op on content already present — so a retried
/// copy after a crash mid-pass is harmless.
pub async fn migrate_legacy_blob_store(
    db: &Db,
    new_store: &dyn BlobStore,
    new_store_root: &Path,
    legacy_root: &Path,
) -> anyhow::Result<MigrationReport> {
    if same_directory(new_store_root, legacy_root) {
        // FE_BLOB_DIR points at the legacy store itself: nothing to copy, and
        // the legacy directory must not even receive the marker (S8).
        return Ok(MigrationReport::default());
    }
    let marker = new_store_root.join(MIGRATED_MARKER_NAME);
    if marker.exists() {
        return Ok(MigrationReport {
            copied: 0,
            already_migrated: true,
        });
    }

    let mut copied = 0u64;
    if legacy_root.exists() {
        for hex in referenced_asset_hashes(db).await? {
            let Ok(hash) = hash_from_hex(&hex) else {
                continue;
            };
            if new_store.has_blob(&hash) {
                continue;
            }
            let legacy_path = canonical_blob_path(legacy_root, &hash);
            if !legacy_path.exists() {
                continue;
            }
            let bytes = std::fs::read(&legacy_path)
                .map_err(|e| anyhow::anyhow!("read legacy blob {}: {e}", legacy_path.display()))?;
            if *blake3::hash(&bytes).as_bytes() != hash {
                // A corrupt legacy file is never carried forward under its name.
                tracing::warn!(path = %legacy_path.display(), "Skipping a legacy blob whose bytes do not match its hash");
                continue;
            }
            new_store.add_blob(&bytes)?;
            copied += 1;
        }
    }

    // Marker write is the last step and is itself atomic (tmp + rename), so a
    // crash can only ever leave the marker absent (safe retry), never
    // half-written.
    std::fs::create_dir_all(new_store_root)
        .map_err(|e| anyhow::anyhow!("create {}: {e}", new_store_root.display()))?;
    let tmp = new_store_root.join(format!("{MIGRATED_MARKER_NAME}.tmp"));
    std::fs::write(&tmp, b"1").map_err(|e| anyhow::anyhow!("write {}: {e}", tmp.display()))?;
    match std::fs::rename(&tmp, &marker) {
        Ok(()) => {}
        Err(_) if marker.exists() => {
            let _ = std::fs::remove_file(&tmp);
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow::anyhow!(
                "rename {} -> {}: {e}",
                tmp.display(),
                marker.display()
            ));
        }
    }

    Ok(MigrationReport {
        copied,
        already_migrated: false,
    })
}

/// Whether `a` and `b` name one directory (resolved when both exist).
fn same_directory(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlobStoreHandle;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    async fn setup() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db
    }

    async fn asset_naming(db: &Db, hash_hex: &str) {
        db.query(
            "CREATE asset CONTENT { asset_id: $aid, name: 'm.glb', \
             content_type: 'model/gltf-binary', size_bytes: 1, content_hash: $hash, \
             owner_did: 'did:key:z6MkOwner', petal_id: 'petal-1', created_at: $at }",
        )
        .bind(("aid", ulid::Ulid::new().to_string()))
        .bind(("hash", hash_hex.to_string()))
        .bind(("at", "2020-01-01T00:00:00Z"))
        .await
        .unwrap()
        .check()
        .unwrap();
    }

    /// Writes `bytes` under its canonical BLAKE3 path inside a plain
    /// directory, mirroring the legacy `FsBlobStore` on-disk layout without
    /// depending on fe-sync (fe-database never depends on fe-sync).
    fn seed_legacy_dir(root: &Path, bytes: &[u8]) -> String {
        let hash: [u8; 32] = *blake3::hash(bytes).as_bytes();
        let path = canonical_blob_path(root, &hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        crate::hash_to_hex(&hash)
    }

    #[tokio::test]
    async fn migration_copies_referenced_blobs_once() {
        let legacy = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let hash_hex = seed_legacy_dir(legacy.path(), b"shared glb bytes");

        let db = setup().await;
        asset_naming(&db, &hash_hex).await;
        let store: BlobStoreHandle = Arc::new(MockBlobStore::default());

        let first = migrate_legacy_blob_store(&db, store.as_ref(), new_root.path(), legacy.path())
            .await
            .unwrap();
        assert_eq!(
            first,
            MigrationReport {
                copied: 1,
                already_migrated: false
            }
        );
        let hash = hash_from_hex(&hash_hex).unwrap();
        assert!(store.has_blob(&hash));

        // Second call: marker already present, no-op, legacy source untouched.
        let second = migrate_legacy_blob_store(&db, store.as_ref(), new_root.path(), legacy.path())
            .await
            .unwrap();
        assert_eq!(
            second,
            MigrationReport {
                copied: 0,
                already_migrated: true
            }
        );
        let legacy_path = canonical_blob_path(legacy.path(), &hash);
        assert_eq!(
            std::fs::read(&legacy_path).unwrap(),
            b"shared glb bytes",
            "legacy source is byte-identical and still present after both calls"
        );
    }

    /// Two installs that shared the old per-user directory each migrate their
    /// own referenced blob into their own store; a GC pass in one never
    /// touches the other's copy, because the stores are now separate files.
    #[tokio::test]
    async fn two_installs_sharing_the_old_store_keep_independent_copies_after_migration() {
        let legacy = tempfile::tempdir().unwrap();
        let hash_hex = seed_legacy_dir(legacy.path(), b"blob shared by two installs");
        let hash = hash_from_hex(&hash_hex).unwrap();

        let db_a = setup().await;
        asset_naming(&db_a, &hash_hex).await;
        let db_b = setup().await;
        asset_naming(&db_b, &hash_hex).await;

        let store_a: BlobStoreHandle = Arc::new(MockBlobStore::default());
        let store_b: BlobStoreHandle = Arc::new(MockBlobStore::default());
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();

        migrate_legacy_blob_store(&db_a, store_a.as_ref(), root_a.path(), legacy.path())
            .await
            .unwrap();
        migrate_legacy_blob_store(&db_b, store_b.as_ref(), root_b.path(), legacy.path())
            .await
            .unwrap();
        assert!(store_a.has_blob(&hash) && store_b.has_blob(&hash));

        // A's own GC collects its expired, unreferenced asset row (seeded by
        // `asset_naming` with an old `created_at`) and removes its own copy
        // of the blob; B's independently-migrated copy must survive.
        let report = super::super::asset_gc::gc_unreferenced_assets(&db_a, &store_a, 0)
            .await
            .unwrap();
        assert_eq!(report.removed_blobs, 1);

        assert!(
            !store_a.has_blob(&hash),
            "A's own store no longer has the blob"
        );
        assert!(
            store_b.has_blob(&hash),
            "B's own store, migrated separately, still has it"
        );
    }

    #[tokio::test]
    async fn no_legacy_directory_is_a_clean_noop_and_still_marks_migrated() {
        let db = setup().await;
        let store: BlobStoreHandle = Arc::new(MockBlobStore::default());
        let new_root = tempfile::tempdir().unwrap();
        let missing_legacy = new_root.path().join("never-created");

        let report =
            migrate_legacy_blob_store(&db, store.as_ref(), new_root.path(), &missing_legacy)
                .await
                .unwrap();
        assert_eq!(
            report,
            MigrationReport {
                copied: 0,
                already_migrated: false
            }
        );
        assert!(new_root.path().join(MIGRATED_MARKER_NAME).exists());
    }

    /// S8: a crash mid-migration (marker absent, one blob already copied, a
    /// stale marker temp file on disk) is retried safely: only the missing
    /// blob is copied, the marker lands, and the legacy files are untouched.
    #[tokio::test]
    async fn a_crash_mid_migration_is_retried_without_loss_or_legacy_writes() {
        let legacy = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let first_hex = seed_legacy_dir(legacy.path(), b"copied before the crash");
        let second_hex = seed_legacy_dir(legacy.path(), b"never reached before the crash");
        let db = setup().await;
        asset_naming(&db, &first_hex).await;
        asset_naming(&db, &second_hex).await;

        let store: BlobStoreHandle = Arc::new(MockBlobStore::default());
        store.add_blob(b"copied before the crash").unwrap();
        std::fs::write(
            new_root.path().join(format!("{MIGRATED_MARKER_NAME}.tmp")),
            b"half",
        )
        .unwrap();
        let legacy_before = legacy_snapshot(legacy.path());

        let report = migrate_legacy_blob_store(&db, store.as_ref(), new_root.path(), legacy.path())
            .await
            .unwrap();
        assert_eq!(
            report,
            MigrationReport {
                copied: 1,
                already_migrated: false
            }
        );
        for hex in [&first_hex, &second_hex] {
            assert!(store.has_blob(&hash_from_hex(hex).unwrap()));
        }
        assert!(new_root.path().join(MIGRATED_MARKER_NAME).exists());
        assert_eq!(legacy_snapshot(legacy.path()), legacy_before);
    }

    /// S8: a legacy file whose bytes no longer hash to its name is not
    /// carried forward, and the legacy file itself is left as it was.
    #[tokio::test]
    async fn a_corrupt_legacy_blob_is_not_copied() {
        let legacy = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let hash_hex = seed_legacy_dir(legacy.path(), b"original bytes");
        let hash = hash_from_hex(&hash_hex).unwrap();
        std::fs::write(
            canonical_blob_path(legacy.path(), &hash),
            b"bit-rotted bytes",
        )
        .unwrap();
        let db = setup().await;
        asset_naming(&db, &hash_hex).await;
        let store: BlobStoreHandle = Arc::new(MockBlobStore::default());

        let report = migrate_legacy_blob_store(&db, store.as_ref(), new_root.path(), legacy.path())
            .await
            .unwrap();
        assert_eq!(report.copied, 0);
        assert!(!store.has_blob(&hash));
        assert!(!store.has_blob(blake3::hash(b"bit-rotted bytes").as_bytes()));
        assert_eq!(
            std::fs::read(canonical_blob_path(legacy.path(), &hash)).unwrap(),
            b"bit-rotted bytes"
        );
    }

    /// S8: when the new store IS the legacy directory (`FE_BLOB_DIR` pointed
    /// at it), migration writes nothing there, not even its marker.
    #[tokio::test]
    async fn a_new_store_at_the_legacy_path_writes_nothing_into_it() {
        let legacy = tempfile::tempdir().unwrap();
        let hash_hex = seed_legacy_dir(legacy.path(), b"lives in the legacy dir");
        let db = setup().await;
        asset_naming(&db, &hash_hex).await;
        let store: BlobStoreHandle = Arc::new(MockBlobStore::default());
        let before = legacy_snapshot(legacy.path());

        let report = migrate_legacy_blob_store(&db, store.as_ref(), legacy.path(), legacy.path())
            .await
            .unwrap();
        assert_eq!(report, MigrationReport::default());
        assert!(!legacy.path().join(MIGRATED_MARKER_NAME).exists());
        assert_eq!(legacy_snapshot(legacy.path()), before);
    }

    /// Every file under `root` with its bytes (recursive, sorted).
    fn legacy_snapshot(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    out.push((path, bytes));
                }
            }
        }
        out.sort();
        out
    }
}
