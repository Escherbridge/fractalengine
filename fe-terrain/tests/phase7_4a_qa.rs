//! Phase 7.4a QA — HexonTileSource + CompositeTileSource + TilesetBuilder + Regions + Format.

use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
use fe_format::HexonArchive;
use fe_terrain::tiles::cache::DiskTileCache;
use fe_terrain::tiles::composite::CompositeTileSource;
use fe_terrain::tiles::hexon_source::HexonTileSource;
use fe_terrain::tiles::regions::*;
use fe_terrain::tiles::source::TileCoord;

// ---- helpers ----

fn make_manifest(id: &str) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: id.into(),
        hexon_type: HexonType::TerrainTileset,
        publisher_did: "did:key:z6Mktest".into(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: format!("QA Tileset {id}"),
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
        signature: None,
    }
}

fn make_meta(tile_count: u32) -> TilesetMeta {
    TilesetMeta {
        bounds: [47.0, -123.0, 49.0, -121.0],
        min_zoom: 10,
        max_zoom: 13,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: false,
        tile_count,
        satellite_tile_count: 0,
        region_name: "QA Region".into(),
        parent_tileset: None,
        chunk_index: None,
        native_scale: None,
        ground_sample_distance_m: None,
        crs: None,
        scale_bounds: None,
    }
}

/// Generate 4 tile entries with unique cache keys within the meta bounds [47,-123,49,-121].
fn make_tiles_in_bounds() -> Vec<(String, Vec<u8>)> {
    // Use from_lat_lon at different zoom levels (10-13) to ensure geographic validity.
    // Higher zooms produce smaller tiles so NW corner stays within bounds.
    let coords = [
        TileCoord::from_lat_lon(48.0, -122.0, 10),
        TileCoord::from_lat_lon(48.0, -122.0, 11),
        TileCoord::from_lat_lon(48.0, -122.0, 12),
        TileCoord::from_lat_lon(48.0, -122.0, 13),
    ];
    coords
        .iter()
        .enumerate()
        .map(|(i, c)| (c.cache_key(), vec![(i + 1) as u8; 32]))
        .collect()
}

fn make_hexon_bytes(tiles: &[(String, Vec<u8>)]) -> Vec<u8> {
    let meta = make_meta(tiles.len() as u32);
    HexonArchive::export_tileset(make_manifest("qa-test"), &meta, tiles, &[], None)
        .expect("export_tileset")
}

// =========================================================================
// CHECK 1: HexonTileSource offline
// =========================================================================

#[test]
fn check1_hexon_tile_source_offline_4_tiles() {
    let tiles = make_tiles_in_bounds();
    let bytes = make_hexon_bytes(&tiles);
    let source = HexonTileSource::from_archive(&bytes).expect("from_archive");

    // bounds() matches meta
    assert_eq!(source.bounds(), [47.0, -123.0, 49.0, -121.0]);

    // zoom_range() matches meta
    assert_eq!(source.zoom_range(), (10, 13));

    // All 4 tiles present
    for (key, expected_data) in &tiles {
        let parts: Vec<&str> = key.split('/').collect();
        let coord = TileCoord::new(
            parts[1].parse().unwrap(),
            parts[2].parse().unwrap(),
            parts[0].parse().unwrap(),
        );
        assert!(source.has_tile(coord), "should have tile {key}");
        let data = source.get_tile(coord).expect("get_tile should return data");
        assert_eq!(data, expected_data.as_slice(), "data mismatch for {key}");
    }

    // Out-of-range coord returns None
    let out = TileCoord::new(0, 0, 5);
    assert!(!source.has_tile(out));
    assert!(source.get_tile(out).is_none());

    // Wrong zoom returns None even for valid x/y
    let first_key = &tiles[0].0;
    let parts: Vec<&str> = first_key.split('/').collect();
    let wrong_zoom = TileCoord::new(
        parts[1].parse().unwrap(),
        parts[2].parse().unwrap(),
        15, // way outside zoom range
    );
    assert!(source.get_tile(wrong_zoom).is_none());
}

#[test]
fn check1_hexon_tile_source_satellite() {
    let meta = make_meta(0);
    let sat_tiles = vec![("5/5/5".to_string(), vec![0xFFu8; 16])];
    let bytes = HexonArchive::export_tileset(
        make_manifest("qa-sat"),
        &meta,
        &[],        // no elevation
        &sat_tiles, // satellite
        None,
    )
    .expect("export_tileset");

    let source = HexonTileSource::from_archive(&bytes).unwrap();
    let coord = TileCoord::new(5, 5, 5);
    assert!(source.get_satellite_tile(coord).is_some());
    assert_eq!(source.get_satellite_tile(coord).unwrap(), &[0xFFu8; 16]);
    assert!(source.get_tile(coord).is_none()); // no elevation tile
}

// =========================================================================
// CHECK 2: CompositeTileSource fallback
// =========================================================================

#[test]
fn check2_composite_hexon_hit() {
    let tiles = make_tiles_in_bounds();
    let bytes = make_hexon_bytes(&tiles);
    let source = HexonTileSource::from_archive(&bytes).unwrap();

    let mut composite = CompositeTileSource::new();
    composite.add_hexon_source(source);

    // Tile inside hexon bounds → returns data
    let first_key = &tiles[0].0;
    let parts: Vec<&str> = first_key.split('/').collect();
    let coord = TileCoord::new(
        parts[1].parse().unwrap(),
        parts[2].parse().unwrap(),
        parts[0].parse().unwrap(),
    );
    let data = composite.get_tile_sync(coord);
    assert!(data.is_some(), "should find tile {first_key} via composite");
    assert_eq!(data.unwrap(), tiles[0].1);
}

#[test]
fn check2_composite_miss_returns_none() {
    let tiles = make_tiles_in_bounds();
    let bytes = make_hexon_bytes(&tiles);
    let source = HexonTileSource::from_archive(&bytes).unwrap();

    let mut composite = CompositeTileSource::new();
    composite.add_hexon_source(source);

    // Tile outside bounds → None (no online source)
    let out = TileCoord::new(0, 0, 5);
    assert!(composite.get_tile_sync(out).is_none());
}

#[test]
fn check2_composite_disk_cache_fallback() {
    let dir = std::env::temp_dir().join("fe_qa_composite_cache");
    let _ = std::fs::remove_dir_all(&dir);

    let cache = DiskTileCache::new(&dir, 1024 * 1024);
    let test_data = b"cached tile bytes";
    cache.put("composite", "5/99/99", test_data).unwrap();

    // No hexon sources, but cache has the tile
    let mut composite = CompositeTileSource::new();
    composite.set_cache(cache);

    let coord = TileCoord::new(99, 99, 5);
    let data = composite.get_tile_sync(coord);
    assert!(data.is_some(), "should find tile in disk cache");
    assert_eq!(data.unwrap(), test_data);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check2_composite_hexon_before_cache() {
    // If hexon has the tile, cache should NOT be consulted (hexon wins)
    let tiles = make_tiles_in_bounds();
    let bytes = make_hexon_bytes(&tiles);
    let source = HexonTileSource::from_archive(&bytes).unwrap();

    let dir = std::env::temp_dir().join("fe_qa_composite_order");
    let _ = std::fs::remove_dir_all(&dir);
    let cache = DiskTileCache::new(&dir, 1024 * 1024);

    // Put a DIFFERENT value in cache for the same key
    let first_key = &tiles[0].0;
    cache.put("composite", first_key, b"from-cache").unwrap();

    let mut composite = CompositeTileSource::new();
    composite.add_hexon_source(source);
    composite.set_cache(cache);

    let parts: Vec<&str> = first_key.split('/').collect();
    let coord = TileCoord::new(
        parts[1].parse().unwrap(),
        parts[2].parse().unwrap(),
        parts[0].parse().unwrap(),
    );
    let data = composite.get_tile_sync(coord).unwrap();
    // Should get hexon data, NOT cache data
    assert_eq!(data, tiles[0].1, "hexon should win over cache");

    let _ = std::fs::remove_dir_all(&dir);
}

// =========================================================================
// CHECK 3: TilesetBuilder (compile-time check — builder is behind fetch)
// =========================================================================
// TilesetBuilder requires `fetch` feature (reqwest, async runtime). Since
// integration tests don't enable `fetch`, we verify that the meta/manifest
// construction logic is correct by doing a manual export_tileset + import
// roundtrip with the same parameters the builder would use.

#[test]
fn check3_builder_equivalent_roundtrip() {
    // Simulate what TilesetBuilder::package_hexon does:
    let region = "QA-Test";
    let bounds = [47.0, -123.0, 49.0, -121.0];
    let tiles: Vec<(String, Vec<u8>)> = vec![
        ("7/20/45".into(), vec![1u8; 100]),
        ("7/21/45".into(), vec![2u8; 100]),
    ];

    let manifest = HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: format!("tileset-{}", region.to_lowercase().replace(' ', "-")),
        hexon_type: HexonType::TerrainTileset,
        publisher_did: "did:key:z6MkQA".into(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: format!("Terrain Tileset — {region}"),
        description: Some(format!("QA test tileset ({} tiles)", tiles.len())),
        tags: vec!["terrain".into(), "tileset".into()],
        created_at: "2026-05-09T00:00:00Z".into(),
        updated_at: "2026-05-09T00:00:00Z".into(),
        source_peer_did: None,
        approx_size_bytes: Some(200),
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        signature: None,
    };

    let meta = TilesetMeta {
        bounds,
        min_zoom: 7,
        max_zoom: 7,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: false,
        tile_count: 2,
        satellite_tile_count: 0,
        region_name: region.into(),
        parent_tileset: None,
        chunk_index: None,
        native_scale: None,
        ground_sample_distance_m: None,
        crs: None,
        scale_bounds: None,
    };

    let archive = HexonArchive::export_tileset(manifest.clone(), &meta, &tiles, &[], None)
        .expect("export_tileset");

    // Import and verify
    let data = HexonArchive::import(&archive).expect("import");
    assert_eq!(data.manifest.hexon_id, "tileset-qa-test");
    assert!(matches!(
        data.manifest.hexon_type,
        HexonType::TerrainTileset
    ));

    let imported_meta = data.tileset_meta.expect("should have tileset_meta");
    assert_eq!(imported_meta.bounds, bounds);
    assert_eq!(imported_meta.min_zoom, 7);
    assert_eq!(imported_meta.max_zoom, 7);
    assert_eq!(imported_meta.tile_count, 2);
    assert_eq!(imported_meta.region_name, "QA-Test");

    assert_eq!(data.elevation_tiles.len(), 2);
    // Verify tile data integrity
    for (key, tdata) in &data.elevation_tiles {
        let original = tiles.iter().find(|(k, _)| k == key);
        assert!(original.is_some(), "unexpected tile key: {key}");
        assert_eq!(tdata, &original.unwrap().1);
    }
}

// =========================================================================
// CHECK 4: Region presets
// =========================================================================

#[test]
fn check4_all_7_presets_exist() {
    let presets: Vec<&RegionPreset> = vec![
        &NA_PACIFIC_NORTHWEST,
        &NA_ROCKIES,
        &NA_APPALACHIAN,
        &NA_GREAT_LAKES,
        &NA_SOUTHWEST,
        &NA_NORTHEAST,
        &NORTH_AMERICA_FULL,
    ];
    assert_eq!(presets.len(), 7, "expected exactly 7 presets");
}

#[test]
fn check4_presets_valid_wgs84() {
    let presets = [
        &NA_PACIFIC_NORTHWEST,
        &NA_ROCKIES,
        &NA_APPALACHIAN,
        &NA_GREAT_LAKES,
        &NA_SOUTHWEST,
        &NA_NORTHEAST,
        &NORTH_AMERICA_FULL,
    ];
    for p in presets {
        let [min_lat, min_lon, max_lat, max_lon] = p.bounds;
        assert!(min_lat >= -90.0, "{}: min_lat < -90", p.name);
        assert!(max_lat <= 90.0, "{}: max_lat > 90", p.name);
        assert!(min_lon >= -180.0, "{}: min_lon < -180", p.name);
        assert!(max_lon <= 180.0, "{}: max_lon > 180", p.name);
        assert!(min_lat < max_lat, "{}: min_lat >= max_lat", p.name);
        assert!(min_lon < max_lon, "{}: min_lon >= max_lon", p.name);

        let (min_z, max_z) = p.recommended_zoom;
        assert!(min_z > 0, "{}: min_zoom is 0", p.name);
        assert!(max_z <= 20, "{}: max_zoom > 20", p.name);
        assert!(min_z <= max_z, "{}: min > max zoom", p.name);
    }
}

#[test]
fn check4_pnw_covers_seattle_and_portland() {
    let [min_lat, min_lon, max_lat, max_lon] = NA_PACIFIC_NORTHWEST.bounds;

    // Seattle: 47.6, -122.3
    assert!(
        47.6 >= min_lat && 47.6 <= max_lat,
        "PNW doesn't cover Seattle lat"
    );
    assert!(
        -122.3 >= min_lon && -122.3 <= max_lon,
        "PNW doesn't cover Seattle lon"
    );

    // Portland: 45.5, -122.7
    assert!(
        45.5 >= min_lat && 45.5 <= max_lat,
        "PNW doesn't cover Portland lat"
    );
    assert!(
        -122.7 >= min_lon && -122.7 <= max_lon,
        "PNW doesn't cover Portland lon"
    );
}

// =========================================================================
// CHECK 5: ChunkIndex
// =========================================================================

#[test]
fn check5_chunk_index_sequential() {
    use fe_format::manifest::ChunkIndex;

    let bounds = [47.0, -123.0, 49.0, -121.0];
    let total_chunks = 3u32;
    let tileset_id = "qa-chunked";

    let mut archives = Vec::new();
    for seq in 0..total_chunks {
        let chunk_tiles = vec![(format!("5/{seq}/0"), vec![seq as u8; 16])];
        let meta = TilesetMeta {
            bounds,
            min_zoom: 5,
            max_zoom: 5,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: 1,
            satellite_tile_count: 0,
            region_name: "QA Chunk".into(),
            parent_tileset: None,
            chunk_index: Some(ChunkIndex {
                tileset_id: tileset_id.into(),
                chunk_seq: seq,
                total_chunks,
                chunk_bounds: bounds,
            }),
            native_scale: None,
            ground_sample_distance_m: None,
            crs: None,
            scale_bounds: None,
        };
        let manifest = make_manifest(&format!("{tileset_id}-chunk-{seq}"));
        let archive = HexonArchive::export_tileset(manifest, &meta, &chunk_tiles, &[], None)
            .expect("export chunk");
        archives.push(archive);
    }

    // Verify each chunk's metadata
    for (seq, archive_bytes) in archives.iter().enumerate() {
        let data = HexonArchive::import(archive_bytes).expect("import chunk");
        let meta = data.tileset_meta.expect("tileset_meta");
        let ci = meta.chunk_index.expect("chunk_index");

        assert_eq!(ci.tileset_id, tileset_id, "chunk {seq}: wrong tileset_id");
        assert_eq!(ci.chunk_seq, seq as u32, "chunk {seq}: wrong chunk_seq");
        assert_eq!(
            ci.total_chunks, total_chunks,
            "chunk {seq}: wrong total_chunks"
        );
        assert_eq!(ci.chunk_bounds, bounds, "chunk {seq}: wrong bounds");

        // Each chunk should have exactly 1 tile
        assert_eq!(
            data.elevation_tiles.len(),
            1,
            "chunk {seq}: wrong tile count"
        );
        assert_eq!(data.elevation_tiles[0].1, vec![seq as u8; 16]);
    }
}

// =========================================================================
// CHECK 6: Format compatibility roundtrip
// =========================================================================

#[test]
fn check6_full_roundtrip_elevation_and_satellite() {
    let elevation = vec![
        ("5/10/20".into(), vec![0xAAu8; 64]),
        ("6/20/40".into(), vec![0xBBu8; 128]),
    ];
    let satellite = vec![("5/10/20".into(), vec![0xCCu8; 256])];
    let meta = TilesetMeta {
        bounds: [40.0, -120.0, 50.0, -110.0],
        min_zoom: 5,
        max_zoom: 10,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::Terrarium,
        has_satellite: true,
        tile_count: 2,
        satellite_tile_count: 1,
        region_name: "Roundtrip Test".into(),
        parent_tileset: None,
        chunk_index: None,
        native_scale: None,
        ground_sample_distance_m: None,
        crs: None,
        scale_bounds: None,
    };

    let archive = HexonArchive::export_tileset(
        make_manifest("roundtrip"),
        &meta,
        &elevation,
        &satellite,
        None,
    )
    .expect("export");

    let data = HexonArchive::import(&archive).expect("import");

    // tileset_meta survives
    let imported_meta = data.tileset_meta.expect("tileset_meta");
    assert_eq!(imported_meta.bounds, meta.bounds);
    assert_eq!(imported_meta.min_zoom, 5);
    assert_eq!(imported_meta.max_zoom, 10);
    assert!(matches!(
        imported_meta.elevation_encoding,
        ElevationEncoding::Terrarium
    ));
    assert!(imported_meta.has_satellite);
    assert_eq!(imported_meta.tile_count, 2);
    assert_eq!(imported_meta.satellite_tile_count, 1);
    assert_eq!(imported_meta.region_name, "Roundtrip Test");

    // elevation_tiles survive
    assert_eq!(data.elevation_tiles.len(), 2);
    let elev_keys: Vec<&str> = data
        .elevation_tiles
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert!(elev_keys.contains(&"5/10/20"));
    assert!(elev_keys.contains(&"6/20/40"));

    // satellite_tiles survive
    assert_eq!(data.satellite_tiles.len(), 1);
    assert_eq!(data.satellite_tiles[0].0, "5/10/20");
    assert_eq!(data.satellite_tiles[0].1, vec![0xCCu8; 256]);
}

#[test]
fn check6_hexon_source_from_roundtripped_archive() {
    let tiles = make_tiles_in_bounds();
    let bytes = make_hexon_bytes(&tiles);

    // Import back through HexonTileSource
    let source = HexonTileSource::from_archive(&bytes).unwrap();

    // Re-export from source data
    let re_exported = HexonArchive::export_tileset(
        make_manifest("re-export"),
        &source.tileset_meta,
        &source
            .tiles
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>(),
        &source
            .satellite
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>(),
        None,
    )
    .expect("re-export");

    // Import the re-exported archive
    let source2 = HexonTileSource::from_archive(&re_exported).unwrap();
    assert_eq!(source2.bounds(), source.bounds());
    assert_eq!(source2.zoom_range(), source.zoom_range());
    assert_eq!(source2.tiles.len(), source.tiles.len());
}

// =========================================================================
// CHECK 7: No fe-database dependency
// =========================================================================

#[test]
fn check7_cargo_toml_no_database_dep() {
    let cargo_toml = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("read Cargo.toml");

    assert!(
        !cargo_toml.contains("fe-database"),
        "fe-terrain/Cargo.toml must NOT depend on fe-database"
    );
    assert!(
        !cargo_toml.contains("fe_database"),
        "fe-terrain/Cargo.toml must NOT depend on fe_database"
    );
}
