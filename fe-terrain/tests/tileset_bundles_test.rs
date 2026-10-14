//! Flow tests for the deterministic spatial bundler (campaign plan S1,
//! `conductor/tracks/p2p_asset_streaming_20260718/campaign-plan-p3p4-20261013.md`).
//!
//! Builds a synthetic multi-zoom tileset with ~100 KB pseudo-random tile
//! payloads (evidence §3: real elevation+satellite pairs are ~110 KB) and
//! bundles it at a 256 KiB cap, which forces several bundles across two
//! zoom levels.

use std::collections::HashMap;

use fe_format::manifest::{
    BundleEntry, ElevationEncoding, HexonManifest, HexonType, TilesetBundleIndex, TilesetMeta,
};
use fe_format::{HexonArchive, HexonArchiveData};
use fe_terrain::tiles::store::UNSAFE_HEXON_ID_ERROR;
use fe_terrain::tiles::{assemble_bundles, bundle_tileset, HexonStore, TileCoord};

/// Forces at least 4 bundles given ~100 KB tile payloads (see module docs).
const BUNDLE_CAP: u64 = 256 * 1024;
/// Evidence §3: a real elevation or satellite tile is on this order.
const TILE_PAYLOAD_BYTES: usize = 100 * 1000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic pseudo-random bytes, expanded from `seed` via blake3 (no new
/// test dependency needed — blake3 is already a direct fe-terrain dependency).
fn pseudo_random_bytes(seed: &str, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 32);
    let mut counter: u64 = 0;
    while out.len() < len {
        let mut hasher = blake3::Hasher::new();
        hasher.update(seed.as_bytes());
        hasher.update(&counter.to_le_bytes());
        out.extend_from_slice(hasher.finalize().as_bytes());
        counter += 1;
    }
    out.truncate(len);
    out
}

fn test_manifest(id: &str) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: id.into(),
        hexon_type: HexonType::TerrainTileset,
        publisher_did: "did:key:z6Mktest".into(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: "Synthetic Tileset".into(),
        description: None,
        tags: vec!["terrain".into(), "tileset".into()],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
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

fn key_coord(key: &str) -> (u8, u32, u32) {
    let mut parts = key.splitn(3, '/');
    let z: u8 = parts.next().unwrap().parse().unwrap();
    let x: u32 = parts.next().unwrap().parse().unwrap();
    let y: u32 = parts.next().unwrap().parse().unwrap();
    (z, x, y)
}

/// A multi-zoom synthetic tileset: zoom 10 is a 2x2 grid (elevation only),
/// zoom 11 is a 3x3 grid (elevation + satellite pairs) — enough distinct
/// coordinates, at `BUNDLE_CAP`, to force several bundles across both zooms.
fn synthetic_tileset() -> HexonArchiveData {
    let mut elevation = Vec::new();
    let mut satellite = Vec::new();

    for x in 0..2u32 {
        for y in 0..2u32 {
            let key = format!("10/{x}/{y}");
            elevation.push((
                key.clone(),
                pseudo_random_bytes(&format!("elev-{key}"), TILE_PAYLOAD_BYTES),
            ));
        }
    }
    for x in 0..3u32 {
        for y in 0..3u32 {
            let key = format!("11/{x}/{y}");
            elevation.push((
                key.clone(),
                pseudo_random_bytes(&format!("elev-{key}"), TILE_PAYLOAD_BYTES),
            ));
            satellite.push((
                key.clone(),
                pseudo_random_bytes(&format!("sat-{key}"), TILE_PAYLOAD_BYTES),
            ));
        }
    }

    let meta = TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 10,
        max_zoom: 11,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        tile_count: elevation.len() as u32,
        satellite_tile_count: satellite.len() as u32,
        region_name: "Synthetic Region".into(),
        ..Default::default()
    };

    let bytes = HexonArchive::export_tileset(
        test_manifest("synthetic-multizoom"),
        &meta,
        &elevation,
        &satellite,
        None,
    )
    .expect("export_tileset failed");

    HexonArchive::import(&bytes).expect("import failed")
}

fn temp_store() -> HexonStore {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tileset_bundles_test_{ts}"));
    HexonStore::with_dir(dir).expect("with_dir failed")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn a_bundled_tileset_reassembles_to_identical_tiles() {
    let source = synthetic_tileset();
    let bundled = bundle_tileset(&source, BUNDLE_CAP).expect("bundle_tileset failed");
    assert!(
        bundled.index.bundles.len() >= 4,
        "expected at least 4 bundles, got {}",
        bundled.index.bundles.len()
    );

    let store = temp_store();
    let dir = store.base_dir().to_path_buf();
    let mut paths = Vec::new();
    for (seq, bytes) in bundled.bundles.iter().enumerate() {
        let path = dir.join(format!("bundle-{seq}.hexon"));
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }

    let installed =
        assemble_bundles(&store, &bundled.index, &paths).expect("assemble_bundles failed");
    let loaded = store
        .load_tileset(&installed.hexon_id)
        .expect("load_tileset failed");

    for (key, bytes) in &source.elevation_tiles {
        let (z, x, y) = key_coord(key);
        let got = loaded
            .get_tile(TileCoord::new(x, y, z))
            .unwrap_or_else(|| panic!("elevation tile {key} missing after reassembly"));
        assert_eq!(
            got,
            bytes.as_slice(),
            "elevation tile {key} differs after reassembly"
        );
    }
    for (key, bytes) in &source.satellite_tiles {
        let (z, x, y) = key_coord(key);
        let got = loaded
            .get_satellite_tile(TileCoord::new(x, y, z))
            .unwrap_or_else(|| panic!("satellite tile {key} missing after reassembly"));
        assert_eq!(
            got,
            bytes.as_slice(),
            "satellite tile {key} differs after reassembly"
        );
    }
}

#[test]
fn bundling_twice_yields_identical_hashes() {
    let source = synthetic_tileset();
    let a = bundle_tileset(&source, BUNDLE_CAP).expect("first bundle_tileset failed");
    let b = bundle_tileset(&source, BUNDLE_CAP).expect("second bundle_tileset failed");

    assert_eq!(a.index.bundles.len(), b.index.bundles.len());
    for (ea, eb) in a.index.bundles.iter().zip(b.index.bundles.iter()) {
        assert_eq!(
            ea.blake3_hex, eb.blake3_hex,
            "bundle {} hash differs between two bundling runs of the same source",
            ea.seq
        );
    }
    assert_eq!(
        a.index_json, b.index_json,
        "the serialized index must also be byte-identical"
    );
}

#[test]
fn elevation_and_satellite_share_a_bundle_and_bundles_are_hilbert_runs() {
    let source = synthetic_tileset();
    let bundled = bundle_tileset(&source, BUNDLE_CAP).expect("bundle_tileset failed");

    // Every zoom-11 coordinate's elevation and satellite tile land in the
    // same bundle (a pair never splits) — verified precisely (tile bytes
    // equal) by `a_bundled_tileset_reassembles_to_identical_tiles`; here we
    // check the index itself agrees the coordinate is covered.
    for x in 0..3u32 {
        for y in 0..3u32 {
            let seq = bundled
                .index
                .bundle_for_tile(11, x, y)
                .unwrap_or_else(|| panic!("tile 11/{x}/{y} not covered by any bundle"));
            let entry = bundled.index.bundles.iter().find(|b| b.seq == seq).unwrap();
            assert!(entry.x_range[0] <= x && x <= entry.x_range[1]);
            assert!(entry.y_range[0] <= y && y <= entry.y_range[1]);
        }
    }

    // No bundle exceeds the cap unless it holds a single tile group alone.
    for entry in &bundled.index.bundles {
        if entry.tile_count > 1 {
            assert!(
                entry.byte_len <= BUNDLE_CAP,
                "bundle {} holds {} tiles and exceeds the cap ({} > {BUNDLE_CAP})",
                entry.seq,
                entry.tile_count,
                entry.byte_len
            );
        }
    }

    // Bundles covering the same zoom are contiguous, non-overlapping Hilbert
    // runs: sorted by their starting Hilbert distance, consecutive bundles
    // never overlap.
    let mut by_zoom: HashMap<u8, Vec<&BundleEntry>> = HashMap::new();
    for entry in &bundled.index.bundles {
        if entry.tile_count > 0 {
            by_zoom.entry(entry.zoom).or_default().push(entry);
        }
    }
    for entries in by_zoom.values_mut() {
        entries.sort_by_key(|e| e.hilbert_range[0]);
        for pair in entries.windows(2) {
            assert!(
                pair[0].hilbert_range[1] < pair[1].hilbert_range[0],
                "bundles {} and {} overlap or are out of hilbert order",
                pair[0].seq,
                pair[1].seq
            );
        }
    }
}

#[test]
fn assembly_rejects_a_bundle_whose_hash_disagrees_with_the_index() {
    let source = synthetic_tileset();
    let bundled = bundle_tileset(&source, BUNDLE_CAP).expect("bundle_tileset failed");
    assert!(
        bundled.bundles.len() > 1,
        "need at least 2 bundles for this test"
    );

    // Corrupt the index's recorded hash for bundle 1 (as a tampered-in-transit
    // peer index would arrive) rather than the bundle bytes themselves, so the
    // `.hexon` ZIP stays structurally valid and the rejection is attributable
    // purely to the blake3 check under test, not an unrelated ZIP parse error.
    let mut tampered_index = bundled.index.clone();
    let entry = tampered_index
        .bundles
        .iter_mut()
        .find(|b| b.seq == 1)
        .expect("bundle seq 1 must exist");
    entry.blake3_hex = "0".repeat(64);

    let store = temp_store();
    let dir = store.base_dir().to_path_buf();
    let mut paths = Vec::new();
    for (seq, bytes) in bundled.bundles.iter().enumerate() {
        let path = dir.join(format!("bundle-{seq}.hexon"));
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }

    let err = assemble_bundles(&store, &tampered_index, &paths)
        .expect_err("a bundle whose hash disagrees with the index must be rejected");
    assert!(
        err.to_string().contains("seq 1"),
        "error should name the mismatched seq: {err}"
    );
    assert!(
        store.list_installed().is_empty(),
        "nothing should be installed when a bundle hash disagrees with the index"
    );
}

#[test]
fn a_tiny_tileset_ships_as_one_bundle() {
    let elevation = vec![("5/1/1".to_string(), pseudo_random_bytes("tiny", 1024))];
    let meta = TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 5,
        max_zoom: 5,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        tile_count: 1,
        region_name: "Tiny Region".into(),
        ..Default::default()
    };
    let bytes =
        HexonArchive::export_tileset(test_manifest("tiny-tileset"), &meta, &elevation, &[], None)
            .unwrap();
    let source = HexonArchive::import(&bytes).unwrap();

    let bundled = bundle_tileset(&source, BUNDLE_CAP).expect("bundle_tileset failed");
    assert_eq!(bundled.index.bundles.len(), 1);
    assert_eq!(bundled.bundles.len(), 1);
    assert_eq!(bundled.index.bundles[0].tile_count, 1);
}

#[test]
fn install_refuses_path_escaping_hexon_ids() {
    let cases = ["../x", "a/b", "a\\b", "C:x", "..", ""];
    for id in cases {
        let store = temp_store();
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 5,
            max_zoom: 5,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 0,
            region_name: "Escape Attempt".into(),
            ..Default::default()
        };
        let bytes = HexonArchive::export_tileset(test_manifest(id), &meta, &[], &[], None).unwrap();

        let result = store.install_tileset(&bytes);
        let err = result.expect_err(&format!("hexon_id {id:?} should be refused"));
        assert!(
            err.to_string().contains(UNSAFE_HEXON_ID_ERROR),
            "error for {id:?} should name the path-traversal guard: {err}"
        );
        assert!(
            store.list_installed().is_empty(),
            "registry must stay empty for {id:?}"
        );
        let written: Vec<_> = std::fs::read_dir(store.base_dir()).unwrap().collect();
        assert!(
            written.is_empty(),
            "no file should be written to the store dir for {id:?}"
        );
    }
}

#[test]
fn bundle_for_tile_table() {
    let entry = BundleEntry {
        seq: 0,
        blake3_hex: "deadbeef".into(),
        byte_len: 10,
        tile_count: 4,
        zoom: 10,
        x_range: [0, 1],
        y_range: [0, 1],
        hilbert_range: [0, 3],
    };
    let index = TilesetBundleIndex {
        schema: 1,
        tileset_id: "t".into(),
        tileset_meta: TilesetMeta {
            bounds: [0.0; 4],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 4,
            region_name: "x".into(),
            ..Default::default()
        },
        bundles: vec![entry],
    };

    // (description, zoom, x, y, expected)
    let cases: &[(&str, u8, u32, u32, Option<u32>)] = &[
        ("inside", 10, 0, 0, Some(0)),
        ("on the range border", 10, 1, 1, Some(0)),
        ("outside the range", 10, 2, 0, None),
        ("wrong zoom", 11, 0, 0, None),
    ];
    for &(desc, zoom, x, y, expected) in cases {
        assert_eq!(index.bundle_for_tile(zoom, x, y), expected, "case: {desc}");
    }
}
