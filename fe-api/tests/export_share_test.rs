//! Integration tests for the analytics-egress endpoints: parquet/CSV exports
//! (`fe-api/src/export.rs`), signed shareable URLs (`share.rs`), and the FR-4/5
//! deltas on `/query`. Mirrors the in-memory SurrealDB + `ApiState` idiom of
//! `gis_test.rs`. READ-BACK assertions throughout.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use fe_api::export::{export_csv, export_parquet, ExportParams};
use fe_api::server::ApiState;
use fe_api::share::{
    issue_share_url, mint_share_token, redeem_share_url, RedeemParams, SharePayload, ShareRequest,
};
use fe_database::handlers::iot_reading::{insert_readings, IotReadingInput};
use fe_identity::api_token::ApiClaims;
use fe_terrain::projection::Projection;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const ORIGIN_LAT: f64 = 47.6062;
const ORIGIN_LON: f64 = -122.3321;
const ORIGIN_ELE: f64 = 56.0;

async fn setup_test_db() -> Db {
    // insert_readings packs HLC timestamps; production init happens during DB
    // startup, which this raw in-memory setup bypasses (iot_ingest_test idiom).
    fe_database::op_log::init_hlc(0);
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
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    })
}

/// Seed verse `v1` / fractal `f1` and one petal; optional terrain origin JSON.
async fn seed_petal(db: &Db, petal_id: &str, with_origin: bool) {
    let now = chrono::Utc::now().to_rfc3339();
    // Verse/fractal are idempotent-ish per test db; ignore duplicate failures.
    let _ = db
        .query("CREATE verse CONTENT { verse_id: 'v1', name: 'V', created_by: 'did:key:z6MkOwner', created_at: $now }")
        .bind(("now", now.clone()))
        .await;
    let _ = db
        .query("CREATE fractal CONTENT { fractal_id: 'f1', verse_id: 'v1', owner_did: 'did:key:z6MkOwner', name: 'F', created_at: $now }")
        .bind(("now", now.clone()))
        .await;
    let terrain_clause = if with_origin {
        ", terrain: $terrain"
    } else {
        ""
    };
    let sql = format!(
        "CREATE petal CONTENT {{ petal_id: $pid, fractal_id: 'f1', name: 'P', \
         node_id: 'anchor-node', created_at: $now{terrain_clause} }}"
    );
    let mut q = db
        .query(sql)
        .bind(("pid", petal_id.to_string()))
        .bind(("now", now));
    if with_origin {
        q = q.bind((
            "terrain",
            serde_json::json!({
                "origin": {
                    "origin_lat": ORIGIN_LAT,
                    "origin_lon": ORIGIN_LON,
                    "origin_ele": ORIGIN_ELE,
                }
            }),
        ));
    }
    q.await.unwrap().check().unwrap();
}

/// Seed a node (geometry cast per fe-database/src/AGENTS.md §geometry-inserts).
async fn seed_node(db: &Db, petal_id: &str, node_id: &str, x: f64, z: f64, elevation: f64) {
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE node CONTENT { \
         node_id: $nid, petal_id: $pid, display_name: $name, \
         position: <geometry<point>> [$x, $z], elevation: $ele, \
         rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
         interactive: true, created_at: $now }",
    )
    .bind(("nid", node_id.to_string()))
    .bind(("pid", petal_id.to_string()))
    .bind(("name", format!("node-{node_id}")))
    .bind(("x", x))
    .bind(("z", z))
    .bind(("ele", elevation))
    .bind(("now", now))
    .await
    .unwrap()
    .check()
    .unwrap();
}

/// Seed one IoT reading anchored to `node_id` (direct write path, mirrors
/// `iot_ingest_test.rs`'s idiom — no HTTP round-trip needed for fixtures).
async fn seed_reading(db: &Db, petal_id: &str, node_id: &str, metric: &str, value: f64) {
    let reading = IotReadingInput {
        node_id: node_id.to_string(),
        metric: metric.to_string(),
        value,
        units: "celsius".to_string(),
        recorded_at: Some("2026-10-09T12:00:00+00:00".to_string()),
    };
    insert_readings(db, petal_id, "did:key:z6MkSensor", &[reading])
        .await
        .expect("seed reading");
}

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

async fn body_bytes(resp: Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn body_json(resp: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(resp).await).unwrap()
}

fn header_str(resp: &Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn params(query: Option<&str>, coords: Option<&str>) -> ExportParams {
    ExportParams {
        query: query.map(str::to_string),
        coords: coords.map(str::to_string),
    }
}

/// Write parquet body bytes to a temp file and read snapshots back (READ-BACK).
fn read_parquet_back(bytes: &[u8]) -> Vec<fe_entity_store::EntitySnapshot> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.parquet");
    std::fs::write(&path, bytes).unwrap();
    fe_query::columnar::geoparquet::read_nodes_parquet(&path).unwrap()
}

fn read_parquet_geo_meta(bytes: &[u8]) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.parquet");
    std::fs::write(&path, bytes).unwrap();
    let raw = fe_query::columnar::geoparquet::read_geo_metadata(&path)
        .unwrap()
        .expect("geo meta");
    serde_json::from_str(&raw).unwrap()
}

/// Write readings parquet body bytes to a temp file and read rows back (READ-BACK).
fn read_readings_parquet_back(bytes: &[u8]) -> Vec<fe_entity_store::ReadingSnapshot> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.parquet");
    std::fs::write(&path, bytes).unwrap();
    fe_query::columnar::geoparquet::read_readings_parquet(&path).unwrap()
}

// ---------------------------------------------------------------------------
// Phase 2 — export endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn export_parquet_round_trips_and_is_scope_filtered() {
    let db = setup_test_db().await;
    let (pa, pb) = (ulid(), ulid());
    seed_petal(&db, &pa, false).await;
    seed_petal(&db, &pb, false).await;
    seed_node(&db, &pa, "node-in-a", 1.5, 3.0, 2.0).await;
    seed_node(&db, &pb, "node-in-b", 9.0, 9.0, 9.0).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = export_parquet(
        State(state.clone()),
        Extension(claims),
        Path(pa.clone()),
        Query(params(None, None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header_str(&resp, "content-type"),
        "application/vnd.apache.parquet"
    );
    assert_eq!(header_str(&resp, "accept-ranges"), "bytes");
    let crs_header = header_str(&resp, "x-fe-crs");
    assert!(crs_header.contains("PETAL-LOCAL"), "{crs_header}");

    let bytes = body_bytes(resp).await;
    let snaps = read_parquet_back(&bytes);
    assert_eq!(snaps.len(), 1, "node from petal B must be pre-filtered out");
    assert_eq!(snaps[0].node_id, "node-in-a");
    assert_eq!(snaps[0].petal_id, pa);
    assert_eq!(snaps[0].position, [1.5, 2.0, 3.0]);

    // Landmine (unconfigured petal): local meters never masquerade as EPSG:4326.
    // DEC-C16: spec `crs` key is null; the honest label lives in `fe:crs`.
    let geo = read_parquet_geo_meta(&bytes);
    assert!(geo["columns"]["position"]["crs"].is_null());
    let crs = geo["columns"]["position"]["fe:crs"].as_str().unwrap();
    assert!(crs.contains("PETAL-LOCAL"), "{crs}");
    assert!(!crs.contains("4326"), "{crs}");
}

#[tokio::test]
async fn export_rejects_bad_role_scope_and_injection() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    seed_node(&db, &pa, "n1", 0.0, 0.0, 0.0).await;
    let state = test_state(db);

    // Wrong scope (different verse) → 403.
    let resp = export_parquet(
        State(state.clone()),
        Extension(test_claims("VERSE#v2", "viewer")),
        Path(pa.clone()),
        Query(params(None, None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Insufficient role → 403.
    let resp = export_csv(
        State(state.clone()),
        Extension(test_claims("VERSE#v1", "none")),
        Path(pa.clone()),
        Query(params(None, None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Injection attempts rejected identically to /query.
    let claims = test_claims("VERSE#v1", "viewer");
    for bad in [
        "SELECT * FROM node; DELETE node",
        "DELETE FROM node",
        "SELECT * FROM node WHERE x = (UPDATE node)",
        "SELECT * FROM secrets",
        "SELECT * FROM verse", // exports are node-table-only
    ] {
        let resp = export_csv(
            State(state.clone()),
            Extension(claims.clone()),
            Path(pa.clone()),
            Query(params(Some(bad), None)),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "should reject: {bad}"
        );
    }

    // Unknown petal → 404.
    let resp = export_parquet(
        State(state.clone()),
        Extension(claims),
        Path(ulid()),
        Query(params(None, None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn export_csv_local_vs_latlon_landmine() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, true).await; // terrain origin configured
    seed_node(&db, &pa, "n1", 100.0, 0.0, 10.0).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    // coords=local: labeled PETAL-LOCAL with the origin, never EPSG:4326.
    let resp = export_csv(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(None, Some("local"))),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    let first = csv.lines().next().unwrap();
    assert!(
        first.starts_with("# crs=PETAL-LOCAL:meters;origin=47.6062"),
        "{first}"
    );
    assert!(
        !first.contains("4326"),
        "local meters labeled as degrees: {first}"
    );
    assert!(csv.lines().nth(1).unwrap().contains("x_m,y_m,z_m"));
    assert!(
        csv.lines()
            .nth(2)
            .unwrap()
            .starts_with(&format!("n1,{pa},100,10,0")),
        "{csv}"
    );

    // coords=latlon: labeled EPSG:4326 with actually-converted coordinates.
    let resp = export_csv(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(None, Some("latlon"))),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, "x-fe-crs"), "EPSG:4326");
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(csv.lines().next().unwrap().starts_with("# crs=EPSG:4326"));
    assert!(csv.lines().nth(1).unwrap().contains("lon,lat,ele_m"));
    let row: Vec<&str> = csv.lines().nth(2).unwrap().split(',').collect();
    let (lon, lat, ele): (f64, f64, f64) = (
        row[2].parse().unwrap(),
        row[3].parse().unwrap(),
        row[4].parse().unwrap(),
    );
    let proj = Projection::new(ORIGIN_LAT, ORIGIN_LON, ORIGIN_ELE);
    let (exp_lat, exp_lon, exp_ele) = proj.local_to_wgs84(100.0, 10.0, 0.0);
    assert!((lat - exp_lat).abs() < 1e-4, "lat {lat} vs {exp_lat}");
    assert!((lon - exp_lon).abs() < 1e-4, "lon {lon} vs {exp_lon}");
    assert!((ele - exp_ele).abs() < 1e-2, "ele {ele} vs {exp_ele}");
    assert!(
        (lat - ORIGIN_LAT).abs() > 1e-7 || (lon - ORIGIN_LON).abs() > 1e-7,
        "coordinates were not actually converted"
    );

    // Parquet latlon: the spec `crs` key is OMITTED (absent = OGC:CRS84, the
    // lon/lat axis order we write — M4 minor #7); the label stays in `fe:crs`.
    let resp = export_parquet(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(None, Some("latlon"))),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let geo = read_parquet_geo_meta(&body_bytes(resp).await);
    let column = geo["columns"]["position"].as_object().unwrap();
    assert!(!column.contains_key("crs"), "latlon must omit crs: {geo}");
    assert_eq!(geo["columns"]["position"]["fe:crs"], "EPSG:4326");

    // Bad coords value → 400.
    let resp = export_csv(
        State(state.clone()),
        Extension(claims),
        Path(pa),
        Query(params(None, Some("weird"))),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn export_latlon_without_origin_is_rejected() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    let state = test_state(db);
    let resp = export_csv(
        State(state),
        Extension(test_claims("VERSE#v1", "viewer")),
        Path(pa),
        Query(params(None, Some("latlon"))),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// F11/A23 — readings export (iot_reading table, anchor-position join)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn export_readings_round_trips_scope_filtered_with_anchor_position() {
    let db = setup_test_db().await;
    let (pa, pb) = (ulid(), ulid());
    seed_petal(&db, &pa, false).await;
    seed_petal(&db, &pb, false).await;
    seed_node(&db, &pa, "anchor-a", 1.5, 3.0, 2.0).await;
    seed_node(&db, &pb, "anchor-b", 9.0, 9.0, 9.0).await;
    // Two readings in petal A, one in petal B — the forced pre-filter must
    // drop B's reading regardless of the query text (FR-6, same as nodes).
    seed_reading(&db, &pa, "anchor-a", "temperature_c", 23.456_789).await;
    seed_reading(&db, &pa, "anchor-a", "temperature_c", -40.0).await;
    seed_reading(&db, &pb, "anchor-b", "temperature_c", 100.0).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = export_parquet(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(Some("SELECT * FROM iot_reading"), None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header_str(&resp, "content-type"),
        "application/vnd.apache.parquet"
    );
    let crs_header = header_str(&resp, "x-fe-crs");
    assert!(crs_header.contains("PETAL-LOCAL"), "{crs_header}");

    let bytes = body_bytes(resp).await;
    let rows = read_readings_parquet_back(&bytes);
    assert_eq!(rows.len(), 2, "petal B's reading must not leak");
    assert!(rows.iter().all(|r| r.petal_id == pa));
    let values: Vec<f64> = rows.iter().map(|r| r.value).collect();
    assert!(
        values.contains(&23.456_789) && values.contains(&-40.0),
        "f64 value must round-trip bit-exact: {values:?}"
    );
    for r in &rows {
        assert_eq!(
            r.anchor_position,
            Some([1.5, 2.0, 3.0]),
            "anchor position joined from the node table, not fabricated"
        );
    }

    // Same query via CSV: anchor columns present, local-meters headers.
    let resp = export_csv(
        State(state.clone()),
        Extension(claims),
        Path(pa.clone()),
        Query(params(Some("SELECT * FROM iot_reading"), None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(csv
        .lines()
        .nth(1)
        .unwrap()
        .starts_with("reading_id,node_id,petal_id,metric,value,units,recorded_at,recorded_at_ms,anchor_x_m,anchor_y_m,anchor_z_m"));
    assert_eq!(
        csv.lines().count(),
        4,
        "header comment + column header + 2 data rows: {csv}"
    );
}

#[tokio::test]
async fn export_readings_latlon_converts_anchor_position() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, true).await; // terrain origin configured
    seed_node(&db, &pa, "anchor-a", 100.0, 0.0, 10.0).await;
    seed_reading(&db, &pa, "anchor-a", "co2_ppm", 415.2).await;
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = export_csv(
        State(state),
        Extension(claims),
        Path(pa.clone()),
        Query(params(Some("SELECT * FROM iot_reading"), Some("latlon"))),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, "x-fe-crs"), "EPSG:4326");
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(csv
        .lines()
        .nth(1)
        .unwrap()
        .contains("anchor_lon,anchor_lat,anchor_ele_m"));
    let row: Vec<&str> = csv.lines().nth(2).unwrap().split(',').collect();
    // reading_id,node_id,petal_id,metric,value,units,recorded_at,recorded_at_ms,lon,lat,ele
    let (lon, lat, ele): (f64, f64, f64) = (
        row[8].parse().unwrap(),
        row[9].parse().unwrap(),
        row[10].parse().unwrap(),
    );
    let proj = Projection::new(ORIGIN_LAT, ORIGIN_LON, ORIGIN_ELE);
    let (exp_lat, exp_lon, exp_ele) = proj.local_to_wgs84(100.0, 10.0, 0.0);
    assert!((lat - exp_lat).abs() < 1e-4, "lat {lat} vs {exp_lat}");
    assert!((lon - exp_lon).abs() < 1e-4, "lon {lon} vs {exp_lon}");
    assert!((ele - exp_ele).abs() < 1e-2, "ele {ele} vs {exp_ele}");
}

// (Non-whitelisted tables, e.g. `verse`, are still covered by
// `export_rejects_bad_role_scope_and_injection` above — the two-table
// whitelist change didn't need a dedicated duplicate test.)

#[tokio::test]
async fn reading_row_cap_reuses_the_same_guarded_mechanism_as_nodes() {
    // F11/A23: the readings path calls the exact same `run_guarded_query`
    // helper the node path does (pinned generically by
    // `row_cap_produces_clear_error` below) — this test only confirms the
    // cap applies when the FROM target is `iot_reading`, instead of seeding
    // 500_001 rows against the real `EXPORT_ROW_CAP` (documented choice —
    // see AGENTS.md §export).
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    seed_node(&db, &pa, "anchor-a", 0.0, 0.0, 0.0).await;
    seed_reading(&db, &pa, "anchor-a", "temperature_c", 1.0).await;
    seed_reading(&db, &pa, "anchor-a", "temperature_c", 2.0).await;
    let db = Arc::new(db);
    let guarded = fe_api::query_guard::GuardedQuery {
        sql: "SELECT * FROM iot_reading".into(),
    };
    let vars = std::collections::HashMap::new();

    let ok = fe_api::query_guard::run_guarded_query(&db, &guarded, &vars, 2).await;
    assert_eq!(ok.unwrap().len(), 2);

    let err = fe_api::query_guard::run_guarded_query(&db, &guarded, &vars, 1)
        .await
        .unwrap_err();
    assert!(err.contains("row cap exceeded (limit 1 rows"), "{err}");
}

// ---------------------------------------------------------------------------
// FR-4 delta — row cap on the shared execution path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn row_cap_produces_clear_error() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    for i in 0..3 {
        seed_node(&db, &pa, &format!("n{i}"), i as f64, 0.0, 0.0).await;
    }
    let db = Arc::new(db);
    let guarded = fe_api::query_guard::GuardedQuery {
        sql: "SELECT * FROM node".into(),
    };
    let vars = std::collections::HashMap::new();

    // Under the cap: fine.
    let ok = fe_api::query_guard::run_guarded_query(&db, &guarded, &vars, 3).await;
    assert_eq!(ok.unwrap().len(), 3);

    // Over the cap: clear error (documented choice: error, not truncate).
    let err = fe_api::query_guard::run_guarded_query(&db, &guarded, &vars, 2)
        .await
        .unwrap_err();
    assert!(err.contains("row cap exceeded (limit 2 rows"), "{err}");
}

// ---------------------------------------------------------------------------
// Phase 5 — /query JSON envelope CRS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_envelope_carries_resolved_crs() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, true).await;
    seed_node(&db, &pa, "n1", 1.0, 2.0, 0.0).await;
    let state = test_state(db);
    let claims = test_claims(&format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}"), "viewer");

    let req = fe_api::types::QueryRequest {
        sql: "SELECT * FROM node".to_string(),
        vars: std::collections::HashMap::new(),
        distributed: None,
    };
    let resp = fe_api::rest::execute_query(State(state), Extension(claims), Json(req))
        .await
        .into_response();
    let body = body_json(resp).await;
    assert!(body["ok"].as_bool().unwrap(), "{body}");
    let crs = body["data"]["crs"]
        .as_str()
        .expect("crs field on /query envelope");
    assert!(crs.contains("origin=47.6062"), "{crs}");
    assert_eq!(body["data"]["data"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Phase 3 — signed shareable URLs
// ---------------------------------------------------------------------------

fn share_req(sql: &str, format: &str, ttl: Option<u64>) -> ShareRequest {
    ShareRequest {
        sql: sql.to_string(),
        format: format.to_string(),
        ttl_secs: ttl,
    }
}

#[tokio::test]
async fn share_issue_redeem_round_trip_enforces_scope_ceiling() {
    let db = setup_test_db().await;
    let (pa, pb) = (ulid(), ulid());
    seed_petal(&db, &pa, false).await;
    seed_petal(&db, &pb, false).await;
    seed_node(&db, &pa, "node-in-a", 1.0, 1.0, 0.0).await;
    seed_node(&db, &pb, "node-in-b", 2.0, 2.0, 0.0).await;
    let state = test_state(db);
    // Narrow (petal-A) issuer.
    let claims = test_claims(&format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}"), "viewer");

    // Issue a JSON share link for a query that TRIES to read everything.
    let resp = issue_share_url(
        State(state.clone()),
        Extension(claims.clone()),
        Json(share_req("SELECT * FROM node", "json", Some(3600))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["ok"].as_bool().unwrap(), "{body}");
    let url = body["data"]["url"].as_str().unwrap();
    assert!(url.starts_with("/api/v1/shared/"), "{url}");
    let token = body["data"]["token"].as_str().unwrap().to_string();
    assert_eq!(
        body["data"]["scope"].as_str().unwrap(),
        claims.scope,
        "ceiling = issuer scope"
    );

    // Redeem WITHOUT any session: scope ceiling caps the result to petal A.
    let resp = redeem_share_url(
        State(state.clone()),
        Path(token.clone()),
        Query(RedeemParams::default()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body["data"]["data"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "narrow-scope URL must not read outside its scope: {body}"
    );
    assert_eq!(rows[0]["node_id"], "node-in-a");
    assert!(
        body["data"]["crs"].as_str().is_some(),
        "shared JSON carries CRS"
    );

    // Parquet share link: redeemed export is pre-filtered to the ceiling.
    let resp = issue_share_url(
        State(state.clone()),
        Extension(claims),
        Json(share_req("SELECT * FROM node", "parquet", None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let token = body_json(resp).await["data"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = redeem_share_url(
        State(state),
        Path(token),
        Query(RedeemParams::default()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header_str(&resp, "content-type"),
        "application/vnd.apache.parquet"
    );
    let snaps = read_parquet_back(&body_bytes(resp).await);
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].node_id, "node-in-a");
}

#[tokio::test]
async fn share_readings_parquet_round_trip_is_scope_filtered() {
    let db = setup_test_db().await;
    let (pa, pb) = (ulid(), ulid());
    seed_petal(&db, &pa, false).await;
    seed_petal(&db, &pb, false).await;
    seed_node(&db, &pa, "anchor-a", 1.0, 1.0, 0.0).await;
    seed_node(&db, &pb, "anchor-b", 2.0, 2.0, 0.0).await;
    seed_reading(&db, &pa, "anchor-a", "humidity_pct", 55.0).await;
    seed_reading(&db, &pb, "anchor-b", "humidity_pct", 10.0).await;
    let state = test_state(db);
    let claims = test_claims(&format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}"), "viewer");

    let resp = issue_share_url(
        State(state.clone()),
        Extension(claims),
        Json(share_req("SELECT * FROM iot_reading", "parquet", None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let token = body_json(resp).await["data"]["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Redeemed UNAUTHENTICATED (no Authorization header, no claims extension).
    let resp = redeem_share_url(
        State(state),
        Path(token),
        Query(RedeemParams::default()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header_str(&resp, "content-type"),
        "application/vnd.apache.parquet"
    );
    let rows = read_readings_parquet_back(&body_bytes(resp).await);
    assert_eq!(
        rows.len(),
        1,
        "narrow-scope URL must not read outside its scope"
    );
    assert_eq!(rows[0].petal_id, pa);
    assert_eq!(rows[0].value, 55.0);
    assert_eq!(rows[0].anchor_position, Some([1.0, 0.0, 1.0]));
}

#[tokio::test]
async fn share_expired_tampered_and_invalid_rejected() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    let state = test_state(db);

    // Expired token → 410 GONE.
    let expired = SharePayload {
        v: 1,
        sql: "SELECT * FROM node".into(),
        scope: format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}"),
        fmt: "json".into(),
        exp: 1, // 1970 — long expired
        sub: "did:key:z6MkIssuer".into(),
    };
    let token = mint_share_token(&state.share_signer, &expired).unwrap();
    let resp = redeem_share_url(
        State(state.clone()),
        Path(token),
        Query(RedeemParams::default()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::GONE);

    // Token signed by a DIFFERENT key (i.e. tampered/forged) → 401.
    let mut valid = expired.clone();
    valid.exp = u64::MAX;
    let foreign = fe_identity::NodeKeypair::generate();
    let forged = mint_share_token(&foreign, &valid).unwrap();
    let resp = redeem_share_url(
        State(state.clone()),
        Path(forged),
        Query(RedeemParams::default()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Garbage token → 401.
    let resp = redeem_share_url(
        State(state.clone()),
        Path("garbage".into()),
        Query(RedeemParams::default()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Issue-time validation: bad format / bad ttl / injection → 400.
    let claims = test_claims(&format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}"), "viewer");
    for (sql, fmt, ttl) in [
        ("SELECT * FROM node", "xml", Some(60)),
        ("SELECT * FROM node", "json", Some(999_999)),
        ("SELECT * FROM node; DELETE node", "json", Some(60)),
        ("SELECT * FROM verse", "parquet", Some(60)), // exports are node-table-only
    ] {
        let resp = issue_share_url(
            State(state.clone()),
            Extension(claims.clone()),
            Json(share_req(sql, fmt, ttl)),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{sql} / {fmt} / {ttl:?}"
        );
    }

    // Verse-scoped issuer cannot mint parquet/csv links (no petal for CRS).
    let resp = issue_share_url(
        State(state.clone()),
        Extension(test_claims("VERSE#v1", "viewer")),
        Json(share_req("SELECT * FROM node", "parquet", Some(60))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// M4 fix B1 — scope-filter bypass vectors (export + share + /query)
// ---------------------------------------------------------------------------

/// Petals A and B (same verse) each with a node + a reading.
async fn two_petal_fixture() -> (Arc<ApiState>, String, String) {
    let db = setup_test_db().await;
    let (pa, pb) = (ulid(), ulid());
    seed_petal(&db, &pa, false).await;
    seed_petal(&db, &pb, false).await;
    seed_node(&db, &pa, "node-in-a", 1.0, 1.0, 0.0).await;
    seed_node(&db, &pb, "node-in-b", 2.0, 2.0, 0.0).await;
    seed_reading(&db, &pa, "node-in-a", "temperature_c", 1.0).await;
    seed_reading(&db, &pb, "node-in-b", "temperature_c", 2.0).await;
    (test_state(db), pa, pb)
}

/// The surfaces a petal-A caller can reach with attacker-chosen SQL.
#[derive(Debug, Clone, Copy)]
enum Surface {
    ExportParquet,
    ExportCsv,
    ShareParquet,
    ShareCsv,
    ShareJson,
    QueryJson,
}

const ALL_SURFACES: [Surface; 6] = [
    Surface::ExportParquet,
    Surface::ExportCsv,
    Surface::ShareParquet,
    Surface::ShareCsv,
    Surface::ShareJson,
    Surface::QueryJson,
];

/// Run `sql` against `surface` as a petal-A caller; return (status, body).
/// Share tokens are minted DIRECTLY (bypassing mint-time validation) so the
/// redemption-time guard is what is under test — tokens minted before the
/// fix carry arbitrary SQL.
async fn hit(
    state: &Arc<ApiState>,
    pa: &str,
    surface: Surface,
    sql: &str,
) -> (StatusCode, Vec<u8>) {
    let scope = format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}");
    let share = |fmt: &str| SharePayload {
        v: 1,
        sql: sql.to_string(),
        scope: scope.clone(),
        fmt: fmt.to_string(),
        exp: u64::MAX,
        sub: "did:key:z6MkIssuer".into(),
    };
    let resp = match surface {
        Surface::ExportParquet => {
            export_parquet(
                State(state.clone()),
                Extension(test_claims("VERSE#v1", "viewer")),
                Path(pa.to_string()),
                Query(params(Some(sql), None)),
                HeaderMap::new(),
            )
            .await
        }
        Surface::ExportCsv => {
            export_csv(
                State(state.clone()),
                Extension(test_claims("VERSE#v1", "viewer")),
                Path(pa.to_string()),
                Query(params(Some(sql), None)),
                HeaderMap::new(),
            )
            .await
        }
        Surface::ShareParquet | Surface::ShareCsv | Surface::ShareJson => {
            let fmt = match surface {
                Surface::ShareParquet => "parquet",
                Surface::ShareCsv => "csv",
                _ => "json",
            };
            let token = mint_share_token(&state.share_signer, &share(fmt)).unwrap();
            redeem_share_url(
                State(state.clone()),
                Path(token),
                Query(RedeemParams::default()),
                HeaderMap::new(),
            )
            .await
        }
        Surface::QueryJson => {
            let req = fe_api::types::QueryRequest {
                sql: sql.to_string(),
                vars: std::collections::HashMap::new(),
                distributed: None,
            };
            fe_api::rest::execute_query(
                State(state.clone()),
                Extension(test_claims(&scope, "viewer")),
                Json(req),
            )
            .await
            .into_response()
        }
    };
    let status = resp.status();
    (status, body_bytes(resp).await)
}

/// Rows a successful body actually carries, as (petal_id, node_id) pairs.
fn body_rows(surface: Surface, sql: &str, body: &[u8]) -> Vec<(String, String)> {
    let squashed: String = sql
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let readings = squashed.contains("from iot_reading");
    match surface {
        Surface::ExportParquet | Surface::ShareParquet if readings => {
            read_readings_parquet_back(body)
                .into_iter()
                .map(|r| (r.petal_id, r.node_id))
                .collect()
        }
        Surface::ExportParquet | Surface::ShareParquet => read_parquet_back(body)
            .into_iter()
            .map(|s| (s.petal_id, s.node_id))
            .collect(),
        Surface::ExportCsv | Surface::ShareCsv => {
            let text = String::from_utf8(body.to_vec()).unwrap();
            text.lines()
                .skip(2)
                .map(|l| {
                    let f: Vec<&str> = l.split(',').collect();
                    // nodes: node_id,petal_id,…  readings: reading_id,node_id,petal_id,…
                    if readings {
                        (f[2].to_string(), f[1].to_string())
                    } else {
                        (f[1].to_string(), f[0].to_string())
                    }
                })
                .collect()
        }
        Surface::ShareJson | Surface::QueryJson => {
            let v: serde_json::Value = serde_json::from_slice(body).unwrap();
            v["data"]["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    (
                        r["petal_id"].as_str().unwrap_or_default().to_string(),
                        r["node_id"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        }
    }
}

/// The four M4-review bypass vectors (+ record-literal / table-expression
/// extras) on every egress surface: each attempt is either rejected (4xx /
/// error envelope) or answers ONLY petal-A rows — and, where it succeeds,
/// still returns A's own row (the fix scopes, it does not just deny).
#[tokio::test]
async fn scope_bypass_vectors_never_leak_foreign_rows() {
    let (state, pa, pb) = two_petal_fixture().await;
    let vectors: &[(&str, &str)] = &[
        ("double-space", "SELECT * FROM  node"),
        ("tab", "SELECT * FROM\tnode"),
        ("newline", "SELECT * FROM\nnode"),
        ("readings double-space", "SELECT * FROM  iot_reading"),
        ("OR precedence", "SELECT * FROM node WHERE true OR true"),
        (
            "readings OR precedence",
            "SELECT * FROM iot_reading WHERE true OR true",
        ),
        ("trailing comment", "SELECT * FROM node --"),
        ("comment after WHERE", "SELECT * FROM node WHERE true --"),
        (
            "projection subquery",
            "SELECT *, (SELECT * FROM iot_reading) AS x FROM node",
        ),
        (
            "record literal",
            "SELECT *, node:x.* AS leak FROM iot_reading",
        ),
        ("table expression", "SELECT * FROM petal AND node"),
    ];
    let mut served = 0;
    for &(name, sql) in vectors {
        for surface in ALL_SURFACES {
            let (status, body) = hit(&state, &pa, surface, sql).await;
            let text = String::from_utf8_lossy(&body);
            assert!(
                !text.contains(&pb) && !text.contains("node-in-b"),
                "{name} on {surface:?} leaked petal B ({status}): {text}"
            );
            let errored = !status.is_success()
                || serde_json::from_slice::<serde_json::Value>(&body)
                    .map(|v| v["ok"] == false)
                    .unwrap_or(false);
            if errored {
                continue;
            }
            served += 1;
            let rows = body_rows(surface, sql, &body);
            assert!(
                rows.iter().all(|(petal, _)| *petal == pa),
                "{name} on {surface:?}: foreign rows {rows:?}"
            );
            assert!(
                rows.iter().any(|(_, node)| node == "node-in-a"),
                "{name} on {surface:?}: scoping must not drop the caller's own row: {rows:?}"
            );
        }
    }
    // Whitespace + OR vectors are legitimate SQL and must still be SERVED
    // (scoped), on all 6 surfaces: 5 vectors × 6 = 30 at minimum.
    assert!(
        served >= 30,
        "only {served} vector/surface pairs were served"
    );
}

/// The bypass vectors that must be REJECTED outright on export/share (not
/// merely filtered): comments and nested SELECTs (M4 fix B1(b)).
#[tokio::test]
async fn egress_dialect_rejects_comments_and_nested_selects() {
    let (state, pa, _pb) = two_petal_fixture().await;
    for sql in [
        "SELECT * FROM node --",
        "SELECT * FROM node /* x */",
        "SELECT *, (SELECT * FROM iot_reading) AS x FROM node",
    ] {
        for surface in [
            Surface::ExportParquet,
            Surface::ExportCsv,
            Surface::ShareParquet,
            Surface::ShareCsv,
            Surface::ShareJson,
        ] {
            let (status, body) = hit(&state, &pa, surface, sql).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{surface:?} {sql}: {}",
                String::from_utf8_lossy(&body)
            );
        }
        // Mint-time fail-fast too.
        let resp = issue_share_url(
            State(state.clone()),
            Extension(test_claims(
                &format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}"),
                "viewer",
            )),
            Json(share_req(sql, "parquet", Some(60))),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "mint {sql}");
    }
}

/// FIX 1(c): verse- and fractal-scoped tokens are row-filtered to the petals
/// under that verse/fractal — they used to get NO node/iot_reading filter.
#[tokio::test]
async fn verse_and_fractal_scopes_filter_rows_to_their_subtree() {
    let db = setup_test_db().await;
    let (pa, pc) = (ulid(), ulid());
    seed_petal(&db, &pa, false).await; // verse v1 / fractal f1
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE verse CONTENT { verse_id: 'v2', name: 'V2', created_by: 'did:key:z6MkOther', created_at: $now }; \
         CREATE fractal CONTENT { fractal_id: 'f2', verse_id: 'v2', owner_did: 'did:key:z6MkOther', name: 'F2', created_at: $now }; \
         CREATE petal CONTENT { petal_id: $pc, fractal_id: 'f2', name: 'C', node_id: 'x', created_at: $now }",
    )
    .bind(("now", now))
    .bind(("pc", pc.clone()))
    .await
    .unwrap()
    .check()
    .unwrap();
    seed_node(&db, &pa, "node-in-a", 1.0, 1.0, 0.0).await;
    seed_node(&db, &pc, "node-in-other-verse", 2.0, 2.0, 0.0).await;
    let state = test_state(db);

    for (scope, expected) in [
        ("VERSE#v1", vec!["node-in-a"]),
        ("VERSE#v1-FRACTAL#f1", vec!["node-in-a"]),
        ("VERSE#v2", vec!["node-in-other-verse"]),
        // Fractal f1 is not under verse v2 → deny, never "whatever f1 holds".
        ("VERSE#v2-FRACTAL#f1", vec![]),
    ] {
        let req = fe_api::types::QueryRequest {
            sql: "SELECT node_id FROM node".to_string(),
            vars: std::collections::HashMap::new(),
            distributed: None,
        };
        let resp = fe_api::rest::execute_query(
            State(state.clone()),
            Extension(test_claims(scope, "viewer")),
            Json(req),
        )
        .await
        .into_response();
        let body = body_json(resp).await;
        assert_eq!(body["ok"], true, "{scope}: {body}");
        let ids: Vec<&str> = body["data"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["node_id"].as_str())
            .collect();
        assert_eq!(ids, expected, "{scope}");
    }
}

/// M4 minor #6: a reading whose anchor node was hard-deleted exports a NULL
/// geometry (parquet `None`, empty CSV cells) — never a fabricated 0,0,0.
#[tokio::test]
async fn hard_deleted_anchor_exports_null_geometry() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    seed_node(&db, &pa, "ghost", 5.0, 6.0, 7.0).await;
    seed_reading(&db, &pa, "ghost", "temperature_c", 3.5).await;
    db.query("DELETE node WHERE node_id = 'ghost'")
        .await
        .unwrap()
        .check()
        .unwrap();
    let state = test_state(db);
    let claims = test_claims("VERSE#v1", "viewer");

    let resp = export_parquet(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(Some("SELECT * FROM iot_reading"), None)),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let rows = read_readings_parquet_back(&body_bytes(resp).await);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].anchor_position, None, "dead anchor must be null");

    let resp = export_csv(
        State(state),
        Extension(claims),
        Path(pa),
        Query(params(Some("SELECT * FROM iot_reading"), None)),
        HeaderMap::new(),
    )
    .await;
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    let row = csv.lines().nth(2).unwrap();
    assert!(row.ends_with(",,,"), "anchor cells must be empty: {row}");
}

/// DEC-C18 end-to-end: export bodies carry a strong ETag + no-store, a
/// matching If-Range yields 206, a stale one yields the full 200.
#[tokio::test]
async fn export_etag_and_if_range_round_trip() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    seed_node(&db, &pa, "n1", 1.0, 2.0, 3.0).await;
    let state = test_state(db);
    let get = |headers: HeaderMap| {
        export_csv(
            State(state.clone()),
            Extension(test_claims("VERSE#v1", "viewer")),
            Path(pa.clone()),
            Query(params(None, None)),
            headers,
        )
    };
    let full = get(HeaderMap::new()).await;
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(header_str(&full, "cache-control"), "private, no-store");
    let etag = header_str(&full, "etag");
    assert!(etag.starts_with('"'), "{etag}");
    assert_eq!(
        header_str(&get(HeaderMap::new()).await, "etag"),
        etag,
        "identical body → identical ETag"
    );

    let mut ranged = HeaderMap::new();
    ranged.insert("range", "bytes=0-3".parse().unwrap());
    ranged.insert("if-range", etag.parse().unwrap());
    assert_eq!(get(ranged).await.status(), StatusCode::PARTIAL_CONTENT);

    let mut stale = HeaderMap::new();
    stale.insert("range", "bytes=0-3".parse().unwrap());
    stale.insert("if-range", "\"stale\"".parse().unwrap());
    assert_eq!(get(stale).await.status(), StatusCode::OK);
}

/// M4 fix M2: the guarded statement carries a DB-side `TIMEOUT 5s`, so a
/// heavy query is aborted BY THE DB (freeing its thread) before the 6s
/// client backstop — the error is SurrealDB's, not the client timer's.
#[tokio::test]
async fn db_side_statement_timeout_aborts_heavy_query() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa, false).await;
    seed_node(&db, &pa, "n1", 0.0, 0.0, 0.0).await;
    let db = Arc::new(db);
    let guarded = fe_api::query_guard::GuardedQuery {
        sql: "SELECT * FROM node WHERE sleep(30s) = NONE".into(),
    };
    let started = std::time::Instant::now();
    let err = fe_api::query_guard::run_guarded_query(&db, &guarded, &Default::default(), 10)
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(6),
        "{err}"
    );
    assert!(
        !err.starts_with("query timed out"),
        "client backstop fired instead of the DB TIMEOUT: {err}"
    );
    assert!(fe_api::query_guard::is_timeout_error(&err), "{err}");
}

/// M4 fix B2: with `db_reader: None` the export/share seam
/// (`run_guarded_query_via_state`) is correlated — an interleaved GUI
/// (uncorrelated) result and another caller's correlated result never reach
/// this caller; it gets exactly its own rows.
#[tokio::test]
async fn channel_fallback_never_receives_foreign_query_results() {
    use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded::<ApiCommand>(4);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
    let state = ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: None,
        cors_origins: vec![],
        db_reader: None,
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    };

    // Stand-in for the Bevy drain + DB thread, using the REAL reply router.
    let router = std::thread::spawn(move || {
        let mut pending = fe_runtime::app::PendingApiRequests::default();
        let ApiCommand::DbRequest { cmd, reply_tx } = api_cmd_rx.recv().unwrap() else {
            panic!("expected a DbRequest");
        };
        let DbCommand::RawQuery {
            correlation_id: Some(id),
            sql,
            ..
        } = &cmd
        else {
            panic!("API RawQuery must be correlated: {cmd:?}");
        };
        assert!(sql.ends_with("TIMEOUT 5s"), "{sql}");
        let id = id.clone();
        pending.enqueue_for(&cmd, reply_tx);
        let foreign = |cid: Option<String>| DbResult::QueryResult {
            data: vec![serde_json::json!({ "petal_id": "FOREIGN" })],
            correlation_id: cid,
        };
        // Interleaved before ours: the desktop user's Query tab + another caller.
        assert!(!pending.try_deliver(foreign(None)));
        assert!(!pending.try_deliver(foreign(Some("someone-else".into()))));
        assert!(pending.try_deliver(DbResult::QueryResult {
            data: vec![serde_json::json!({ "petal_id": "MINE" })],
            correlation_id: Some(id),
        }));
    });

    let rows = fe_api::query_guard::run_guarded_query_via_state(
        &state,
        &fe_api::query_guard::GuardedQuery {
            sql: "SELECT * FROM node".into(),
        },
        &Default::default(),
        10,
    )
    .await
    .expect("own result");
    router.join().unwrap();
    assert_eq!(rows, vec![serde_json::json!({ "petal_id": "MINE" })]);
}
