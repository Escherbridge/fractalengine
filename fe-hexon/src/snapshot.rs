//! Petal snapshot runtime: blob attach on export, verify-then-write blob restore on import (see `src/AGENTS.md` §petal-snapshot).

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use fe_format::snapshot::{
    export_petal_snapshot, import_petal_snapshot, OpLogRange, PetalSnapshotData, SnapshotMeta,
    SNAPSHOT_FORMAT_VERSION,
};
use fe_format::{
    sign_manifest, verifying_key_to_did, AssetEntry, EntryKind, HexonManifest, HexonType,
};
use fe_runtime::petal_snapshot::{PetalSnapshotRows, OPLOG_PREDICATE_VERSION};
use serde_json::Value;

use crate::registry::{FsBlobStore, RegistryError};
use crate::signature::asset_hash;

const LAMPORT_FIELD: &str = "lamport_clock";
const SNAPSHOT_HEXON_VERSION: &str = "1.0.0";
const UNSIGNED_PUBLISHER: &str = "unsigned";

/// Why a snapshot archive could not be built or restored.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// An asset row names a blob the store does not hold.
    #[error("missing blob {0}")]
    MissingBlob(String),
    /// A blob's bytes do not hash to the name that references them.
    #[error("blob hash mismatch {0}")]
    BlobHashMismatch(String),
    /// The petal row carries no usable id.
    #[error("petal row has no petal_id or id")]
    MissingPetalId,
    /// An op-log row has no u64 lamport clock.
    #[error("oplog row {0} has no u64 lamport_clock")]
    InvalidOplog(usize),
    /// The archive layer refused the content.
    #[error("snapshot archive: {0}")]
    Archive(String),
    /// The blob store failed while writing.
    #[error("blob store: {0}")]
    Store(#[from] RegistryError),
}

/// Petal id from the petal row (`petal_id`, else a `petal:`-prefixed `id`).
fn petal_id_of(petal: &Value) -> Result<String, SnapshotError> {
    let text = petal
        .get("petal_id")
        .or_else(|| petal.get("id"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(SnapshotError::MissingPetalId)?;
    Ok(text.strip_prefix("petal:").unwrap_or(text).to_string())
}

/// Entry kind for an asset row's content type.
fn entry_kind_of(content_type: &str) -> EntryKind {
    let lowered = content_type.to_ascii_lowercase();
    if lowered.starts_with("image/") {
        EntryKind::Texture
    } else if lowered.starts_with("audio/") {
        EntryKind::Sound
    } else if lowered.contains("gpx") {
        EntryKind::GpxTrack
    } else if lowered.contains("geo+json") || lowered.contains("geojson") {
        EntryKind::GeoJson
    } else {
        lowered.parse::<EntryKind>().unwrap_or(EntryKind::Model)
    }
}

/// Content hash of an asset row, if it references a blob.
fn content_hash_of(row: &Value) -> Option<&str> {
    row.get("content_hash")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty())
}

/// Lamport span and count of the op-log rows.
fn oplog_range(oplog: &[Value]) -> Result<OpLogRange, SnapshotError> {
    let lamport = |index: usize| {
        oplog[index]
            .get(LAMPORT_FIELD)
            .and_then(Value::as_u64)
            .ok_or(SnapshotError::InvalidOplog(index))
    };
    if oplog.is_empty() {
        return Ok(OpLogRange::default());
    }
    Ok(OpLogRange {
        min_lamport: lamport(0)?,
        max_lamport: lamport(oplog.len() - 1)?,
        count: oplog.len() as u64,
    })
}

/// Rows to the archive layer's data shape.
fn data_from_rows(rows: &PetalSnapshotRows) -> PetalSnapshotData {
    PetalSnapshotData {
        petal: rows.petal.clone(),
        nodes: rows.nodes.clone(),
        node_log: rows.node_log.clone(),
        field_defs: rows.field_defs.clone(),
        rooms: rows.rooms.clone(),
        models: rows.models.clone(),
        assets: rows.assets.clone(),
        roles: rows.roles.clone(),
        iot_readings: rows.iot_readings.clone(),
        oplog: rows.oplog.clone(),
    }
}

/// Archive data back to DB-seam rows.
fn rows_from_data(data: PetalSnapshotData) -> PetalSnapshotRows {
    PetalSnapshotRows {
        petal: data.petal,
        nodes: data.nodes,
        node_log: data.node_log,
        field_defs: data.field_defs,
        rooms: data.rooms,
        models: data.models,
        assets: data.assets,
        roles: data.roles,
        iot_readings: data.iot_readings,
        oplog: data.oplog,
    }
}

/// Build a petal snapshot hexon, attaching every referenced blob after verifying its hash.
pub fn build_snapshot_archive(
    rows: &PetalSnapshotRows,
    blobs: &FsBlobStore,
    signer: Option<&SigningKey>,
    source_node_id: &str,
    scope: &str,
) -> Result<Vec<u8>, SnapshotError> {
    let petal_id = petal_id_of(&rows.petal)?;
    let op_log_range = oplog_range(&rows.oplog)?;

    // One entry and one blob per distinct hash, in hash order (BTreeMap), so export is deterministic.
    let mut by_hash: BTreeMap<String, &Value> = BTreeMap::new();
    for row in &rows.assets {
        if let Some(hash) = content_hash_of(row) {
            by_hash.entry(hash.to_ascii_lowercase()).or_insert(row);
        }
    }
    let mut entries = Vec::with_capacity(by_hash.len());
    let mut blob_payloads = Vec::with_capacity(by_hash.len());
    for (hash, row) in by_hash {
        let bytes = blobs
            .load(&hash)
            .map_err(|_| SnapshotError::MissingBlob(hash.clone()))?;
        if asset_hash(&bytes) != hash {
            return Err(SnapshotError::BlobHashMismatch(hash));
        }
        let content_type = row
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let label = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&hash)
            .to_string();
        entries.push(AssetEntry {
            entry_id: hash.clone(),
            kind: entry_kind_of(content_type),
            asset_hash: hash.clone(),
            format: content_type.to_string(),
            label,
            tags: Vec::new(),
            description: None,
            is_placeable: false,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        });
        blob_payloads.push((hash, bytes));
    }

    let exported_at = chrono::Utc::now().to_rfc3339();
    let table_row_counts = rows
        .table_row_counts()
        .into_iter()
        .map(|(table, count)| (table, count as u64))
        .collect();
    let meta = SnapshotMeta {
        snapshot_format_version: SNAPSHOT_FORMAT_VERSION,
        oplog_predicate_version: OPLOG_PREDICATE_VERSION,
        petal_id: petal_id.clone(),
        scope: scope.to_string(),
        source_node_id: source_node_id.to_string(),
        exported_at: exported_at.clone(),
        table_row_counts,
        op_log_range,
    };

    let mut manifest = HexonManifest {
        schema_version: "1.0.0".to_string(),
        hexon_id: petal_id.clone(),
        hexon_type: HexonType::PetalSnapshot,
        publisher_did: signer
            .map(|key| verifying_key_to_did(&key.verifying_key()))
            .unwrap_or_else(|| UNSIGNED_PUBLISHER.to_string()),
        publisher_name: None,
        version: SNAPSHOT_HEXON_VERSION.to_string(),
        build_id: None,
        name: petal_id,
        description: None,
        tags: Vec::new(),
        created_at: exported_at.clone(),
        updated_at: exported_at,
        source_peer_did: None,
        approx_size_bytes: Some(blob_payloads.iter().map(|(_, bytes)| bytes.len() as u64).sum()),
        min_engine_version: None,
        homepage_url: None,
        dependencies: Vec::new(),
        platforms: Vec::new(),
        address: None,
        integrity: None,
        signature: None,
    };
    if let Some(key) = signer {
        let manifest_json = serde_json::to_vec(&manifest)
            .map_err(|error| SnapshotError::Archive(error.to_string()))?;
        manifest.signature = Some(
            sign_manifest(&manifest_json, key)
                .map_err(|error| SnapshotError::Archive(error.to_string()))?,
        );
    }

    export_petal_snapshot(manifest, &meta, &data_from_rows(rows), entries, blob_payloads)
        .map_err(|error| SnapshotError::Archive(format!("{error:#}")))
}

/// Verify an archive and its blobs, write the blobs, then return the rows for `ImportPetalSnapshot`.
pub fn restore_snapshot_archive(
    bytes: &[u8],
    blobs: &FsBlobStore,
) -> Result<PetalSnapshotRows, SnapshotError> {
    // Import re-hashes every blob; nothing is written until it and the cross-check pass.
    let imported = import_petal_snapshot(bytes)
        .map_err(|error| SnapshotError::Archive(format!("{error:#}")))?;

    let shipped: BTreeSet<&str> = imported.blobs.iter().map(|(hash, _)| hash.as_str()).collect();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for row in &imported.data.assets {
        if let Some(hash) = content_hash_of(row) {
            let hash = hash.to_ascii_lowercase();
            if !shipped.contains(hash.as_str()) {
                return Err(SnapshotError::MissingBlob(hash));
            }
            referenced.insert(hash);
        }
    }
    // A blob no asset row references would be stored with nothing pointing at it: refuse the archive.
    if let Some(unreferenced) = shipped.iter().find(|hash| !referenced.contains(**hash)) {
        return Err(SnapshotError::Archive(format!("unreferenced blob {unreferenced}")));
    }

    for (hash, body) in &imported.blobs {
        blobs.store(hash, body)?;
    }
    Ok(rows_from_data(imported.data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows_with_assets(assets: Vec<Value>) -> PetalSnapshotRows {
        PetalSnapshotRows {
            petal: json!({"petal_id": "petal-1", "name": "Plaza"}),
            nodes: vec![json!({"node_id": "n1"})],
            assets,
            oplog: vec![
                json!({"lamport_clock": 3, "op": "a"}),
                json!({"lamport_clock": 7, "op": "b"}),
            ],
            ..Default::default()
        }
    }

    fn asset_row(hash: &str) -> Value {
        json!({"content_hash": hash, "content_type": "model/gltf-binary", "name": "tree"})
    }

    #[test]
    fn snapshot_blob_stage_refuses_missing_and_mismatched_blobs_and_round_trips_good_ones() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = FsBlobStore::new(source_dir.path().to_path_buf());
        let good_bytes = b"glb bytes of the tree".to_vec();
        let good_hash = asset_hash(&good_bytes);
        source.store(&good_hash, &good_bytes).unwrap();

        // A blob whose file content does not hash to its name.
        let bad_bytes = b"original".to_vec();
        let bad_hash = asset_hash(&bad_bytes);
        let bad_path = fe_runtime::blob_store::canonical_blob_path(
            source_dir.path(),
            &fe_runtime::blob_store::hash_from_hex(&bad_hash).unwrap(),
        );
        std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
        std::fs::write(&bad_path, b"tampered").unwrap();
        let absent_hash = asset_hash(b"never stored");

        // Export refusals.
        let refusals = [
            (absent_hash.clone(), true),
            (bad_hash.clone(), false),
        ];
        for (hash, expect_missing) in refusals {
            let rows = rows_with_assets(vec![asset_row(&hash)]);
            let error = build_snapshot_archive(&rows, &source, None, "node-a", "petal").unwrap_err();
            match (expect_missing, &error) {
                (true, SnapshotError::MissingBlob(named)) => assert_eq!(named, &hash),
                (false, SnapshotError::BlobHashMismatch(named)) => assert_eq!(named, &hash),
                _ => panic!("unexpected error {error:?} for {hash}"),
            }
        }

        // Good export, signed, restored into an empty store.
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let rows = rows_with_assets(vec![asset_row(&good_hash), asset_row(&good_hash)]);
        let archive =
            build_snapshot_archive(&rows, &source, Some(&key), "node-a", "petal").unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let target = FsBlobStore::new(target_dir.path().to_path_buf());
        let restored = restore_snapshot_archive(&archive, &target).unwrap();
        assert_eq!(restored, rows);
        assert_eq!(target.load(&good_hash).unwrap(), good_bytes);

        // Corrupt one blob byte inside the archive: refused before any blob is written.
        let mut corrupted = archive.clone();
        let position = corrupted
            .windows(good_bytes.len())
            .position(|window| window == good_bytes.as_slice())
            .expect("stored blob bytes appear verbatim in the archive");
        corrupted[position] ^= 0xff;
        let clean_dir = tempfile::tempdir().unwrap();
        let clean = FsBlobStore::new(clean_dir.path().to_path_buf());
        assert!(restore_snapshot_archive(&corrupted, &clean).is_err());
        assert!(!clean.has(&good_hash));
    }
}
