//! Integration tests for the sample-hexons builders — see `sample-hexons/README.md`.

#[path = "support/sample_hexons.rs"]
mod sample_hexons;

use fe_format::manifest::HexonType;
use fe_format::HexonArchive;
use fe_terrain::tiles::{HexonTileSource, TileCoord};

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_SOI: [u8; 2] = [0xFF, 0xD8];

#[test]
fn all_samples_build_and_import() {
    let root = sample_hexons::samples_root();
    let built = sample_hexons::build_all(&root).expect("build_all failed");
    assert_eq!(built.len(), 3);
    for (name, bytes) in built {
        assert!(!bytes.is_empty(), "{name} produced empty archive");
        HexonArchive::import(&bytes).unwrap_or_else(|e| panic!("{name} import failed: {e}"));
    }
}

#[test]
fn alpine_tileset_loads_via_hexon_tile_source() {
    let root = sample_hexons::samples_root();
    let bytes = sample_hexons::build_alpine_demo_terrain(&root).expect("alpine build failed");

    // Manifest fields survive round-trip.
    let data = HexonArchive::import(&bytes).expect("import failed");
    assert!(matches!(data.manifest.hexon_type, HexonType::Terrain));
    assert_eq!(data.manifest.version, "0.1.0");
    assert_eq!(data.manifest.publisher_did, "did:key:z6MkSampleAlpineGis");
    assert_eq!(data.elevation_tiles.len(), 12);
    assert_eq!(data.satellite_tiles.len(), 12);

    // Loads through the terrain-side consumer and serves tiles.
    let source = HexonTileSource::from_archive(&bytes).expect("from_archive failed");
    assert_eq!(source.tileset_meta.region_name, "Zurich Demo Region");
    assert_eq!(source.zoom_range(), (10, 12));

    let coord = TileCoord::from_lat_lon(47.3769, 8.5417, 10);
    assert!(source.covers(coord), "Zurich z10 tile not covered");
    let elevation = source.get_tile(coord).expect("missing elevation tile");
    assert_eq!(&elevation[..8], &PNG_MAGIC);
    let satellite = source
        .get_satellite_tile(coord)
        .expect("missing satellite tile");
    assert_eq!(&satellite[..2], &JPEG_SOI);

    // Every generated coordinate is servable.
    for (z, x, y) in sample_hexons::demo_tile_coords() {
        assert!(
            source.has_tile(TileCoord::new(x, y, z)),
            "missing tile {z}/{x}/{y}"
        );
    }
}

#[test]
fn morning_run_gpx_round_trips_bytes() {
    let root = sample_hexons::samples_root();
    let bytes = sample_hexons::build_morning_run_gpx(&root).expect("gpx build failed");
    let data = HexonArchive::import(&bytes).expect("import failed");

    assert!(matches!(data.manifest.hexon_type, HexonType::GpxCollection));
    assert_eq!(data.manifest.version, "0.1.0");
    assert_eq!(data.manifest.publisher_did, "did:key:z6MkSampleTrailRunner");
    assert!(data.terrain_config.is_some());

    let source_gpx = std::fs::read(
        root.join(sample_hexons::MORNING_RUN)
            .join("terrain/tracks/morning_run.gpx"),
    )
    .expect("failed to read source GPX");
    assert_eq!(data.gpx_files.len(), 1);
    assert_eq!(data.gpx_files[0].0, "morning_run.gpx");
    assert_eq!(data.gpx_files[0].1, source_gpx, "GPX bytes not lossless");

    assert_eq!(data.entries.len(), 1);
    assert_eq!(
        data.entries[0].asset_hash,
        blake3::hash(&source_gpx).to_hex().to_string()
    );
}

#[test]
fn lakeside_scene_round_trips_nodes_and_schema() {
    let root = sample_hexons::samples_root();
    let bytes = sample_hexons::build_lakeside_scene(&root).expect("scene build failed");
    let data = HexonArchive::import(&bytes).expect("import failed");

    assert!(matches!(data.manifest.hexon_type, HexonType::Scene));
    assert_eq!(data.manifest.version, "0.1.0");
    assert_eq!(data.manifest.publisher_did, "did:key:z6MkSampleSceneAuthor");

    assert_eq!(data.nodes.len(), 4);
    let anchor = data
        .nodes
        .iter()
        .find(|n| n.name == "Alpine Terrain Anchor")
        .expect("terrain anchor node missing");
    let hexon_ref = anchor.properties.as_ref().unwrap()["hexon_ref"]
        .as_str()
        .expect("hexon_ref property missing");
    assert!(hexon_ref.contains("alpine-demo-terrain"));
    assert_eq!(anchor.node_log.len(), 2);

    let waypoints = data
        .nodes
        .iter()
        .filter(|n| n.properties.as_ref().and_then(|p| p["gpx_type"].as_str()) == Some("waypoint"))
        .count();
    assert_eq!(waypoints, 2);

    assert!(data.field_defs.iter().any(|f| f.key == "hexon_ref"));
    assert!(data.field_defs.iter().any(|f| f.key == "gpx_type"));
    assert_eq!(data.schema.field_defs.len(), data.field_defs.len());
}
