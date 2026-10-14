//! One-time legacy `imported/` → blob-store backfill — see `src/AGENTS.md`
//! §asset-resolution (why startup, why a full asset scan, hash replacement).

use std::path::{Path, PathBuf};

use crate::repo::Db;
use crate::{hash_from_hex, hash_to_hex, BlobStoreHandle};

/// What one backfill pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedBackfillReport {
    /// Rows re-pointed at a blob ingested from their legacy `imported/` file.
    pub backfilled: u64,
    /// Rows with no usable blob and no legacy file to recover one from.
    pub missing: u64,
    /// Rows whose legacy file could not be read, stored, or recorded.
    pub failed: u64,
}

/// Ingest `dir/<asset_id>.{glb,gltf}` for every asset row whose `content_hash`
/// is absent/invalid or whose blob is gone, then record the hash on the row.
/// Idempotent; never deletes legacy files; returns at once when `dir` holds no model file.
pub async fn backfill_imported_assets(
    db: &Db,
    blob_store: &BlobStoreHandle,
    dir: &Path,
) -> anyhow::Result<ImportedBackfillReport> {
    let mut report = ImportedBackfillReport::default();
    if !holds_model_file(dir) {
        return Ok(report);
    }

    let mut res = db
        .query("SELECT asset_id, content_hash FROM asset")
        .await?
        .check()?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    for row in &rows {
        let Some(asset_id) = row["asset_id"].as_str() else {
            continue;
        };
        let recorded = row["content_hash"].as_str();
        let blob_present = recorded
            .and_then(|hex| hash_from_hex(hex).ok())
            .is_some_and(|hash| blob_store.has_blob(&hash));
        if blob_present {
            continue;
        }
        // AC-7 (critic tick 7, AC-35g): `legacy_file` joins `asset_id` into a
        // filesystem path with no charset guard of its own. That join was
        // safe only because inbound `asset` rows fell to `NotApplicable`
        // (replicated_row.rs) — nothing peer-supplied could reach it. A3
        // makes anchored `asset` rows replicate insert-if-absent, so a
        // peer-chosen `asset_id` can now reach this startup join on any
        // machine that still has legacy `imported/` files. Reuse the ONE
        // shared path-id guard (`fe_runtime::safe_id::is_safe_path_id`) — NOT
        // a third hand-rolled one beside P3P4 S1's hexon-id guard and this
        // one ("shared cores + separate guards = drift", RUNBOOK §5) — and
        // refuse BEFORE `legacy_file` ever builds a path from the id.
        if !fe_runtime::safe_id::is_safe_path_id(asset_id) {
            tracing::warn!(
                asset_id,
                "legacy asset backfill: refusing a path-escaping or malformed asset_id"
            );
            report.failed += 1;
            continue;
        }
        let Some(path) = legacy_file(dir, asset_id) else {
            report.missing += 1;
            continue;
        };
        match ingest(db, blob_store, asset_id, recorded, &path).await {
            Ok(()) => report.backfilled += 1,
            Err(e) => {
                tracing::error!("legacy asset backfill failed for {}: {e}", path.display());
                report.failed += 1;
            }
        }
    }

    if report == ImportedBackfillReport::default() {
        tracing::debug!("legacy imported/ asset backfill: nothing to do");
    } else {
        tracing::info!(
            backfilled = report.backfilled,
            missing = report.missing,
            failed = report.failed,
            dir = %dir.display(),
            "legacy imported/ asset backfill"
        );
    }
    Ok(report)
}

/// Store one legacy file's bytes and point its asset row at the resulting blob.
async fn ingest(
    db: &Db,
    blob_store: &BlobStoreHandle,
    asset_id: &str,
    recorded: Option<&str>,
    path: &Path,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let content_hash = hash_to_hex(&blob_store.add_blob(&bytes)?);
    if let Some(old) = recorded.filter(|old| !old.eq_ignore_ascii_case(&content_hash)) {
        tracing::warn!(
            "asset {asset_id}: legacy file hash {content_hash} replaces recorded content_hash {old} (its blob is absent)"
        );
    }
    // size_bytes always describes the blob the row now points at.
    let size_bytes = i64::try_from(bytes.len())?;
    db.query("UPDATE asset SET content_hash = $hash, size_bytes = $size WHERE asset_id = $aid")
        .bind(("hash", content_hash))
        .bind(("size", size_bytes))
        .bind(("aid", asset_id.to_string()))
        .await?
        .check()?;
    Ok(())
}

/// `dir/<asset_id>.glb`, else `.gltf` — the old resolver's probe order.
fn legacy_file(dir: &Path, asset_id: &str) -> Option<PathBuf> {
    ["glb", "gltf"]
        .into_iter()
        .map(|ext| dir.join(format!("{asset_id}.{ext}")))
        .find(|path| path.is_file())
}

/// Whether `dir` exists and contains at least one `.glb` / `.gltf` file.
fn holds_model_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("glb") || ext.eq_ignore_ascii_case("gltf")
                })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::Table;
    use crate::schema::{Asset, Model};
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    /// In-memory DB: the real (SCHEMAFULL) `asset` DDL — the table the backfill
    /// writes — and `model` (read by the hierarchy load), plus a schemaless
    /// verse→fractal→petal chain for the hierarchy read.
    async fn setup() -> (Db, BlobStoreHandle) {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        db.query(format!("{}{}", Asset::schema(), Model::schema()))
            .await
            .expect("define asset + model")
            .check()
            .expect("define asset + model check");
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v' };
             CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: 'verse-1', name: 'f' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'p' };",
        )
        .await
        .expect("seed scope chain")
        .check()
        .expect("seed scope chain check");
        let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        (db, store)
    }

    /// A pre-blob-store asset row: no `content_hash` unless `recorded_hash` is given.
    async fn legacy_asset(db: &Db, asset_id: &str, size_bytes: i64, recorded_hash: Option<&str>) {
        let hash_field = if recorded_hash.is_some() {
            "content_hash: $hash,"
        } else {
            ""
        };
        let mut query = db
            .query(format!(
                "CREATE asset CONTENT {{ asset_id: $aid, name: 'legacy.glb', \
                 content_type: 'model/gltf-binary', size_bytes: $size, {hash_field} \
                 created_at: '2026-04-05T00:00:00Z' }}"
            ))
            .bind(("aid", asset_id.to_string()))
            .bind(("size", size_bytes));
        if let Some(hash) = recorded_hash {
            query = query.bind(("hash", hash.to_string()));
        }
        query.await.unwrap().check().unwrap();
    }

    async fn asset_row(db: &Db, asset_id: &str) -> serde_json::Value {
        let mut res = db
            .query("SELECT content_hash, size_bytes FROM asset WHERE asset_id = $aid")
            .bind(("aid", asset_id.to_string()))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        rows.into_iter().next().expect("asset row")
    }

    /// Legacy GLB on disk + hashless row → after startup backfill the row names
    /// the blob, a placed node resolves to `blob://`, the file is kept, and a
    /// second pass changes nothing. Also: a row whose recorded blob vanished is
    /// restored from its file, and a row with neither blob nor file is `missing`.
    #[tokio::test]
    async fn backfill_moves_legacy_imports_into_the_blob_store() {
        let (db, store) = setup().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let hashless_bytes = b"glTF legacy import A";
        let vanished_bytes = b"glTF legacy import B";
        std::fs::write(dir.path().join("asset-hashless.glb"), hashless_bytes).unwrap();
        std::fs::write(dir.path().join("asset-vanished.gltf"), vanished_bytes).unwrap();
        let vanished_hash = hash_to_hex(blake3::hash(vanished_bytes).as_bytes());

        legacy_asset(&db, "asset-hashless", 0, None).await;
        legacy_asset(&db, "asset-vanished", 99, Some(&vanished_hash)).await;
        legacy_asset(&db, "asset-unrecoverable", 7, None).await;
        db.query(
            "CREATE node CONTENT { node_id: 'node-1', petal_id: 'petal-1', \
             display_name: 'duck', asset_id: 'asset-hashless', \
             position: <geometry<point>> [0.0, 0.0], elevation: 0.0, \
             rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
             created_at: '2026-04-05T00:00:00Z' }",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let report = backfill_imported_assets(&db, &store, dir.path())
            .await
            .expect("backfill");

        assert_eq!(
            report,
            ImportedBackfillReport {
                backfilled: 2,
                missing: 1,
                failed: 0
            }
        );
        let hashless_hash = hash_to_hex(blake3::hash(hashless_bytes).as_bytes());
        let row = asset_row(&db, "asset-hashless").await;
        assert_eq!(row["content_hash"], hashless_hash.as_str());
        assert_eq!(
            row["size_bytes"],
            hashless_bytes.len() as i64,
            "size follows the ingested blob"
        );
        assert!(store.has_blob(&hash_from_hex(&hashless_hash).unwrap()));
        let row = asset_row(&db, "asset-vanished").await;
        assert_eq!(row["content_hash"], vanished_hash.as_str());
        assert_eq!(
            row["size_bytes"],
            vanished_bytes.len() as i64,
            "size follows the ingested blob"
        );
        assert!(store.has_blob(&hash_from_hex(&vanished_hash).unwrap()));
        assert!(
            dir.path().join("asset-hashless.glb").is_file(),
            "legacy files are never deleted"
        );

        let verses = crate::handlers::crud::load_hierarchy_handler(&db, &store)
            .await
            .expect("load hierarchy");
        let node = &verses[0].fractals[0].petals[0].nodes[0];
        assert_eq!(
            node.asset_path.as_deref(),
            Some(format!("blob://{hashless_hash}.glb").as_str())
        );

        let again = backfill_imported_assets(&db, &store, dir.path())
            .await
            .expect("second backfill");
        assert_eq!(
            again,
            ImportedBackfillReport {
                backfilled: 0,
                missing: 1,
                failed: 0
            },
            "idempotent: already-backfilled rows are skipped"
        );
    }

    /// No legacy directory → no query, no writes: a hashless row stays hashless.
    #[tokio::test]
    async fn backfill_is_a_no_op_without_a_legacy_dir() {
        let (db, store) = setup().await;
        legacy_asset(&db, "asset-hashless", 0, None).await;
        let dir = tempfile::tempdir().expect("tempdir");

        let report = backfill_imported_assets(&db, &store, &dir.path().join("absent"))
            .await
            .expect("backfill");

        assert_eq!(report, ImportedBackfillReport::default());
        assert!(asset_row(&db, "asset-hashless").await["content_hash"].is_null());
    }

    /// AC-7 (critic tick 7, AC-35g): a legacy row whose `asset_id` is
    /// path-escaping or outside the safe charset is refused by the guard
    /// BEFORE `legacy_file`/the write path ever builds a path from it — a
    /// typed `failed` count, never `missing`, and no path outside the legacy
    /// dir is touched. Mutation check: deleting the guard would let the
    /// escaping id reach the join and this test would see `failed: 0`.
    #[tokio::test]
    async fn backfill_ignores_path_escaping_asset_ids() {
        let (db, store) = setup().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().parent().unwrap().join("escaped-sentinel.glb");
        for bad_id in ["../../etc/passwd", "../escaped-sentinel", "a/b", "a\\b"] {
            legacy_asset(&db, bad_id, 0, None).await;
        }
        // `holds_model_file` gates the whole pass on at least one real legacy
        // file existing; seed one well-formed row too so the directory scan
        // proceeds and every bad id is actually evaluated by the per-row guard.
        std::fs::write(dir.path().join("asset-hashless.glb"), b"ok").unwrap();
        legacy_asset(&db, "asset-hashless", 0, None).await;

        let report = backfill_imported_assets(&db, &store, dir.path())
            .await
            .expect("backfill");

        assert_eq!(
            report.failed, 4,
            "every path-escaping/malformed id is a typed failure"
        );
        assert_eq!(
            report.backfilled, 1,
            "the one well-formed id still backfills"
        );
        // If the guard were absent, `dir.join("../../etc/passwd.glb")` would
        // resolve outside `dir` — prove no escape by asserting this sentinel
        // path (one level up) was never created.
        assert!(
            !sentinel.exists(),
            "no path outside the legacy dir was ever touched"
        );
    }

    /// A re-point test (W0 reviewer note: this branch had no test): a row's
    /// RECORDED `content_hash` is some other value, but its legacy file's
    /// ACTUAL bytes hash to something else — the backfill trusts the file,
    /// never the stale recorded value, and re-points the row.
    #[tokio::test]
    async fn backfill_repoints_a_row_whose_recorded_hash_disagrees_with_its_file() {
        let (db, store) = setup().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let real_bytes = b"the actual legacy file bytes";
        std::fs::write(dir.path().join("asset-stale.glb"), real_bytes).unwrap();
        // A deliberately WRONG recorded hash — some unrelated 32-byte digest.
        let wrong_hash = hash_to_hex(blake3::hash(b"not the real bytes").as_bytes());
        legacy_asset(&db, "asset-stale", 0, Some(&wrong_hash)).await;

        let report = backfill_imported_assets(&db, &store, dir.path())
            .await
            .expect("backfill");

        assert_eq!(report.backfilled, 1);
        let real_hash = hash_to_hex(blake3::hash(real_bytes).as_bytes());
        let row = asset_row(&db, "asset-stale").await;
        assert_eq!(
            row["content_hash"],
            real_hash.as_str(),
            "re-pointed to the file's real hash, not the stale recorded one"
        );
        assert!(store.has_blob(&hash_from_hex(&real_hash).unwrap()));
    }
}
