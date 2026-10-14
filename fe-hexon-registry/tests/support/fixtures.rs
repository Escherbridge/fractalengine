//! Minimal in-memory `.hexon` fixture builders (pattern mirrors fe-hexon/tests/support/sample_hexons.rs).

use std::io::Write;
use std::path::Path;

use ed25519_dalek::SigningKey;
use fe_format::{
    AssetEntry, ChunkInput, ElevationEncoding, EntryKind, HexonArchive, HexonManifest, HexonType,
    License, TilesetMeta,
};

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
        integrity: None,
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

/// Deterministic signing key for fixtures — never a real secret.
pub fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

/// A second, distinct deterministic key — for fixtures that must prove "a
/// *different* publisher" (continuity 409).
pub fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[99u8; 32])
}

/// Seal `bytes` with [`test_key`].
pub fn seal(bytes: Vec<u8>) -> Vec<u8> {
    seal_with(bytes, &test_key())
}

/// Seal `bytes` with an explicit key (continuity-violation fixtures need a
/// publisher other than [`test_key`]).
pub fn seal_with(bytes: Vec<u8>, key: &SigningKey) -> Vec<u8> {
    fe_format::seal(&bytes, key).expect("fixture seal failed")
}

/// Rebuild `zip_bytes`, replacing member `name`'s content with `replacement`;
/// every other member is copied byte-for-byte. The fixture for "a sealed
/// archive whose body no longer matches its signed digest" — `seal()` itself
/// refuses to produce a genuinely mismatched archive (it computes digests
/// from the real bytes), so a forged mismatch can only exist as a sealed
/// archive tampered with *after* sealing, which is exactly the attack the
/// registry's publish-time `verify_archive` gate exists to catch.
pub fn tamper_member(zip_bytes: &[u8], name: &str, replacement: &[u8]) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflate = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let file_name = archive.name_for_index(i).unwrap().to_string();
        if file_name == name {
            out.start_file(&file_name, deflate).unwrap();
            out.write_all(replacement).unwrap();
        } else {
            out.raw_copy_file(archive.by_index_raw(i).unwrap()).unwrap();
        }
    }
    out.finish().unwrap().into_inner()
}

/// A sealed hexon whose asset no longer matches its catalogued `asset_hash`
/// (the registry's "ledger mismatch" publish case): seal an honest
/// entry/asset pair, then tamper the asset's own bytes in place.
pub fn sealed_asset_hash_mismatch_hexon(id: &str) -> Vec<u8> {
    let blob = b"authentic glb bytes".to_vec();
    let hexon = build_hexon(
        id,
        "1.0.0",
        HexonType::Model,
        "Mismatch",
        "asset_hash mismatch fixture",
        &[],
        "model",
        EntryKind::Model,
        &blob,
    );
    let hash = blake3::hash(&blob).to_hex().to_string();
    let sealed = seal(hexon);
    tamper_member(
        &sealed,
        &format!("assets/{hash}"),
        b"corrupted bytes, wrong hash",
    )
}

/// A minimal `TilesetMeta` for fixture tilesets (bounds/zoom are arbitrary —
/// nothing in these tests reads them).
fn tileset_meta(tile_count: u32) -> TilesetMeta {
    TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 10,
        max_zoom: 10,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        tile_count,
        region_name: "Registry fixture".into(),
        ..Default::default()
    }
}

/// An unsealed tileset archive with `tiles` as `(cache_key, bytes)` elevation
/// tiles — large tiles let `meta_route_header_verifies_client_side_without_tile_bytes`
/// prove the header response stays small regardless of archive size.
pub fn tileset_hexon(id: &str, version: &str, tiles: &[(String, Vec<u8>)]) -> Vec<u8> {
    let m = manifest(
        id,
        version,
        HexonType::TerrainTileset,
        "Fixture Tileset",
        "registry fixture",
        &["terrain"],
    );
    HexonArchive::export_tileset(m, &tileset_meta(tiles.len() as u32), tiles, &[], None)
        .expect("tileset export failed")
}

/// A sealed 3-chunk container: each chunk is itself sealed with [`test_key`]
/// (PAS-P5's "recursive trust" — fe-format/src/AGENTS.md §integrity), then
/// the outer container is sealed too, committing `chunks/index.json`.
pub fn sealed_container_hexon(id: &str) -> Vec<u8> {
    let mut chunks = Vec::new();
    for seq in 0..3u32 {
        let chunk_tiles = vec![(format!("10/{seq}/0"), vec![seq as u8; 256])];
        let chunk_bytes = tileset_hexon(&format!("{id}-chunk-{seq}"), "1.0.0", &chunk_tiles);
        let sealed_chunk = seal(chunk_bytes);
        chunks.push(ChunkInput {
            seq,
            tile_count: 1,
            zoom: 10,
            x_range: [seq, seq],
            y_range: [0, 0],
            hilbert_range: [seq as u64, seq as u64],
            archive_bytes: sealed_chunk,
        });
    }
    let m = manifest(
        id,
        "1.0.0",
        HexonType::TerrainTileset,
        "Fixture Container",
        "registry fixture",
        &["terrain"],
    );
    let unsealed =
        HexonArchive::export_chunked(m, &tileset_meta(3), &chunks).expect("export_chunked failed");
    seal(unsealed)
}

/// A sealed hexon with one body member at `terrain/tracks/notes.html` — an
/// arbitrary, attacker-choosable member name, used to prove `/member` never
/// trusts a file extension for its response `Content-Type` (nosniff fixture).
pub fn sealed_hexon_with_html_member(id: &str) -> Vec<u8> {
    sealed_hexon_with_track_member(id, "notes.html")
}

/// A sealed hexon with one body member at `terrain/tracks/{name}` (any
/// publisher-chosen name, including header-hostile characters).
pub fn sealed_hexon_with_track_member(id: &str, name: &str) -> Vec<u8> {
    let m = manifest(
        id,
        "1.0.0",
        HexonType::Model,
        "Html Member",
        "registry fixture",
        &[],
    );
    let bytes = HexonArchive::export(
        m,
        vec![],
        vec![],
        Some(License::default()),
        None,
        Some(&[(
            name.to_string(),
            b"<script>alert(document.domain)</script>".to_vec(),
        )]),
        None,
    )
    .expect("export failed");
    seal(bytes)
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

/// Write the standard three-fixture set into `dir` as `{id}@{version}.hexon`,
/// sealed with [`test_key`] so they exercise the same sealed path real
/// publishes do (unsealed-specific tests build their own unsealed bytes).
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
        std::fs::write(dir.join(format!("{id}@{version}.hexon")), seal(bytes))
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
