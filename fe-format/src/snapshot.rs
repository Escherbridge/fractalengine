//! PetalSnapshot hexon section: plain-JSON petal rows, ordered op-log and content-addressed blobs (see `src/AGENTS.md` §petal-snapshot).

use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    entries::{AssetEntry, EntryCatalog},
    limits,
    manifest::{HexonManifest, HexonType},
    signature::canonical_json_bytes,
};

/// Snapshot layout version this module reads and writes.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;
/// Op-log predicate version this module reads and writes.
pub const OPLOG_PREDICATE_VERSION: u32 = 1;

const META_MEMBER: &str = "snapshot/meta.json";
const OPLOG_MEMBER: &str = "snapshot/oplog.jsonl";
const STATE_PREFIX: &str = "snapshot/state/";
const BLOB_PREFIX: &str = "assets/";
const LAMPORT_FIELD: &str = "lamport_clock";

/// Lamport span and row count of the exported op-log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpLogRange {
    pub min_lamport: u64,
    pub max_lamport: u64,
    pub count: u64,
}

/// Provenance and shape of one exported petal snapshot (`snapshot/meta.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub snapshot_format_version: u32,
    pub oplog_predicate_version: u32,
    pub petal_id: String,
    pub scope: String,
    pub source_node_id: String,
    pub exported_at: String,
    pub table_row_counts: BTreeMap<String, u64>,
    pub op_log_range: OpLogRange,
}

/// Petal rows as plain JSON so fe-format never depends on fe-database types.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PetalSnapshotData {
    pub petal: Value,
    pub nodes: Vec<Value>,
    pub node_log: Vec<Value>,
    pub field_defs: Vec<Value>,
    pub rooms: Vec<Value>,
    pub models: Vec<Value>,
    pub assets: Vec<Value>,
    pub roles: Vec<Value>,
    pub iot_readings: Vec<Value>,
    pub oplog: Vec<Value>,
}

/// Everything read back out of a petal snapshot archive.
#[derive(Debug, Clone)]
pub struct ImportedPetalSnapshot {
    pub manifest: HexonManifest,
    pub meta: SnapshotMeta,
    pub data: PetalSnapshotData,
    pub entries: Vec<AssetEntry>,
    pub blobs: Vec<(String, Vec<u8>)>,
}

/// The state tables in archive order, paired with the member stem.
fn state_tables(data: &PetalSnapshotData) -> [(&'static str, &[Value]); 8] {
    [
        ("nodes", &data.nodes),
        ("node_log", &data.node_log),
        ("field_defs", &data.field_defs),
        ("rooms", &data.rooms),
        ("models", &data.models),
        ("assets", &data.assets),
        ("roles", &data.roles),
        ("iot_readings", &data.iot_readings),
    ]
}

/// Lamport clock of one op-log row, or an error naming the row.
fn row_lamport(row: &Value, index: usize) -> Result<u64> {
    row.get(LAMPORT_FIELD)
        .and_then(Value::as_u64)
        .with_context(|| format!("oplog row {index} has no u64 {LAMPORT_FIELD}"))
}

/// Refuse an op-log that is not strictly ascending in lamport clock.
fn check_oplog_order(oplog: &[Value]) -> Result<()> {
    let mut previous: Option<u64> = None;
    for (index, row) in oplog.iter().enumerate() {
        let lamport = row_lamport(row, index)?;
        if let Some(prior) = previous {
            ensure!(
                lamport > prior,
                "oplog row {index} lamport {lamport} is not above the previous {prior}"
            );
        }
        previous = Some(lamport);
    }
    Ok(())
}

/// Refuse a meta whose versions or op-log range disagree with the op-log it ships with.
fn check_meta(meta: &SnapshotMeta, oplog: &[Value]) -> Result<()> {
    ensure!(
        meta.snapshot_format_version == SNAPSHOT_FORMAT_VERSION,
        "unsupported snapshot_format_version {}",
        meta.snapshot_format_version
    );
    ensure!(
        meta.oplog_predicate_version == OPLOG_PREDICATE_VERSION,
        "unsupported oplog_predicate_version {}",
        meta.oplog_predicate_version
    );
    ensure!(!meta.petal_id.is_empty(), "snapshot meta has an empty petal_id");
    let range = &meta.op_log_range;
    ensure!(
        range.count == oplog.len() as u64,
        "op_log_range.count {} does not match {} oplog rows",
        range.count,
        oplog.len()
    );
    if let (Some(first), Some(last)) = (oplog.first(), oplog.last()) {
        ensure!(
            range.min_lamport == row_lamport(first, 0)?
                && range.max_lamport == row_lamport(last, oplog.len() - 1)?,
            "op_log_range does not match the first and last oplog lamports"
        );
    }
    Ok(())
}

/// Refuse a meta whose present `table_row_counts` keys name an unknown table or disagree with the shipped rows.
fn check_table_row_counts(meta: &SnapshotMeta, data: &PetalSnapshotData) -> Result<()> {
    // An empty `{}` petal row is the exporter's encoding of "no petal row".
    let petal_rows = match data.petal.as_object() {
        Some(object) if !object.is_empty() => 1,
        _ => 0,
    };
    for (table, claimed) in &meta.table_row_counts {
        let actual = match table.as_str() {
            "petal" => petal_rows,
            "oplog" => data.oplog.len() as u64,
            stem => match state_tables(data).iter().find(|(name, _)| *name == stem) {
                Some((_, rows)) => rows.len() as u64,
                None => bail!("table_row_counts names unknown table {table:?}"),
            },
        };
        ensure!(
            *claimed == actual,
            "table_row_counts[{table}] claims {claimed} rows but the archive ships {actual}"
        );
    }
    Ok(())
}

/// True when `name` is a lowercase 64-char blake3 hex digest.
fn is_blake3_hex(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One JSON array member body (compact serde_json).
fn json_array_bytes(rows: &[Value]) -> Result<Vec<u8>> {
    serde_json::to_vec(rows).context("serialising snapshot state table")
}

/// Write a petal snapshot hexon: manifest, entries, meta, state tables, op-log and blobs.
pub fn export_petal_snapshot(
    manifest: HexonManifest,
    meta: &SnapshotMeta,
    data: &PetalSnapshotData,
    entries: Vec<AssetEntry>,
    blobs: Vec<(String, Vec<u8>)>,
) -> Result<Vec<u8>> {
    ensure!(
        manifest.hexon_type == HexonType::PetalSnapshot,
        "petal snapshot export needs hexon_type PetalSnapshot"
    );
    ensure!(
        data.petal.is_object() || data.petal.is_null(),
        "snapshot petal row must be a JSON object"
    );
    check_oplog_order(&data.oplog)?;
    check_meta(meta, &data.oplog)?;

    let mut sorted_blobs = blobs;
    sorted_blobs.sort_by(|a, b| a.0.cmp(&b.0));
    for (index, (name, bytes)) in sorted_blobs.iter().enumerate() {
        ensure!(is_blake3_hex(name), "blob name {name:?} is not a blake3 hex digest");
        ensure!(
            blake3::hash(bytes).to_hex().as_str() == name,
            "blob {name} does not hash to its name"
        );
        if index > 0 {
            ensure!(sorted_blobs[index - 1].0 != *name, "duplicate blob {name}");
        }
    }

    let manifest_json = canonical_json_bytes(&manifest).context("canonicalising manifest.json")?;
    // Canonical (sorted keys): `sub_assets` is a HashMap, so plain serde output is not byte-stable.
    let entries_json = canonical_json_bytes(&EntryCatalog { entries }).context("entries.json")?;
    let meta_json = serde_json::to_vec(meta).context("snapshot meta")?;
    let petal_json = if data.petal.is_null() {
        b"{}".to_vec()
    } else {
        serde_json::to_vec(&data.petal).context("snapshot petal row")?
    };
    let mut oplog_jsonl = Vec::new();
    for row in &data.oplog {
        serde_json::to_writer(&mut oplog_jsonl, row).context("oplog row")?;
        oplog_jsonl.push(b'\n');
    }

    // Same member options as HexonArchive::export_stored_deterministic (archive.rs), which
    // cannot take an entry catalog; see NEEDS in the slice report.
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    let mut put = |name: &str, body: &[u8]| -> Result<()> {
        zip.start_file(name, options)?;
        zip.write_all(body)?;
        Ok(())
    };
    put("manifest.json", &manifest_json)?;
    put("entries.json", &entries_json)?;
    put(META_MEMBER, &meta_json)?;
    put(&format!("{STATE_PREFIX}petal.json"), &petal_json)?;
    for (stem, rows) in state_tables(data) {
        put(&format!("{STATE_PREFIX}{stem}.json"), &json_array_bytes(rows)?)?;
    }
    put(OPLOG_MEMBER, &oplog_jsonl)?;
    for (name, bytes) in &sorted_blobs {
        put(&format!("{BLOB_PREFIX}{name}"), bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Read a required member under `cap` and the shared decompression `budget`.
fn read_required(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    cap: u64,
    budget: &mut u64,
) -> Result<Vec<u8>> {
    let file = archive
        .by_name(name)
        .with_context(|| format!("missing {name}"))?;
    let declared = file.size();
    limits::read_entry_capped(file, declared, cap, budget)
}

/// Read a petal snapshot hexon, verifying blob hashes and op-log order.
pub fn import_petal_snapshot(bytes: &[u8]) -> Result<ImportedPetalSnapshot> {
    // An archive that claims a seal must verify (fail closed); an unsealed one reads as before.
    crate::integrity::verify_archive(bytes, &crate::integrity::IntegrityPolicy::VerifyIfSealed)
        .map_err(anyhow::Error::from)
        .context("petal snapshot integrity check")?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("failed to open ZIP archive")?;
    limits::check_entry_count(archive.len())?;
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
    let json_cap = limits::MAX_JSON_ENTRY_BYTES;
    let binary_cap = limits::MAX_ENTRY_UNCOMPRESSED_BYTES;

    let manifest: HexonManifest = serde_json::from_slice(&read_required(
        &mut archive,
        "manifest.json",
        json_cap,
        &mut budget,
    )?)
    .context("invalid manifest.json")?;
    if manifest.hexon_type != HexonType::PetalSnapshot {
        bail!("archive is not a PetalSnapshot hexon");
    }
    let catalog: EntryCatalog = serde_json::from_slice(&read_required(
        &mut archive,
        "entries.json",
        json_cap,
        &mut budget,
    )?)
    .context("invalid entries.json")?;
    let meta: SnapshotMeta = serde_json::from_slice(&read_required(
        &mut archive,
        META_MEMBER,
        json_cap,
        &mut budget,
    )?)
    .context("invalid snapshot/meta.json")?;

    let petal: Value = serde_json::from_slice(&read_required(
        &mut archive,
        &format!("{STATE_PREFIX}petal.json"),
        binary_cap,
        &mut budget,
    )?)
    .context("invalid snapshot petal.json")?;
    ensure!(petal.is_object(), "snapshot petal.json must be a JSON object");

    let mut tables: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for stem in [
        "nodes",
        "node_log",
        "field_defs",
        "rooms",
        "models",
        "assets",
        "roles",
        "iot_readings",
    ] {
        let name = format!("{STATE_PREFIX}{stem}.json");
        let rows: Vec<Value> =
            serde_json::from_slice(&read_required(&mut archive, &name, binary_cap, &mut budget)?)
                .with_context(|| format!("invalid {name}"))?;
        tables.insert(stem, rows);
    }
    let mut take = |stem: &str| tables.remove(stem).unwrap_or_default();

    let oplog_bytes = read_required(&mut archive, OPLOG_MEMBER, binary_cap, &mut budget)?;
    let oplog_text = std::str::from_utf8(&oplog_bytes).context("oplog.jsonl is not UTF-8")?;
    let mut oplog = Vec::new();
    for (index, line) in oplog_text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        oplog.push(
            serde_json::from_str::<Value>(line)
                .with_context(|| format!("invalid oplog.jsonl line {}", index + 1))?,
        );
    }
    check_oplog_order(&oplog)?;
    check_meta(&meta, &oplog)?;

    let data = PetalSnapshotData {
        petal,
        nodes: take("nodes"),
        node_log: take("node_log"),
        field_defs: take("field_defs"),
        rooms: take("rooms"),
        models: take("models"),
        assets: take("assets"),
        roles: take("roles"),
        iot_readings: take("iot_readings"),
        oplog,
    };
    check_table_row_counts(&meta, &data)?;

    let mut blob_names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let name = archive.by_index_raw(index)?.name().to_string();
        if let Some(hash) = name.strip_prefix(BLOB_PREFIX) {
            if hash.is_empty() {
                continue;
            }
            ensure!(is_blake3_hex(hash), "blob member {name:?} is not a blake3 hex name");
            blob_names.push(name);
        }
    }
    blob_names.sort();
    blob_names.dedup();
    let mut blobs = Vec::with_capacity(blob_names.len());
    for name in blob_names {
        let body = read_required(&mut archive, &name, binary_cap, &mut budget)?;
        let hash = name[BLOB_PREFIX.len()..].to_string();
        ensure!(
            blake3::hash(&body).to_hex().as_str() == hash,
            "blob {hash} does not hash to its name"
        );
        blobs.push((hash, body));
    }

    Ok(ImportedPetalSnapshot {
        manifest,
        meta,
        data,
        entries: catalog.entries,
        blobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::HexonArchive;
    use crate::entries::EntryKind;
    use serde_json::json;

    fn manifest(hexon_type: HexonType) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "snap".into(),
            hexon_type,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Snapshot".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        }
    }

    fn meta_for(oplog: &[Value]) -> SnapshotMeta {
        let lamport = |row: &Value| row[LAMPORT_FIELD].as_u64().unwrap();
        SnapshotMeta {
            snapshot_format_version: SNAPSHOT_FORMAT_VERSION,
            oplog_predicate_version: OPLOG_PREDICATE_VERSION,
            petal_id: "petal-1".into(),
            scope: "petal".into(),
            source_node_id: "node-a".into(),
            exported_at: "2026-01-02T00:00:00Z".into(),
            table_row_counts: BTreeMap::new(),
            op_log_range: OpLogRange {
                min_lamport: oplog.first().map(lamport).unwrap_or(0),
                max_lamport: oplog.last().map(lamport).unwrap_or(0),
                count: oplog.len() as u64,
            },
        }
    }

    fn entry(asset_hash: &str) -> AssetEntry {
        AssetEntry {
            entry_id: "e1".into(),
            kind: EntryKind::Model,
            asset_hash: asset_hash.into(),
            format: "glb".into(),
            label: "e1".into(),
            tags: vec![],
            description: None,
            is_placeable: true,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }
    }

    fn oplog_rows(lamports: &[u64]) -> Vec<Value> {
        lamports
            .iter()
            .map(|l| json!({ "lamport_clock": l, "op": format!("op-{l}"), "z": [1, 2], "a": null }))
            .collect()
    }

    #[test]
    fn petal_snapshot_round_trips_rows_and_oplog_order() {
        let data = PetalSnapshotData {
            petal: json!({ "petal_id": "petal-1", "name": "Plaza" }),
            nodes: vec![json!({ "node_id": "n1", "pos": [1.0, 2.0, 3.0] }), json!({ "node_id": "n2" })],
            node_log: vec![json!({ "node_id": "n1", "seq": 1 })],
            field_defs: vec![json!({ "key": "temp" })],
            rooms: vec![json!({ "room": "r" })],
            models: vec![json!({ "model": "m" })],
            assets: vec![json!({ "asset": "a" })],
            roles: vec![json!({ "peer": "p", "role": "editor" })],
            iot_readings: vec![json!({ "t": 1, "v": 2.5 })],
            oplog: oplog_rows(&[3, 7, 9, 20]),
        };
        let blob = b"glb-bytes".to_vec();
        let hash = blake3::hash(&blob).to_hex().to_string();
        let mut meta = meta_for(&data.oplog);
        meta.table_row_counts = [
            ("petal", 1),
            ("nodes", 2),
            ("node_log", 1),
            ("field_defs", 1),
            ("rooms", 1),
            ("models", 1),
            ("assets", 1),
            ("roles", 1),
            ("iot_readings", 1),
            ("oplog", 4),
        ]
        .into_iter()
        .map(|(table, count)| (table.to_string(), count))
        .collect();
        let bytes = export_petal_snapshot(
            manifest(HexonType::PetalSnapshot),
            &meta,
            &data,
            vec![entry(&hash)],
            vec![(hash.clone(), blob.clone())],
        )
        .expect("export");

        let imported = import_petal_snapshot(&bytes).expect("import");
        assert_eq!(imported.meta, meta);
        assert_eq!(imported.data, data);
        assert_eq!(
            serde_json::to_string(&imported.data.oplog).unwrap(),
            serde_json::to_string(&data.oplog).unwrap(),
            "oplog order and bytes survive"
        );
        assert_eq!(imported.entries.len(), 1);
        assert_eq!(imported.blobs, vec![(hash, blob)]);
        // Byte-stable: the same inputs always produce the same archive.
        let again = export_petal_snapshot(
            manifest(HexonType::PetalSnapshot),
            &meta,
            &data,
            vec![entry(&imported.blobs[0].0)],
            imported.blobs.clone(),
        )
        .unwrap();
        assert_eq!(bytes, again);

        // Sealed: the snapshot survives a seal, and a sealed copy with an edited state table is refused.
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let sealed = crate::integrity::seal(&bytes, &key).expect("seal");
        assert_eq!(import_petal_snapshot(&sealed).expect("sealed import").data, data);
        let rooms = "snapshot/state/rooms.json";
        let mut source = zip::ZipArchive::new(Cursor::new(&sealed[..])).unwrap();
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..source.len() {
            let raw = source.by_index_raw(index).unwrap();
            if raw.name() == rooms {
                drop(raw);
                out.start_file(rooms, zip::write::SimpleFileOptions::default()).unwrap();
                out.write_all(b"[]").unwrap();
            } else {
                out.raw_copy_file(raw).unwrap();
            }
        }
        let edited = out.finish().unwrap().into_inner();
        assert!(import_petal_snapshot(&edited).is_err(), "edited table in a sealed snapshot");
    }

    #[test]
    fn empty_petal_snapshot_is_valid_and_lists_every_state_file() {
        let data = PetalSnapshotData {
            petal: json!({}),
            ..PetalSnapshotData::default()
        };
        let bytes = export_petal_snapshot(
            manifest(HexonType::PetalSnapshot),
            &meta_for(&[]),
            &data,
            vec![],
            vec![],
        )
        .expect("export empty");
        let names: Vec<String> = HexonArchive::member_order(&bytes)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for expected in [
            "snapshot/meta.json",
            "snapshot/oplog.jsonl",
            "snapshot/state/petal.json",
            "snapshot/state/nodes.json",
            "snapshot/state/node_log.json",
            "snapshot/state/field_defs.json",
            "snapshot/state/rooms.json",
            "snapshot/state/models.json",
            "snapshot/state/assets.json",
            "snapshot/state/roles.json",
            "snapshot/state/iot_readings.json",
        ] {
            assert!(names.iter().any(|n| n == expected), "missing {expected}");
        }
        let imported = import_petal_snapshot(&bytes).expect("import empty");
        assert_eq!(imported.data, data);
        assert!(imported.blobs.is_empty());
    }

    #[test]
    fn petal_snapshot_refuses_tampered_blob_wrong_type_and_descending_oplog() {
        let blob = b"glb-bytes".to_vec();
        let hash = blake3::hash(&blob).to_hex().to_string();
        let good_data = PetalSnapshotData {
            petal: json!({}),
            oplog: oplog_rows(&[1, 2]),
            ..PetalSnapshotData::default()
        };
        let good = export_petal_snapshot(
            manifest(HexonType::PetalSnapshot),
            &meta_for(&good_data.oplog),
            &good_data,
            vec![],
            vec![(hash.clone(), blob.clone())],
        )
        .unwrap();
        let tampered = crate::test_support::tamper_member(&good, |name, _| {
            name.starts_with(BLOB_PREFIX)
        });
        assert!(import_petal_snapshot(&tampered).is_err(), "tampered blob");

        let descending = PetalSnapshotData {
            petal: json!({}),
            oplog: oplog_rows(&[5, 4]),
            ..PetalSnapshotData::default()
        };
        // (manifest type, data, blobs): every row must be refused at export.
        let refusals: Vec<(HexonType, &PetalSnapshotData, Vec<(String, Vec<u8>)>)> = vec![
            (HexonType::Scene, &good_data, vec![]),
            (HexonType::PetalSnapshot, &descending, vec![]),
            (HexonType::PetalSnapshot, &good_data, vec![(hash, b"other".to_vec())]),
        ];
        for (hexon_type, data, blobs) in refusals {
            let result = export_petal_snapshot(
                manifest(hexon_type),
                &meta_for(&data.oplog),
                data,
                vec![],
                blobs,
            );
            assert!(result.is_err());
        }

        // Table rows: export does not cross-check them, so a lying meta reaches import, which must refuse it.
        let two_nodes = PetalSnapshotData {
            petal: json!({ "petal_id": "petal-1" }),
            nodes: vec![json!({ "node_id": "n1" }), json!({ "node_id": "n2" })],
            oplog: oplog_rows(&[1, 2]),
            ..PetalSnapshotData::default()
        };
        let import_with_counts = |counts: &[(&str, u64)]| {
            let mut meta = meta_for(&two_nodes.oplog);
            meta.table_row_counts = counts.iter().map(|(t, c)| (t.to_string(), *c)).collect();
            let bytes = export_petal_snapshot(
                manifest(HexonType::PetalSnapshot),
                &meta,
                &two_nodes,
                vec![],
                vec![],
            )
            .unwrap();
            import_petal_snapshot(&bytes)
        };
        let wrong_nodes = import_with_counts(&[("nodes", 3)]).unwrap_err().to_string();
        assert!(wrong_nodes.contains("nodes"), "names the table: {wrong_nodes}");
        assert!(import_with_counts(&[("gadgets", 0)]).is_err(), "unknown table key");
        assert!(import_with_counts(&[("petal", 0)]).is_err(), "petal row is present");
        assert!(import_with_counts(&[("oplog", 5)]).is_err(), "oplog count lies");
        assert!(
            import_with_counts(&[("petal", 1), ("nodes", 2), ("rooms", 0), ("oplog", 2)]).is_ok(),
            "a correct map passes"
        );
        assert!(import_with_counts(&[]).is_ok(), "an empty map passes");
    }
}
