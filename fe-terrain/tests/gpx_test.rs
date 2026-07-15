use fe_terrain::gpx::{compute_stats, gpx_to_scene_commands, parse_gpx_bytes, scene_nodes_to_gpx};
use fe_terrain::projection::Projection;
use fe_terrain::ExportNode;

const SAMPLE_GPX: &[u8] = include_bytes!("fixtures/sample.gpx");

#[test]
fn parse_sample_gpx() {
    let data = parse_gpx_bytes(SAMPLE_GPX).expect("parse failed");
    assert_eq!(data.tracks.len(), 1);
    assert_eq!(data.tracks[0].name.as_deref(), Some("Morning Run"));
    assert_eq!(data.tracks[0].segments.len(), 1);
    assert_eq!(data.tracks[0].segments[0].points.len(), 5);
    assert_eq!(data.waypoints.len(), 2);
    assert_eq!(data.waypoints[0].name.as_deref(), Some("Space Needle"));
    assert_eq!(data.waypoints[1].name.as_deref(), Some("Kerry Park"));

    // Verify coordinates
    let first_pt = &data.tracks[0].segments[0].points[0];
    assert!((first_pt.lat - 47.6062).abs() < 1e-6);
    assert!((first_pt.lon - (-122.3321)).abs() < 1e-6);
    assert_eq!(first_pt.ele, Some(56.0));

    // Verify metadata
    assert!(data.metadata.is_some());
    let meta = data.metadata.as_ref().unwrap();
    assert_eq!(meta.name.as_deref(), Some("Test Trail"));
    assert_eq!(
        meta.description.as_deref(),
        Some("A sample GPX for unit tests")
    );
}

#[test]
fn compute_track_stats() {
    let data = parse_gpx_bytes(SAMPLE_GPX).expect("parse failed");
    let stats = compute_stats(&data);

    // 5 points along a ~1km trail
    assert!(stats.total_distance_m > 500.0, "distance should be > 500m");
    assert!(stats.total_distance_m < 5000.0, "distance should be < 5km");

    // Elevation: gain from 56->62 (+6), 58->70 (+12) = ~18m gain
    assert!(stats.elevation_gain_m > 10.0);
    // Elevation loss: 62->58 (-4), 70->65 (-5) = ~9m
    assert!(stats.elevation_loss_m > 5.0);

    // Duration: 20 minutes
    assert_eq!(stats.duration, Some(1200.0));

    // Avg speed should be reasonable (> 1 km/h, < 20 km/h for a run)
    assert!(stats.avg_speed_kmh.unwrap() > 1.0);
    assert!(stats.avg_speed_kmh.unwrap() < 20.0);

    // Bounding box
    assert!((stats.bounding_box.min_lat - 47.6062).abs() < 1e-4);
    assert!((stats.bounding_box.max_lat - 47.6150).abs() < 1e-4);
    assert!(stats.bounding_box.min_ele.is_some());
    assert!(stats.bounding_box.max_ele.is_some());
}

#[test]
fn convert_gpx_to_commands() {
    let data = parse_gpx_bytes(SAMPLE_GPX).expect("parse failed");
    let projection = Projection::new(47.61, -122.34, 50.0);
    let commands = gpx_to_scene_commands(&data, "test-petal", &projection);

    // 1 track + 1 segment + 5 trackpoints + 2 waypoints = 9 commands
    assert_eq!(commands.len(), 9);

    // First command should be the track
    let track_cmd = &commands[0];
    assert!(track_cmd.parent_node_id.is_none());
    let props = track_cmd.properties.as_ref().unwrap();
    assert_eq!(props["gpx_type"], "track");

    // Second should be the segment
    let seg_cmd = &commands[1];
    assert!(seg_cmd.parent_node_id.is_some());
    let seg_props = seg_cmd.properties.as_ref().unwrap();
    assert_eq!(seg_props["gpx_type"], "segment");

    // Points 3-7 should be trackpoints
    for cmd in &commands[2..7] {
        let pt_props = cmd.properties.as_ref().unwrap();
        assert_eq!(pt_props["gpx_type"], "trackpoint");
    }

    // Last 2 should be waypoints
    for cmd in &commands[7..9] {
        let wp_props = cmd.properties.as_ref().unwrap();
        assert_eq!(wp_props["gpx_type"], "waypoint");
    }
}

#[test]
fn export_roundtrip_coordinates() {
    let data = parse_gpx_bytes(SAMPLE_GPX).expect("parse failed");
    let projection = Projection::new(47.61, -122.34, 50.0);
    let commands = gpx_to_scene_commands(&data, "test-petal", &projection);

    // Build ExportNode tree from commands for the export function
    let export_nodes = build_export_tree(&commands);

    let gpx_xml = scene_nodes_to_gpx(&export_nodes, &projection);

    // Verify it's valid GPX by re-parsing
    let reparsed = parse_gpx_bytes(gpx_xml.as_bytes()).expect("re-parse failed");
    assert_eq!(reparsed.tracks.len(), 1);
    assert_eq!(reparsed.tracks[0].segments[0].points.len(), 5);
    assert_eq!(reparsed.waypoints.len(), 2);

    // Verify coordinate precision (7 decimal places)
    let orig_wp = &data.waypoints[0];
    let re_wp = &reparsed.waypoints[0];
    assert!(
        (orig_wp.lat - re_wp.lat).abs() < 1e-7,
        "lat mismatch: {} vs {}",
        orig_wp.lat,
        re_wp.lat
    );
    assert!(
        (orig_wp.lon - re_wp.lon).abs() < 1e-7,
        "lon mismatch: {} vs {}",
        orig_wp.lon,
        re_wp.lon
    );

    // Verify trackpoint coordinates preserved
    let orig_pt = &data.tracks[0].segments[0].points[0];
    let re_pt = &reparsed.tracks[0].segments[0].points[0];
    assert!(
        (orig_pt.lat - re_pt.lat).abs() < 1e-7,
        "trackpoint lat mismatch"
    );
    assert!(
        (orig_pt.lon - re_pt.lon).abs() < 1e-7,
        "trackpoint lon mismatch"
    );
}

#[test]
fn projection_inverse() {
    let projection = Projection::new(47.6062, -122.3321, 56.0);

    // Project a nearby point
    let local = projection
        .wgs84_to_local(47.6080, -122.3350, 62.0)
        .expect("projection failed");
    assert!(local[0].abs() < 10000.0, "x should be reasonable");
    assert!((local[1] - 6.0).abs() < 0.01, "y should be elevation diff");

    // Inverse should return original coords
    let (lat, lon, ele) = projection.local_to_wgs84(local[0], local[1], local[2]);
    assert!(
        (lat - 47.6080).abs() < 1e-7,
        "inverse lat: {} vs 47.6080",
        lat
    );
    assert!(
        (lon - (-122.3350)).abs() < 1e-7,
        "inverse lon: {} vs -122.3350",
        lon
    );
    assert!((ele - 62.0).abs() < 0.01, "inverse ele: {} vs 62.0", ele);
}

#[test]
fn projection_inverse_100km() {
    // Test round-trip at ~100km distance from origin
    let projection = Projection::new(47.0, -122.0, 0.0);

    // ~100km north (about 0.9 degrees latitude)
    let local = projection
        .wgs84_to_local(47.9, -122.0, 100.0)
        .expect("projection failed");
    let (lat, lon, ele) = projection.local_to_wgs84(local[0], local[1], local[2]);

    // Within 1cm at 100km: 1cm ≈ 9e-8 degrees of latitude
    assert!(
        (lat - 47.9).abs() < 1e-6,
        "100km inverse lat error: {} (diff {})",
        lat,
        (lat - 47.9).abs()
    );
    assert!(
        (lon - (-122.0)).abs() < 1e-6,
        "100km inverse lon error: {} (diff {})",
        lon,
        (lon - (-122.0)).abs()
    );
    assert!(
        (ele - 100.0).abs() < 0.001,
        "100km inverse ele error: {}",
        ele
    );
}

#[test]
fn projection_rejects_invalid_coordinates() {
    let projection = Projection::new(47.0, -122.0, 0.0);

    // Out-of-range latitude
    let result = projection.wgs84_to_local(91.0, -122.0, 0.0);
    assert!(result.is_err(), "lat 91 should be rejected");
    assert!(
        result.unwrap_err().to_string().contains("latitude"),
        "error should mention latitude"
    );

    // Out-of-range latitude negative
    let result = projection.wgs84_to_local(-91.0, -122.0, 0.0);
    assert!(result.is_err(), "lat -91 should be rejected");

    // Out-of-range longitude
    let result = projection.wgs84_to_local(47.0, 181.0, 0.0);
    assert!(result.is_err(), "lon 181 should be rejected");
    assert!(
        result.unwrap_err().to_string().contains("longitude"),
        "error should mention longitude"
    );

    // Out-of-range longitude negative
    let result = projection.wgs84_to_local(47.0, -181.0, 0.0);
    assert!(result.is_err(), "lon -181 should be rejected");

    // Boundary values should work
    assert!(projection.wgs84_to_local(90.0, 180.0, 0.0).is_ok());
    assert!(projection.wgs84_to_local(-90.0, -180.0, 0.0).is_ok());
    assert!(projection.wgs84_to_local(0.0, 0.0, 0.0).is_ok());
}

#[test]
fn extension_data_survives_in_properties() {
    // Extensions (hr/cad/power) aren't parsed by the gpx crate,
    // but they survive when stored in node properties and exported to GPX XML.
    let projection = Projection::new(47.61, -122.34, 50.0);

    // Simulate a trackpoint node with extension data in properties
    let mut seg_node = ExportNode {
        node_id: "seg-0".into(),
        name: "Segment 1".into(),
        position: [0.0, 0.0, 0.0],
        properties: Some(serde_json::json!({"gpx_type": "segment"})),
        children: vec![],
    };

    seg_node.children.push(ExportNode {
        node_id: "pt-0".into(),
        name: "Point 1".into(),
        position: [0.0, 6.0, 0.0],
        properties: Some(serde_json::json!({
            "gpx_type": "trackpoint",
            "lat": 47.6080,
            "lon": -122.3350,
            "ele": 62.0,
            "hr": 155,
            "cad": 85,
            "power": 250,
        })),
        children: vec![],
    });

    let track_node = ExportNode {
        node_id: "track-0".into(),
        name: "Test Track".into(),
        position: [0.0, 0.0, 0.0],
        properties: Some(serde_json::json!({"gpx_type": "track"})),
        children: vec![seg_node],
    };

    let gpx_xml = scene_nodes_to_gpx(&[track_node], &projection);

    // Verify GPX XML contains Garmin extensions
    assert!(
        gpx_xml.contains("gpxtpx:TrackPointExtension"),
        "GPX should contain TrackPointExtension"
    );
    assert!(
        gpx_xml.contains("<gpxtpx:hr>155</gpxtpx:hr>"),
        "GPX should contain hr=155"
    );
    assert!(
        gpx_xml.contains("<gpxtpx:cad>85</gpxtpx:cad>"),
        "GPX should contain cad=85"
    );
    // power stored in its own gpxtpx:power element
    assert!(
        gpx_xml.contains("<gpxtpx:power>250</gpxtpx:power>"),
        "GPX should contain power=250"
    );
    assert!(
        gpx_xml.contains("xmlns:gpxtpx"),
        "GPX root should declare gpxtpx namespace"
    );
}

#[test]
fn hexon_terrain_roundtrip() {
    use fe_terrain::format::{TerrainConfig, TerrainFormatExt};

    let config = TerrainConfig {
        crs: "EPSG:4326".into(),
        tile_size: 256,
        elevation_source: Some("srtm".into()),
        projection_origin: None,
    };

    let gpx_data = SAMPLE_GPX.to_vec();
    let geojson_data = b"{\"type\":\"FeatureCollection\",\"features\":[]}".to_vec();

    let manifest = fe_format::HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: "terrain-test".into(),
        hexon_type: fe_format::HexonType::Terrain,
        publisher_did: "did:key:z6Mktest".into(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: "Test Terrain".into(),
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
    };

    let gpx_files = vec![("trail.gpx".to_string(), gpx_data.clone())];
    let overlays = vec![("overlay.geojson".to_string(), geojson_data.clone())];

    // Export
    let zip_bytes =
        fe_format::HexonArchive::export_terrain_data(&config, &gpx_files, &overlays, manifest)
            .expect("terrain export failed");

    // Import
    let bundle = fe_format::HexonArchive::import_terrain_data(&zip_bytes)
        .expect("terrain import failed")
        .expect("no terrain data found");

    assert_eq!(bundle.config.crs, "EPSG:4326");
    assert_eq!(bundle.config.tile_size, 256);
    assert_eq!(bundle.config.elevation_source.as_deref(), Some("srtm"));
    assert_eq!(bundle.gpx_files.len(), 1);
    assert_eq!(bundle.gpx_files[0].0, "trail.gpx");
    assert_eq!(bundle.gpx_files[0].1, gpx_data);
    assert_eq!(bundle.geojson_overlays.len(), 1);
    assert_eq!(bundle.geojson_overlays[0].0, "overlay.geojson");
    assert_eq!(bundle.geojson_overlays[0].1, geojson_data);

    // Verify the embedded GPX can still be parsed
    let reparsed = parse_gpx_bytes(&bundle.gpx_files[0].1).expect("GPX re-parse failed");
    assert_eq!(reparsed.tracks.len(), 1);
    assert_eq!(reparsed.waypoints.len(), 2);
}

// --- Helpers ---

fn build_export_tree(commands: &[fe_terrain::DbCommand]) -> Vec<ExportNode> {
    let mut export_nodes: Vec<ExportNode> = Vec::new();

    // Track
    let track_cmd = &commands[0];
    let mut track_node = ExportNode {
        node_id: "track-0".into(),
        name: track_cmd.name.clone(),
        position: track_cmd.position,
        properties: track_cmd.properties.clone(),
        children: vec![],
    };

    // Segment
    let seg_cmd = &commands[1];
    let mut seg_node = ExportNode {
        node_id: "seg-0".into(),
        name: seg_cmd.name.clone(),
        position: seg_cmd.position,
        properties: seg_cmd.properties.clone(),
        children: vec![],
    };

    // Trackpoints
    for (i, pt_cmd) in commands[2..7].iter().enumerate() {
        seg_node.children.push(ExportNode {
            node_id: format!("pt-{}", i),
            name: pt_cmd.name.clone(),
            position: pt_cmd.position,
            properties: pt_cmd.properties.clone(),
            children: vec![],
        });
    }
    track_node.children.push(seg_node);
    export_nodes.push(track_node);

    // Waypoints
    for (i, wp_cmd) in commands[7..9].iter().enumerate() {
        export_nodes.push(ExportNode {
            node_id: format!("wp-{}", i),
            name: wp_cmd.name.clone(),
            position: wp_cmd.position,
            properties: wp_cmd.properties.clone(),
            children: vec![],
        });
    }

    export_nodes
}
