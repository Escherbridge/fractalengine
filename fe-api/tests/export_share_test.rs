//! Integration tests for the analytics-egress endpoints: parquet/CSV exports
//! (`fe-api/src/export.rs`), signed shareable URLs (`share.rs`), and the FR-4/5
//! deltas on `/query`. Mirrors the in-memory SurrealDB + `ApiState` idiom of
//! `gis_test.rs`. READ-BACK assertions throughout.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use fe_api::export::{export_csv, export_parquet, ExportParams};
use fe_api::server::ApiState;
use fe_api::share::{issue_share_url, mint_share_token, redeem_share_url, RedeemParams, SharePayload, ShareRequest};
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
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db).await.expect("apply schema");
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
    let terrain_clause = if with_origin { ", terrain: $terrain" } else { "" };
    let sql = format!(
        "CREATE petal CONTENT {{ petal_id: $pid, fractal_id: 'f1', name: 'P', \
         node_id: 'anchor-node', created_at: $now{terrain_clause} }}"
    );
    let mut q = db.query(sql).bind(("pid", petal_id.to_string())).bind(("now", now));
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

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

async fn body_bytes(resp: Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()
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
    let raw = fe_query::columnar::geoparquet::read_geo_metadata(&path).unwrap().expect("geo meta");
    serde_json::from_str(&raw).unwrap()
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
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, "content-type"), "application/vnd.apache.parquet");
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
    let geo = read_parquet_geo_meta(&bytes);
    let crs = geo["columns"]["position"]["crs"].as_str().unwrap();
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
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Insufficient role → 403.
    let resp = export_csv(
        State(state.clone()),
        Extension(test_claims("VERSE#v1", "none")),
        Path(pa.clone()),
        Query(params(None, None)),
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
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "should reject: {bad}");
    }

    // Unknown petal → 404.
    let resp = export_parquet(
        State(state.clone()),
        Extension(claims),
        Path(ulid()),
        Query(params(None, None)),
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
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    let first = csv.lines().next().unwrap();
    assert!(first.starts_with("# crs=PETAL-LOCAL:meters;origin=47.6062"), "{first}");
    assert!(!first.contains("4326"), "local meters labeled as degrees: {first}");
    assert!(csv.lines().nth(1).unwrap().contains("x_m,y_m,z_m"));
    assert!(csv.lines().nth(2).unwrap().starts_with(&format!("n1,{pa},100,10,0")), "{csv}");

    // coords=latlon: labeled EPSG:4326 with actually-converted coordinates.
    let resp = export_csv(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(None, Some("latlon"))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, "x-fe-crs"), "EPSG:4326");
    let csv = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(csv.lines().next().unwrap().starts_with("# crs=EPSG:4326"));
    assert!(csv.lines().nth(1).unwrap().contains("lon,lat,ele_m"));
    let row: Vec<&str> = csv.lines().nth(2).unwrap().split(',').collect();
    let (lon, lat, ele): (f64, f64, f64) =
        (row[2].parse().unwrap(), row[3].parse().unwrap(), row[4].parse().unwrap());
    let proj = Projection::new(ORIGIN_LAT, ORIGIN_LON, ORIGIN_ELE);
    let (exp_lat, exp_lon, exp_ele) = proj.local_to_wgs84(100.0, 10.0, 0.0);
    assert!((lat - exp_lat).abs() < 1e-4, "lat {lat} vs {exp_lat}");
    assert!((lon - exp_lon).abs() < 1e-4, "lon {lon} vs {exp_lon}");
    assert!((ele - exp_ele).abs() < 1e-2, "ele {ele} vs {exp_ele}");
    assert!((lat - ORIGIN_LAT).abs() > 1e-7 || (lon - ORIGIN_LON).abs() > 1e-7,
        "coordinates were not actually converted");

    // Parquet latlon carries EPSG:4326 geo metadata.
    let resp = export_parquet(
        State(state.clone()),
        Extension(claims.clone()),
        Path(pa.clone()),
        Query(params(None, Some("latlon"))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let geo = read_parquet_geo_meta(&body_bytes(resp).await);
    assert_eq!(geo["columns"]["position"]["crs"], "EPSG:4326");

    // Bad coords value → 400.
    let resp = export_csv(
        State(state.clone()),
        Extension(claims),
        Path(pa),
        Query(params(None, Some("weird"))),
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
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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
    let guarded = fe_api::query_guard::GuardedQuery { sql: "SELECT * FROM node".into() };
    let vars = std::collections::HashMap::new();

    // Under the cap: fine.
    let ok = fe_api::query_guard::run_guarded_query(&db, &guarded, &vars, 3).await;
    assert_eq!(ok.unwrap().len(), 3);

    // Over the cap: clear error (documented choice: error, not truncate).
    let err = fe_api::query_guard::run_guarded_query(&db, &guarded, &vars, 2).await.unwrap_err();
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
    };
    let resp = fe_api::rest::execute_query(State(state), Extension(claims), Json(req))
        .await
        .into_response();
    let body = body_json(resp).await;
    assert!(body["ok"].as_bool().unwrap(), "{body}");
    let crs = body["data"]["crs"].as_str().expect("crs field on /query envelope");
    assert!(crs.contains("origin=47.6062"), "{crs}");
    assert_eq!(body["data"]["data"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Phase 3 — signed shareable URLs
// ---------------------------------------------------------------------------

fn share_req(sql: &str, format: &str, ttl: Option<u64>) -> ShareRequest {
    ShareRequest { sql: sql.to_string(), format: format.to_string(), ttl_secs: ttl }
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
    assert_eq!(body["data"]["scope"].as_str().unwrap(), claims.scope, "ceiling = issuer scope");

    // Redeem WITHOUT any session: scope ceiling caps the result to petal A.
    let resp = redeem_share_url(State(state.clone()), Path(token.clone()), Query(RedeemParams::default())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body["data"]["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "narrow-scope URL must not read outside its scope: {body}");
    assert_eq!(rows[0]["node_id"], "node-in-a");
    assert!(body["data"]["crs"].as_str().is_some(), "shared JSON carries CRS");

    // Parquet share link: redeemed export is pre-filtered to the ceiling.
    let resp = issue_share_url(
        State(state.clone()),
        Extension(claims),
        Json(share_req("SELECT * FROM node", "parquet", None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let token = body_json(resp).await["data"]["token"].as_str().unwrap().to_string();
    let resp = redeem_share_url(State(state), Path(token), Query(RedeemParams::default())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, "content-type"), "application/vnd.apache.parquet");
    let snaps = read_parquet_back(&body_bytes(resp).await);
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].node_id, "node-in-a");
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
    let resp = redeem_share_url(State(state.clone()), Path(token), Query(RedeemParams::default())).await;
    assert_eq!(resp.status(), StatusCode::GONE);

    // Token signed by a DIFFERENT key (i.e. tampered/forged) → 401.
    let mut valid = expired.clone();
    valid.exp = u64::MAX;
    let foreign = fe_identity::NodeKeypair::generate();
    let forged = mint_share_token(&foreign, &valid).unwrap();
    let resp = redeem_share_url(State(state.clone()), Path(forged), Query(RedeemParams::default())).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Garbage token → 401.
    let resp = redeem_share_url(State(state.clone()), Path("garbage".into()), Query(RedeemParams::default())).await;
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
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{sql} / {fmt} / {ttl:?}");
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
