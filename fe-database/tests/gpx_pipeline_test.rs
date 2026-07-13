//! Integration test: `gpx_bridge`'s exact command sequence (`CreateNode` +
//! `SetNodeProperty`) round-trips through the real embedded DB and reads back
//! via the same SELECT shape `fe-api/src/gis.rs`'s `list_gis_tracks` /
//! `list_gis_nodes` endpoints use. See `fractalengine/src/AGENTS.md` §gpx for
//! the full property-shape contract. Harness mirrors `gis_data_test.rs`.

use fe_database::BlobStoreHandle;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn mock_blob_store() -> BlobStoreHandle {
    Arc::new(MockBlobStore::new())
}

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// Mirrors `fe-api/src/gis.rs`'s `list_gis_tracks` SELECT verbatim.
const TRACKS_SQL: &str = "SELECT node_id, display_name, position, elevation, properties, created_at \
    FROM node WHERE petal_id = $pid AND properties.gpx_type = 'track' ORDER BY created_at ASC";

/// Same shape, filtered for waypoints instead (mirrors the `gis/nodes` read pattern).
const WAYPOINTS_SQL: &str = "SELECT node_id, display_name, position, elevation, properties, created_at \
    FROM node WHERE petal_id = $pid AND properties.gpx_type = 'waypoint' ORDER BY created_at ASC";

/// An isolated DB thread backed by a unique temp directory — no shared
/// state or file-lock contention with other tests (each gets its own path).
struct TestDb {
    cmd_tx: crossbeam::channel::Sender<DbCommand>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    _tmp_dir: tempfile::TempDir,
}

fn spawn_test_db() -> TestDb {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir for test DB");
    let db_path = tmp_dir.path().join("gpx_pipeline_test.db").to_string_lossy().to_string();

    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(256);
    let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(256);

    let _handle = fe_database::spawn_db_thread_with_sync(
        cmd_rx,
        res_tx,
        mock_blob_store(),
        None,
        None,
        None,
        None,
        Some(db_path),
    )
    .expect("gpx pipeline test DB init failed");

    let started = res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("gpx pipeline test DB did not start within 30s");
    assert!(matches!(started, DbResult::Started), "expected DbResult::Started, got {started:?}");

    TestDb { cmd_tx, res_rx, _tmp_dir: tmp_dir }
}

/// Build the minimal verse -> fractal -> petal hierarchy and return the petal ID.
fn seed_hierarchy(db: &TestDb) -> String {
    db.cmd_tx
        .send(DbCommand::CreateVerse { name: "gpx-test-verse".to_string() })
        .unwrap();
    let verse_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateVerse result") {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreateFractal { verse_id, name: "gpx-test-fractal".to_string() })
        .unwrap();
    let fractal_id = match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateFractal result") {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };

    db.cmd_tx
        .send(DbCommand::CreatePetal { fractal_id, name: "gpx-test-petal".to_string() })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreatePetal result") {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    }
}

/// Create a node in `petal_id` at local `position` ([x, elevation, z]) and
/// return its node ID — mirrors `gpx_bridge::drain_gpx_ops`'s `CreateNode` call.
fn create_node(db: &TestDb, petal_id: &str, name: &str, position: [f32; 3]) -> String {
    db.cmd_tx
        .send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: name.to_string(),
            position,
            correlation_id: None,
        })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("CreateNode result") {
        DbResult::NodeCreated { id, .. } => id,
        other => panic!("expected NodeCreated, got {other:?}"),
    }
}

/// Set a custom property on a node via the real `SetNodeProperty` command —
/// mirrors `gpx_bridge::advance_gpx_imports`'s per-key property sets.
fn set_property(db: &TestDb, node_id: &str, key: &str, value: serde_json::Value) {
    db.cmd_tx
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value,
        })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("SetNodeProperty result") {
        DbResult::NodePropertySet { .. } => {}
        other => panic!("expected NodePropertySet, got {other:?}"),
    }
}

/// Run a raw SELECT through the real `DbCommand::RawQuery` gateway.
fn run_query(db: &TestDb, sql: &str, vars: HashMap<String, serde_json::Value>) -> Vec<serde_json::Value> {
    db.cmd_tx
        .send(DbCommand::RawQuery { sql: sql.to_string(), vars })
        .unwrap();
    match db.res_rx.recv_timeout(CMD_TIMEOUT).expect("RawQuery result") {
        DbResult::QueryResult { data } => data,
        other => panic!("expected QueryResult, got {other:?}"),
    }
}

fn pid_vars(petal_id: &str) -> HashMap<String, serde_json::Value> {
    let mut vars = HashMap::new();
    vars.insert("pid".to_string(), serde_json::json!(petal_id));
    vars
}

/// FR-4 handler-level round trip: the exact `CreateNode` + `SetNodeProperty`
/// sequence `gpx_bridge.rs` sends for a track + one waypoint "child" persists
/// through the real embedded DB and is readable back via the same SELECT
/// shape the `gis/tracks` HTTP endpoint uses.
#[test]
fn gpx_track_and_waypoint_round_trip_through_gis_tracks_select() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);

    // --- Track node: mirrors gpx_bridge's build_track_draft + advance_gpx_imports ---
    let track_id = create_node(&db, &petal_id, "Test Hike", [0.0, 0.0, 0.0]);
    set_property(&db, &track_id, "gpx_type", serde_json::json!("track"));
    set_property(&db, &track_id, "total_distance_m", serde_json::json!(1234.5));
    set_property(&db, &track_id, "elevation_gain_m", serde_json::json!(56.0));
    set_property(&db, &track_id, "elevation_loss_m", serde_json::json!(12.0));
    set_property(&db, &track_id, "duration_s", serde_json::json!(3600.0));
    set_property(&db, &track_id, "avg_speed_kmh", serde_json::json!(5.5));
    set_property(&db, &track_id, "max_speed_kmh", serde_json::json!(9.0));
    set_property(
        &db,
        &track_id,
        "bounding_box",
        serde_json::json!({
            "min_lat": 45.0,
            "max_lat": 45.01,
            "min_lon": -122.01,
            "max_lon": -122.0,
        }),
    );

    // --- Waypoint "child": mirrors gpx_bridge's build_waypoint_drafts + advance_gpx_imports ---
    let wp_id = create_node(&db, &petal_id, "Camp", [5.0, 1.0, 5.0]);
    set_property(&db, &wp_id, "gpx_type", serde_json::json!("waypoint"));
    set_property(&db, &wp_id, "gis.annotation.title", serde_json::json!("Camp"));
    set_property(&db, &wp_id, "gpx_track_id", serde_json::json!(track_id.clone()));

    // A non-GPX control node must not leak into either query.
    let _other = create_node(&db, &petal_id, "Unrelated", [0.0, 0.0, 0.0]);

    let track_rows = run_query(&db, TRACKS_SQL, pid_vars(&petal_id));
    assert_eq!(track_rows.len(), 1, "exactly one track row expected: {track_rows:?}");
    let track_row = &track_rows[0];
    assert_eq!(track_row["node_id"], serde_json::json!(track_id));
    assert_eq!(track_row["display_name"], serde_json::json!("Test Hike"));
    assert_eq!(track_row["properties"]["gpx_type"], serde_json::json!("track"));
    assert_eq!(track_row["properties"]["total_distance_m"], serde_json::json!(1234.5));
    assert_eq!(track_row["properties"]["elevation_gain_m"], serde_json::json!(56.0));
    assert_eq!(track_row["properties"]["duration_s"], serde_json::json!(3600.0));
    assert_eq!(track_row["properties"]["bounding_box"]["min_lat"], serde_json::json!(45.0));
    assert_eq!(track_row["properties"]["bounding_box"]["max_lon"], serde_json::json!(-122.0));

    let wp_rows = run_query(&db, WAYPOINTS_SQL, pid_vars(&petal_id));
    assert_eq!(wp_rows.len(), 1, "exactly one waypoint row expected: {wp_rows:?}");
    let wp_row = &wp_rows[0];
    assert_eq!(wp_row["node_id"], serde_json::json!(wp_id));
    assert_eq!(wp_row["display_name"], serde_json::json!("Camp"));
    assert_eq!(wp_row["properties"]["gis.annotation.title"], serde_json::json!("Camp"));
    assert_eq!(
        wp_row["properties"]["gpx_track_id"],
        serde_json::json!(track_id),
        "waypoint must retain a linkage back to its track node — approximates a \
         parent/child relation via a flat property since node.parent_node_id does \
         not exist on the schema (see fractalengine/src/AGENTS.md §gpx)"
    );
}

/// A waypoint-only import (no track in the source file) still persists and
/// is readable, just without a `gpx_track_id` — mirrors gpx_bridge's
/// standalone-waypoint fallback path.
#[test]
fn standalone_waypoint_without_track_persists_and_has_no_track_link() {
    let db = spawn_test_db();
    let petal_id = seed_hierarchy(&db);

    let wp_id = create_node(&db, &petal_id, "Lone Marker", [1.0, 0.0, 1.0]);
    set_property(&db, &wp_id, "gpx_type", serde_json::json!("waypoint"));
    set_property(&db, &wp_id, "gis.annotation.title", serde_json::json!("Lone Marker"));

    let wp_rows = run_query(&db, WAYPOINTS_SQL, pid_vars(&petal_id));
    assert_eq!(wp_rows.len(), 1);
    let wp_row = &wp_rows[0];
    assert_eq!(wp_row["properties"]["gis.annotation.title"], serde_json::json!("Lone Marker"));
    assert!(
        wp_row["properties"].get("gpx_track_id").is_none(),
        "standalone waypoint must not carry a gpx_track_id: {wp_row:?}"
    );

    let track_rows = run_query(&db, TRACKS_SQL, pid_vars(&petal_id));
    assert!(track_rows.is_empty(), "no track node should exist: {track_rows:?}");
}
