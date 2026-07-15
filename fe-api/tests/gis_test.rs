//! Router-level tests for the petal GIS read endpoints (`fe-api/src/gis.rs`).
//!
//! Mirrors the in-memory SurrealDB + `ApiState` idiom used by the inline tests
//! in `fe-api/src/assets.rs`: seed a verse/fractal/petal/node chain into an
//! embedded SurrealKV `Mem` engine, then call the handlers directly.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Extension;

use fe_api::gis::{
    bbox_ll_to_local, list_gis_nodes, list_gis_tracks, parse_bbox, within_radius, Bbox,
    GisNodesQuery,
};
use fe_api::server::ApiState;
use fe_identity::api_token::ApiClaims;
use fe_terrain::projection::Projection;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

async fn setup_test_db() -> Db {
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db)
        .await
        .expect("apply schema");
    db
}

fn test_claims(scope: &str, role: &str) -> ApiClaims {
    ApiClaims {
        sub: "did:key:z6MkUser".to_string(),
        scope: scope.to_string(),
        max_role: role.to_string(),
        token_type: "api".to_string(),
        iat: 0,
        exp: u64::MAX,
        jti: "jti-test".to_string(),
    }
}

fn test_state(db: Db) -> Arc<ApiState> {
    let (api_cmd_tx, _rx) = crossbeam::channel::bounded(1);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap();

    Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key,
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: None,
        cors_origins: vec![],
        db_reader: Some(Arc::new(db)),
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    })
}

/// Seed verse `v1` / fractal `f1` / petal `<petal_id>`. When `terrain` is
/// `Some`, store it as the petal's `terrain` JSON (used for bbox_ll tests).
async fn seed_hierarchy(db: &Db, petal_id: &str, terrain: Option<serde_json::Value>) {
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE verse CONTENT {
            verse_id: 'v1', name: 'V', created_by: 'did:key:z6MkOwner', created_at: $now
        }",
    )
    .bind(("now", now.clone()))
    .await
    .unwrap()
    .check()
    .unwrap();
    db.query(
        "CREATE fractal CONTENT {
            fractal_id: 'f1', verse_id: 'v1', owner_did: 'did:key:z6MkOwner', name: 'F', created_at: $now
        }",
    )
    .bind(("now", now.clone()))
    .await
    .unwrap()
    .check()
    .unwrap();
    let terrain_clause = if terrain.is_some() {
        ", terrain: $terrain"
    } else {
        ""
    };
    let petal_sql = format!(
        "CREATE petal CONTENT {{ \
         petal_id: $pid, fractal_id: 'f1', name: 'P', node_id: 'anchor-node', \
         created_at: $now{terrain_clause} }}"
    );
    let mut petal_q = db
        .query(petal_sql)
        .bind(("pid", petal_id.to_string()))
        .bind(("now", now));
    if let Some(t) = terrain {
        petal_q = petal_q.bind(("terrain", t));
    }
    petal_q.await.unwrap().check().unwrap();
}

/// Seed a node with a geometry position + optional properties object. When
/// `properties` is `None` the field is omitted entirely (NONE), matching how
/// `CreateNode` writes a fresh node — avoids binding JSON null to `option<object>`.
#[allow(clippy::too_many_arguments)] // test fixture — params mirror node columns
async fn seed_node(
    db: &Db,
    petal_id: &str,
    node_id: &str,
    display_name: &str,
    x: f64,
    z: f64,
    elevation: f64,
    properties: Option<serde_json::Value>,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let props_clause = if properties.is_some() {
        ", properties: $props"
    } else {
        ""
    };
    // Geometry MUST use an explicit cast — see fe-database/src/AGENTS.md §geometry-inserts.
    let sql = format!(
        "CREATE node CONTENT {{ \
         node_id: $nid, petal_id: $pid, display_name: $name, \
         position: <geometry<point>> [$x, $z], elevation: $ele, \
         rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
         interactive: true, created_at: $now{props_clause} }}"
    );
    let mut query = db
        .query(sql)
        .bind(("nid", node_id.to_string()))
        .bind(("pid", petal_id.to_string()))
        .bind(("name", display_name.to_string()))
        .bind(("x", x))
        .bind(("z", z))
        .bind(("ele", elevation))
        .bind(("now", now));
    if let Some(props) = properties {
        query = query.bind(("props", props));
    }
    query.await.unwrap().check().unwrap();
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

fn empty_query() -> GisNodesQuery {
    GisNodesQuery::default()
}

// ---------------------------------------------------------------------------
// Pure geometry helpers (no DB)
// ---------------------------------------------------------------------------

#[test]
fn parse_bbox_normalizes_min_max() {
    // Deliberately swapped corners; parse_bbox must normalize.
    let b = parse_bbox("10, 20, -5, -8").unwrap();
    assert_eq!(
        b,
        Bbox {
            minx: -5.0,
            minz: -8.0,
            maxx: 10.0,
            maxz: 20.0,
        }
    );
}

#[test]
fn parse_bbox_rejects_wrong_arity() {
    assert!(parse_bbox("1,2,3").is_err());
    assert!(parse_bbox("1,2,3,four").is_err());
}

#[test]
fn bbox_contains_is_inclusive_on_edges() {
    let b = parse_bbox("0,0,10,10").unwrap();
    assert!(b.contains(0.0, 0.0));
    assert!(b.contains(10.0, 10.0));
    assert!(b.contains(5.0, 5.0));
    assert!(!b.contains(10.001, 5.0));
    assert!(!b.contains(5.0, -0.001));
}

#[test]
fn within_radius_math() {
    // 3-4-5 triangle: point at (3,4) is exactly distance 5 from origin.
    assert!(within_radius(3.0, 4.0, 0.0, 0.0, 5.0));
    assert!(!within_radius(3.0, 4.0, 0.0, 0.0, 4.999));
    assert!(within_radius(0.0, 0.0, 0.0, 0.0, 0.0));
}

#[test]
fn bbox_ll_to_local_brackets_origin() {
    let proj = Projection::new(45.0, -122.0, 0.0);
    let b = bbox_ll_to_local("44.9,-122.1,45.1,-121.9", &proj).unwrap();
    // A symmetric lat/lon box about the origin must bracket local [0,0].
    assert!(b.minx < 0.0 && b.maxx > 0.0, "x brackets origin: {b:?}");
    assert!(b.minz < 0.0 && b.maxz > 0.0, "z brackets origin: {b:?}");
    assert!(b.contains(0.0, 0.0));
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nodes_rejects_insufficient_role() {
    let db = setup_test_db().await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "none");

    let resp = list_gis_nodes(
        State(state),
        Extension(claims),
        Path(ulid()),
        Query(empty_query()),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn nodes_rejects_out_of_scope_token() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;
    let state = test_state(db);
    // Viewer role but a token scoped to a *different* verse -> 403, not 404.
    let claims = test_claims("VERSE#other", "viewer");

    let resp = list_gis_nodes(
        State(state),
        Extension(claims),
        Path(petal_id),
        Query(empty_query()),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn nodes_rejects_invalid_petal_id() {
    let db = setup_test_db().await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = list_gis_nodes(
        State(state),
        Extension(claims),
        Path("not-a-ulid".to_string()),
        Query(empty_query()),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn nodes_unknown_petal_is_404() {
    let db = setup_test_db().await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    // Valid ULID that was never seeded -> scope resolution fails -> 404.
    let resp = list_gis_nodes(
        State(state),
        Extension(claims),
        Path(ulid()),
        Query(empty_query()),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Nodes endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nodes_empty_petal_returns_empty_list() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = list_gis_nodes(
        State(state),
        Extension(claims),
        Path(petal_id.clone()),
        Query(empty_query()),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["petal_id"], petal_id);
    assert_eq!(body["data"]["nodes"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn nodes_returns_position_and_extracts_annotation() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;

    let annotated_id = ulid();
    seed_node(
        &db,
        &petal_id,
        &annotated_id,
        "Water Station",
        12.5,
        -7.25,
        30.0,
        Some(serde_json::json!({
            "gis.annotation.title": "Water Station",
            "gis.annotation.body": "Refill here",
            "gis.annotation.color": "#00aaff"
        })),
    )
    .await;
    // A plain node with no annotation.
    seed_node(&db, &petal_id, &ulid(), "Rock", 1.0, 2.0, 0.0, None).await;

    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = list_gis_nodes(
        State(state),
        Extension(claims),
        Path(petal_id),
        Query(empty_query()),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let nodes = body["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);

    let annotated = nodes
        .iter()
        .find(|n| n["node_id"] == annotated_id)
        .expect("annotated node present");
    assert_eq!(annotated["display_name"], "Water Station");
    assert_eq!(annotated["position"][0].as_f64().unwrap(), 12.5);
    assert_eq!(annotated["position"][1].as_f64().unwrap(), -7.25);
    assert_eq!(annotated["elevation"].as_f64().unwrap(), 30.0);
    assert_eq!(annotated["annotation"]["title"], "Water Station");
    assert_eq!(annotated["annotation"]["body"], "Refill here");
    assert_eq!(annotated["annotation"]["color"], "#00aaff");

    let plain = nodes
        .iter()
        .find(|n| n["display_name"] == "Rock")
        .expect("plain node present");
    assert!(plain.get("annotation").is_none() || plain["annotation"].is_null());
}

#[tokio::test]
async fn nodes_bbox_filter_selects_inside_only() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;

    let inside_id = ulid();
    seed_node(&db, &petal_id, &inside_id, "Inside", 5.0, 5.0, 0.0, None).await;
    seed_node(&db, &petal_id, &ulid(), "Outside", 100.0, 100.0, 0.0, None).await;

    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let q = GisNodesQuery {
        bbox: Some("0,0,10,10".to_string()),
        ..Default::default()
    };
    let resp = list_gis_nodes(State(state), Extension(claims), Path(petal_id), Query(q)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let nodes = body["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], inside_id);
}

#[tokio::test]
async fn nodes_radius_filter_selects_inside_only() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;

    let near_id = ulid();
    seed_node(&db, &petal_id, &near_id, "Near", 3.0, 4.0, 0.0, None).await; // dist 5
    seed_node(&db, &petal_id, &ulid(), "Far", 30.0, 40.0, 0.0, None).await; // dist 50

    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let q = GisNodesQuery {
        radius: Some(10.0),
        cx: Some(0.0),
        cz: Some(0.0),
        ..Default::default()
    };
    let resp = list_gis_nodes(State(state), Extension(claims), Path(petal_id), Query(q)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let nodes = body["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], near_id);
}

#[tokio::test]
async fn nodes_radius_without_center_is_400() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let q = GisNodesQuery {
        radius: Some(10.0),
        ..Default::default()
    };
    let resp = list_gis_nodes(State(state), Extension(claims), Path(petal_id), Query(q)).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn nodes_conflicting_filters_is_400() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let q = GisNodesQuery {
        bbox: Some("0,0,1,1".to_string()),
        radius: Some(10.0),
        cx: Some(0.0),
        cz: Some(0.0),
        ..Default::default()
    };
    let resp = list_gis_nodes(State(state), Extension(claims), Path(petal_id), Query(q)).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn nodes_bbox_ll_requires_terrain_origin() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    // No terrain configured -> bbox_ll cannot be converted -> 400.
    seed_hierarchy(&db, &petal_id, None).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let q = GisNodesQuery {
        bbox_ll: Some("44.9,-122.1,45.1,-121.9".to_string()),
        ..Default::default()
    };
    let resp = list_gis_nodes(State(state), Extension(claims), Path(petal_id), Query(q)).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn nodes_bbox_ll_converts_via_terrain_origin() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    let terrain = serde_json::json!({
        "enabled": true,
        "origin": { "origin_lat": 45.0, "origin_lon": -122.0, "origin_ele": 0.0 },
        "tile_source_url": "",
        "elevation_source": "none"
    });
    seed_hierarchy(&db, &petal_id, Some(terrain)).await;

    let origin_node = ulid();
    seed_node(
        &db,
        &petal_id,
        &origin_node,
        "AtOrigin",
        0.0,
        0.0,
        0.0,
        None,
    )
    .await;
    // ~500 km NE of origin — far outside a ~0.2 degree lat/lon box.
    seed_node(
        &db,
        &petal_id,
        &ulid(),
        "FarAway",
        500_000.0,
        500_000.0,
        0.0,
        None,
    )
    .await;

    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let q = GisNodesQuery {
        bbox_ll: Some("44.9,-122.1,45.1,-121.9".to_string()),
        ..Default::default()
    };
    let resp = list_gis_nodes(State(state), Extension(claims), Path(petal_id), Query(q)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let nodes = body["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], origin_node);
}

// ---------------------------------------------------------------------------
// Tracks endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tracks_returns_only_track_nodes_with_stats() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;

    let track_id = ulid();
    seed_node(
        &db,
        &petal_id,
        &track_id,
        "Morning Run",
        1.0,
        2.0,
        10.0,
        Some(serde_json::json!({
            "gpx_type": "track",
            "total_distance_m": 5000.0,
            "elevation_gain_m": 120.0,
            "elevation_loss_m": 118.0,
            "duration_s": 1800.0,
            "avg_speed_kmh": 10.0,
            "max_speed_kmh": 18.0,
            "bounding_box": { "min_lat": 45.0, "max_lat": 45.1, "min_lon": -122.0, "max_lon": -121.9 }
        })),
    )
    .await;
    // Noise that must NOT appear in the tracks listing.
    seed_node(
        &db,
        &petal_id,
        &ulid(),
        "Trackpoint",
        1.1,
        2.1,
        10.0,
        Some(serde_json::json!({ "gpx_type": "trackpoint", "lat": 45.0, "lon": -122.0 })),
    )
    .await;
    seed_node(&db, &petal_id, &ulid(), "Plain", 3.0, 3.0, 0.0, None).await;

    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = list_gis_tracks(State(state), Extension(claims), Path(petal_id.clone())).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["petal_id"], petal_id);
    let tracks = body["data"]["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1);
    let t = &tracks[0];
    assert_eq!(t["node_id"], track_id);
    assert_eq!(t["display_name"], "Morning Run");
    assert_eq!(t["total_distance_m"].as_f64().unwrap(), 5000.0);
    assert_eq!(t["elevation_gain_m"].as_f64().unwrap(), 120.0);
    assert_eq!(t["avg_speed_kmh"].as_f64().unwrap(), 10.0);
    assert_eq!(t["bounding_box"]["min_lat"].as_f64().unwrap(), 45.0);
}

#[tokio::test]
async fn tracks_rejects_insufficient_role() {
    let petal_id = ulid();
    let db = setup_test_db().await;
    seed_hierarchy(&db, &petal_id, None).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "none");

    let resp = list_gis_tracks(State(state), Extension(claims), Path(petal_id)).await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
