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
pub fn asset_entry(entry_id: &str, kind: EntryKind, format: &str, bytes: &[u8]) -> (AssetEntry, (String, Vec<u8>)) {
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
        std::fs::write(dir.join(format!("{id}@{version}.hexon")), bytes).expect("fixture write failed");
    }
}
