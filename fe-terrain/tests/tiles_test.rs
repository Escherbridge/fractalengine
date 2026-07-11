use fe_terrain::tiles::source::TileCoord;
use fe_terrain::tiles::elevation::{TerrainRgbDecoder, TerrariumDecoder, ElevationDecoder};
use fe_terrain::tiles::cache::DiskTileCache;
use fe_terrain::tiles::lod::TileLodManager;

#[test]
fn tile_coord_from_lat_lon() {
    // Known test: zoom 0 has 1 tile, zoom 1 has 4 tiles
    let coord = TileCoord::from_lat_lon(0.0, 0.0, 0);
    assert_eq!(coord.zoom, 0);
    assert_eq!(coord.x, 0);
    assert_eq!(coord.y, 0);

    // New York at zoom 10
    let nyc = TileCoord::from_lat_lon(40.7128, -74.0060, 10);
    assert_eq!(nyc.zoom, 10);
    // NYC should be roughly at x=301, y=384 at zoom 10
    assert!(nyc.x > 290 && nyc.x < 310, "NYC x={}", nyc.x);
    assert!(nyc.y > 370 && nyc.y < 400, "NYC y={}", nyc.y);
}

#[test]
fn tile_coord_roundtrip() {
    // Convert lat/lon to tile coord and back — should be in the same tile
    let lat = 47.6062;
    let lon = -122.3321;
    let zoom = 14;

    let coord = TileCoord::from_lat_lon(lat, lon, zoom);
    let (tile_lat, tile_lon) = coord.to_lat_lon();

    // The returned lat/lon is the NW corner of the tile, so it should be close
    assert!((tile_lat - lat).abs() < 0.02, "tile lat: {}", tile_lat);
    assert!((tile_lon - lon).abs() < 0.02, "tile lon: {}", tile_lon);
}

#[test]
fn tile_coord_cache_key() {
    let coord = TileCoord::new(42, 99, 15);
    assert_eq!(coord.cache_key(), "15/42/99");
}

#[test]
fn tiles_at_zoom() {
    assert_eq!(TileCoord::tiles_at_zoom(0), 1);
    assert_eq!(TileCoord::tiles_at_zoom(1), 2);
    assert_eq!(TileCoord::tiles_at_zoom(10), 1024);
    assert_eq!(TileCoord::tiles_at_zoom(20), 1048576);
}

#[test]
fn terrain_rgb_decoder() {
    let decoder = TerrainRgbDecoder;

    // Sea level: R=1, G=134, B=160 -> h = -10000 + (1*65536 + 134*256 + 160)*0.1 = 0.0
    let pixels = vec![1, 134, 160]; // RGB for 0m elevation
    let elevations = decoder.decode(&pixels, 1, 1);
    assert_eq!(elevations.len(), 1);
    assert!((elevations[0] - 0.0).abs() < 0.2, "sea level should be ~0, got {}", elevations[0]);

    // Known value: R=0, G=0, B=0 -> h = -10000 + 0 = -10000
    let pixels_deep = vec![0, 0, 0];
    let elev_deep = decoder.decode(&pixels_deep, 1, 1);
    assert!((elev_deep[0] - (-10000.0)).abs() < 0.1);

    // Multiple pixels
    let pixels_multi = vec![1, 134, 160, 2, 0, 0]; // 2 pixels
    let elev_multi = decoder.decode(&pixels_multi, 2, 1);
    assert_eq!(elev_multi.len(), 2);
}

#[test]
fn terrarium_decoder() {
    let decoder = TerrariumDecoder;

    // Sea level: R=128, G=0, B=0 -> h = 128*256 + 0 + 0/256 - 32768 = 0
    let pixels = vec![128, 0, 0];
    let elevations = decoder.decode(&pixels, 1, 1);
    assert_eq!(elevations.len(), 1);
    assert!((elevations[0] - 0.0).abs() < 0.1, "sea level should be ~0, got {}", elevations[0]);

    // R=0, G=0, B=0 -> h = -32768
    let pixels_deep = vec![0, 0, 0];
    let elev_deep = decoder.decode(&pixels_deep, 1, 1);
    assert!((elev_deep[0] - (-32768.0)).abs() < 0.1);
}

#[test]
fn terrain_rgb_decoder_rgba() {
    let decoder = TerrainRgbDecoder;

    // RGBA pixels (4 bytes per pixel)
    let pixels = vec![1, 134, 160, 255, 0, 0, 0, 255]; // 2 RGBA pixels
    let elevations = decoder.decode(&pixels, 2, 1);
    assert_eq!(elevations.len(), 2);
    assert!((elevations[0] - 0.0).abs() < 0.2);
    assert!((elevations[1] - (-10000.0)).abs() < 0.1);
}

#[test]
fn disk_tile_cache_put_get() {
    let dir = std::env::temp_dir().join("fe_terrain_test_cache");
    let _ = std::fs::remove_dir_all(&dir);

    let cache = DiskTileCache::new(&dir, 1024 * 1024);

    // Put a tile
    let data = b"fake tile data";
    cache.put("test-source", "14/42/99", data).expect("put failed");

    // Get it back
    let retrieved = cache.get("test-source", "14/42/99");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), data);

    // Missing tile returns None
    let missing = cache.get("test-source", "14/42/100");
    assert!(missing.is_none());

    // Clean up
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disk_tile_cache_integrity_check() {
    let dir = std::env::temp_dir().join("fe_terrain_test_cache_integrity");
    let _ = std::fs::remove_dir_all(&dir);

    let cache = DiskTileCache::new(&dir, 1024 * 1024);
    let data = b"tile data";
    cache.put("src", "1/0/0", data).expect("put failed");

    // Corrupt the tile file
    let tile_path = dir.join("src").join("1/0/0.tile");
    std::fs::write(&tile_path, b"corrupted!").unwrap();

    // Get should return None (integrity check fails)
    let result = cache.get("src", "1/0/0");
    assert!(result.is_none(), "corrupted tile should not be returned");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lod_manager_basic() {
    let lod = TileLodManager::new(10, 14);

    // Compute tiles centered on Seattle
    let requests = lod.compute_tile_requests(47.6062, -122.3321, 0.1);

    // Should have some tile requests
    assert!(!requests.is_empty(), "should have tile requests");

    // All tiles should be within zoom range
    for req in &requests {
        assert!(req.coord.zoom >= 10, "zoom {} < min 10", req.coord.zoom);
        assert!(req.coord.zoom <= 14, "zoom {} > max 14", req.coord.zoom);
    }

    // Should be sorted by distance
    for pair in requests.windows(2) {
        assert!(
            pair[0].distance <= pair[1].distance + 1e-10,
            "not sorted: {} > {}",
            pair[0].distance,
            pair[1].distance
        );
    }
}

#[test]
fn config_serde_roundtrip() {
    use fe_terrain::config::{TerrainConfig, ElevationSourceKind, LayerConfig};
    use fe_terrain::projection::Projection;

    let config = TerrainConfig {
        enabled: true,
        origin: Projection::new(47.6, -122.3, 50.0),
        tile_source_url: "https://tiles.example.com/{z}/{x}/{y}.png".into(),
        elevation_source: ElevationSourceKind::TerrainRgb,
        elevation_api_key_env: Some("MAPBOX_TOKEN".into()),
        max_zoom: 16,
        min_zoom: 8,
        cache_dir: "/tmp/tiles".into(),
        layers: vec![LayerConfig {
            name: "satellite".into(),
            visible: true,
            opacity: Some(0.8),
            source_url: Some("https://sat.example.com/{z}/{x}/{y}.jpg".into()),
        }],
        tileset_hexon_uris: vec![],
        tile_source_mode: fe_terrain::config::TileSourceMode::Hybrid,
        world_scale: 0.01,
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: TerrainConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.enabled, true);
    assert_eq!(deserialized.max_zoom, 16);
    assert_eq!(deserialized.min_zoom, 8);
    assert_eq!(deserialized.elevation_source, ElevationSourceKind::TerrainRgb);
    assert_eq!(deserialized.layers.len(), 1);
    assert_eq!(deserialized.layers[0].name, "satellite");
    assert!((deserialized.origin.origin_lat - 47.6).abs() < 1e-6);
    assert_eq!(deserialized.world_scale, 0.01);
}

#[test]
fn petal_terrain_binding_serde() {
    use fe_terrain::config::{PetalTerrainBinding, TerrainConfig};

    let binding = PetalTerrainBinding {
        petal_id: "petal-123".into(),
        config: TerrainConfig::default(),
    };

    let json = serde_json::to_string(&binding).unwrap();
    let deserialized: PetalTerrainBinding = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.petal_id, "petal-123");
    assert_eq!(deserialized.config.enabled, false);
}
