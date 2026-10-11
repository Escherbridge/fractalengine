//! Unreferenced-asset garbage collection — see `src/AGENTS.md` §handlers (asset_gc)
//! and fe-api/AGENTS.md §asset-ingest (grace, blob-dir-never-scanned invariant).

use std::collections::HashSet;

use crate::repo::Db;
use crate::{hash_from_hex, BlobStoreHandle};

/// What one GC pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetGcReport {
    pub removed_assets: u64,
    pub removed_blobs: u64,
}

/// Delete `asset` rows that no `node` row references (tombstoned nodes count)
/// and whose `created_at` is older than `grace_secs`; then remove each of their
/// blobs that no remaining `asset` / `crate_entry` row or node `path_asset`
/// stamp descriptor names. Runs on the
/// single-writer DB thread, so the reference check and the delete cannot race
/// a placement. Only hashes of deleted rows are ever considered — the blob
/// directory is never scanned (replication and hexon blobs share the store).
pub async fn gc_unreferenced_assets(
    db: &Db,
    blob_store: &BlobStoreHandle,
    grace_secs: u64,
) -> anyhow::Result<AssetGcReport> {
    let grace = chrono::TimeDelta::try_seconds(i64::try_from(grace_secs).unwrap_or(i64::MAX))
        .unwrap_or(chrono::TimeDelta::MAX);
    let Some(cutoff) = chrono::Utc::now().checked_sub_signed(grace) else {
        return Ok(AssetGcReport::default());
    };

    let referenced: HashSet<String> = strings(
        db,
        "SELECT VALUE asset_id FROM node WHERE asset_id != NONE",
        None,
    )
    .await?
    .into_iter()
    .collect();
    let mut res = db
        .query("SELECT asset_id, content_hash, created_at FROM asset")
        .await?
        .check()?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let text = |row: &serde_json::Value, key: &str| row.get(key)?.as_str().map(str::to_string);
    let mut doomed_ids = Vec::new();
    let mut doomed_hashes = HashSet::new();
    for row in &rows {
        let Some(asset_id) = text(row, "asset_id") else {
            continue;
        };
        // An unparseable `created_at` is kept (conservative).
        let expired = text(row, "created_at")
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
            .is_some_and(|at| at < cutoff);
        if expired && !referenced.contains(&asset_id) {
            doomed_hashes.extend(text(row, "content_hash"));
            doomed_ids.push(asset_id);
        }
    }
    if doomed_ids.is_empty() {
        return Ok(AssetGcReport::default());
    }

    db.query("DELETE asset WHERE asset_id IN $ids")
        .bind(("ids", doomed_ids.clone()))
        .await?
        .check()?;

    let hashes: Vec<String> = doomed_hashes.into_iter().collect();
    let mut still_named: HashSet<String> = strings(
        db,
        "SELECT VALUE content_hash FROM asset WHERE content_hash IN $hashes",
        Some(&hashes),
    )
    .await?
    .into_iter()
    .collect();
    still_named.extend(
        strings(
            db,
            "SELECT VALUE asset_hash FROM crate_entry WHERE asset_hash IN $hashes",
            Some(&hashes),
        )
        .await?,
    );
    // Path stamps name their GLB by `blob://{hash}.{ext}` (fe-sdk `path_asset`), not asset_id.
    still_named.extend(
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

    let mut removed_blobs = 0;
    for hex in hashes.iter().filter(|h| !still_named.contains(*h)) {
        let Ok(hash) = hash_from_hex(hex) else {
            continue;
        };
        if blob_store.has_blob(&hash) {
            blob_store.remove_blob(&hash)?;
            removed_blobs += 1;
        }
    }
    tracing::info!(
        removed_assets = doomed_ids.len(),
        removed_blobs,
        "asset GC pass"
    );
    Ok(AssetGcReport {
        removed_assets: doomed_ids.len() as u64,
        removed_blobs,
    })
}

/// Run a `SELECT VALUE <string field>` (optionally bound to `$hashes`).
async fn strings(db: &Db, sql: &str, hashes: Option<&[String]>) -> anyhow::Result<Vec<String>> {
    let mut query = db.query(sql);
    if let Some(hashes) = hashes {
        query = query.bind(("hashes", hashes.to_vec()));
    }
    let mut res = query.await?.check()?;
    let values: Vec<serde_json::Value> = res.take(0)?;
    Ok(values
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hash_to_hex, BlobStore};
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    const OLD: &str = "2020-01-01T00:00:00Z";

    async fn setup() -> (Db, Arc<MockBlobStore>, BlobStoreHandle) {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        let store = Arc::new(MockBlobStore::default());
        let handle: BlobStoreHandle = store.clone();
        (db, store, handle)
    }

    /// Store `bytes` and register an asset row for it created at `created_at`.
    async fn asset(db: &Db, store: &BlobStoreHandle, bytes: &[u8], created_at: &str) -> String {
        let hash = hash_to_hex(&store.add_blob(bytes).unwrap());
        let asset_id = ulid::Ulid::new().to_string();
        db.query(
            "CREATE asset CONTENT { asset_id: $aid, name: 'm.glb', \
             content_type: 'model/gltf-binary', size_bytes: 1, content_hash: $hash, \
             owner_did: 'did:key:z6MkOwner', petal_id: 'petal-1', created_at: $at }",
        )
        .bind(("aid", asset_id.clone()))
        .bind(("hash", hash))
        .bind(("at", created_at.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
        asset_id
    }

    async fn place(db: &Db, asset_id: &str) {
        db.query(
            "CREATE node CONTENT { node_id: $nid, petal_id: 'petal-1', display_name: 'n', \
             asset_id: $aid, position: <geometry<point>> [0.0, 0.0], elevation: 0.0, \
             rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], created_at: $at }",
        )
        .bind(("nid", ulid::Ulid::new().to_string()))
        .bind(("aid", asset_id.to_string()))
        .bind(("at", OLD))
        .await
        .unwrap()
        .check()
        .unwrap();
    }

    async fn asset_ids(db: &Db) -> HashSet<String> {
        strings(db, "SELECT VALUE asset_id FROM asset", None)
            .await
            .unwrap()
            .into_iter()
            .collect()
    }

    fn has(store: &MockBlobStore, bytes: &[u8]) -> bool {
        store.has_blob(blake3::hash(bytes).as_bytes())
    }

    #[tokio::test]
    async fn gc_removes_only_expired_unreferenced_rows_and_unshared_blobs() {
        let (db, store, handle) = setup().await;
        let now = chrono::Utc::now().to_rfc3339();

        let orphan = asset(&db, &handle, b"orphan", OLD).await;
        let placed = asset(&db, &handle, b"placed", OLD).await;
        place(&db, &placed).await;
        let fresh = asset(&db, &handle, b"fresh", &now).await;
        // Two rows share one blob; only one of them is placed.
        let shared_orphan = asset(&db, &handle, b"shared", OLD).await;
        let shared_placed = asset(&db, &handle, b"shared", OLD).await;
        place(&db, &shared_placed).await;
        // A non-asset blob (replicated row) in the same store.
        let (repl_tx, _repl_rx) = crossbeam::channel::bounded(4);
        crate::replicate_row(
            Some(&repl_tx),
            &handle,
            "verse-1",
            "node",
            "n1",
            b"{\"row\":1}",
        );
        assert!(has(&store, b"{\"row\":1}"));

        let report = gc_unreferenced_assets(&db, &handle, 3600).await.unwrap();

        assert_eq!(
            report,
            AssetGcReport {
                removed_assets: 2,
                removed_blobs: 1
            }
        );
        let left = asset_ids(&db).await;
        assert!(!left.contains(&orphan) && !left.contains(&shared_orphan));
        assert!(left.contains(&placed) && left.contains(&fresh) && left.contains(&shared_placed));
        assert!(!has(&store, b"orphan"), "unshared orphan blob removed");
        for kept in [&b"placed"[..], b"fresh", b"shared", b"{\"row\":1}"] {
            assert!(
                has(&store, kept),
                "{:?} must survive",
                String::from_utf8_lossy(kept)
            );
        }
    }

    #[tokio::test]
    async fn blob_named_by_a_crate_entry_survives_its_asset_row() {
        let (db, store, handle) = setup().await;
        asset(&db, &handle, b"crate model", OLD).await;
        db.query("CREATE crate_entry CONTENT { entry_id: 'e1', hexon_uri: 'x@1', kind: 'model', asset_hash: $h, format: 'glb', label: 'l' }")
            .bind(("h", hash_to_hex(blake3::hash(b"crate model").as_bytes())))
            .await
            .unwrap()
            .check()
            .unwrap();

        let report = gc_unreferenced_assets(&db, &handle, 0).await.unwrap();

        assert_eq!(report.removed_assets, 1);
        assert_eq!(report.removed_blobs, 0);
        assert!(has(&store, b"crate model"));
    }

    /// A track whose `path_asset` stamps an expired, never-placed upload by its
    /// `blob://` URI keeps that blob (the stamps would otherwise lose their mesh).
    #[tokio::test]
    async fn blob_stamped_by_a_path_asset_survives_its_asset_row() {
        let (db, store, handle) = setup().await;
        asset(&db, &handle, b"stamp mesh", OLD).await;
        let hash = hash_to_hex(blake3::hash(b"stamp mesh").as_bytes());
        db.query(
            "CREATE node CONTENT { node_id: 'track-1', petal_id: 'petal-1', display_name: 't', \
             asset_id: NONE, position: <geometry<point>> [0.0, 0.0], elevation: 0.0, \
             rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], created_at: $at, \
             properties: { path_asset: { asset_path: $uri } } }",
        )
        .bind(("at", OLD))
        .bind(("uri", format!("blob://{hash}.glb")))
        .await
        .unwrap()
        .check()
        .unwrap();

        let report = gc_unreferenced_assets(&db, &handle, 0).await.unwrap();

        assert_eq!(
            report.removed_assets, 1,
            "the unplaced row itself is collected"
        );
        assert_eq!(report.removed_blobs, 0);
        assert!(has(&store, b"stamp mesh"));
    }
}
