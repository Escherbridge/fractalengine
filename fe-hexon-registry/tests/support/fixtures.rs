//! Minimal in-memory `.hexon` fixture builders (pattern mirrors fe-hexon/tests/support/sample_hexons.rs).

use std::path::Path;

use fe_format::{AssetEntry, EntryKind, HexonArchive, HexonManifest, HexonType, License};

/// Build a minimal valid manifest.
pub fn manifest(
    id: &str,
    version: &str,
    hexon_type: HexonType,
    name: &str,
    description: &str,
    tags: &[&str],
) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: id.into(),
        hexon_type,
        publisher_did: "did:key:z6MkTestPublisher".into(),
        publisher_name: Some("Test Publisher".into()),
        version: version.into(),
        build_id: None,
        name: name.into(),
        description: Some(description.into()),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created_at: "2026-07-01T00:00:00Z".into(),
        updated_at: "2026-07-01T00:00:00Z".into(),
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        signature: None,
    }
}

/// Build an asset entry + content-addressed blob pair for `bytes`.
pub fn asset_entry(
    entry_id: &str,
    kind: EntryKind,
    format: &str,
    bytes: &[u8],
) -> (AssetEntry, (String, Vec<u8>)) {
    let hash = blake3::hash(bytes).to_hex().to_string();
    let entry = AssetEntry {
        entry_id: entry_id.into(),
        kind,
        asset_hash: hash.clone(),
        format: format.into(),
        label: entry_id.into(),
        tags: vec![],
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
    };
    (entry, (hash, bytes.to_vec()))
}

/// Build one `.hexon` archive with a single asset entry.
#[allow(clippy::too_many_arguments)] // test fixture — params mirror manifest fields
pub fn build_hexon(
    id: &str,
    version: &str,
    hexon_type: HexonType,
    name: &str,
    description: &str,
    tags: &[&str],
    entry_id: &str,
    kind: EntryKind,
    blob: &[u8],
) -> Vec<u8> {
    let (entry, asset) = asset_entry(entry_id, kind, "bin", blob);
    HexonArchive::export(
        manifest(id, version, hexon_type, name, description, tags),
        vec![entry],
        vec![asset],
        Some(License::default()),
        None,
        None,
        None,
    )
    .expect("fixture export failed")
}

/// Blob payloads shared with assertions.
pub const ALPINE_V1_BLOB: &[u8] = b"alpine tile data v1.0.0";
pub const ALPINE_V12_BLOB: &[u8] = b"alpine tile data v1.2.0 (newer)";
pub const GPX_BLOB: &[u8] = b"<gpx><trk><name>Morning Run</name></trk></gpx>";

/// Write the standard three-fixture set into `dir` as `{id}@{version}.hexon`.
pub fn write_fixture_set(dir: &Path) {
    let fixtures = [
        (
            "alpine-terrain",
            "1.0.0",
            build_hexon(
                "alpine-terrain",
                "1.0.0",
                HexonType::Terrain,
                "Alpine Terrain Pack",
                "Demo terrain tiles for the Alps",
                &["terrain", "alps"],
                "tileset_a",
                EntryKind::TerrainTileset,
                ALPINE_V1_BLOB,
            ),
        ),
        (
            "alpine-terrain",
            "1.2.0",
            build_hexon(
                "alpine-terrain",
                "1.2.0",
                HexonType::Terrain,
                "Alpine Terrain Pack",
                "Demo terrain tiles for the Alps",
                &["terrain", "alps"],
                "tileset_a",
                EntryKind::TerrainTileset,
                ALPINE_V12_BLOB,
            ),
        ),
        (
            "morning-run",
            "0.1.0",
            build_hexon(
                "morning-run",
                "0.1.0",
                HexonType::GpxCollection,
                "Morning Run",
                "GPX run along the lake",
                &["gpx", "running"],
                "morning_run",
                EntryKind::GpxTrack,
                GPX_BLOB,
            ),
        ),
    ];
    for (id, version, bytes) in fixtures {
        std::fs::write(dir.join(format!("{id}@{version}.hexon")), bytes)
            .expect("fixture write failed");
    }
}

/// Zero bytes one bomb entry inflates to: just over the 64 MiB per-entry cap,
/// while the compressed archive stays well under 1 MiB.
pub const BOMB_INFLATED_BYTES: u64 = (64 << 20) + (8 << 20);

/// A hexon with a valid manifest and catalog whose `bomb_entry` inflates to
/// `inflated` zero bytes; streamed in 1 MiB chunks, never held in memory.
/// The catalog lists entry `bomb` with asset hash `bomb` (→ `assets/bomb`).
pub fn bomb_hexon(id: &str, bomb_entry: &str, inflated: u64) -> Vec<u8> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let plain = SimpleFileOptions::default();
    zip.start_file("manifest.json", plain).unwrap();
    let manifest = manifest(id, "1.0.0", HexonType::Model, "Bomb", "zip bomb", &[]);
    zip.write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    if bomb_entry != "entries.json" {
        zip.start_file("entries.json", plain).unwrap();
        zip.write_all(br#"{"entries":[{"entry_id":"bomb","kind":"model","asset_hash":"bomb","format":"bin","label":"bomb"}]}"#)
            .unwrap();
    }
    let deflate = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(1));
    zip.start_file(bomb_entry, deflate).unwrap();
    let chunk = vec![0u8; 1 << 20];
    let mut left = inflated;
    while left > 0 {
        let n = left.min(chunk.len() as u64) as usize;
        zip.write_all(&chunk[..n]).unwrap();
        left -= n as u64;
    }
    zip.finish().unwrap().into_inner()
}

/// Rewrite the declared uncompressed size of `name` in its local and central
/// headers — the "lying header" bomb variant.
pub fn falsify_declared_size(zip: &mut [u8], name: &str, declared: u32) {
    let name = name.as_bytes();
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let mut patched = 0;
    for i in 0..zip.len().saturating_sub(46) {
        let (sig, size_at, name_len_at, name_at) = match &zip[i..i + 4] {
            [0x50, 0x4b, 3, 4] => (0, 22, 26, 30),
            [0x50, 0x4b, 1, 2] => (1, 24, 28, 46),
            _ => continue,
        };
        let len = u16_at(zip, i + name_len_at);
        if zip.get(i + name_at..i + name_at + len) == Some(name) {
            zip[i + size_at..i + size_at + 4].copy_from_slice(&declared.to_le_bytes());
            patched |= 1 << sig;
        }
    }
    assert_eq!(
        patched, 0b11,
        "both headers of the bomb entry must be patched"
    );
}
