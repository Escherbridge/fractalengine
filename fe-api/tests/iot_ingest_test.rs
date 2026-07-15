//! Integration tests for the IoT ingestion endpoint (`fe-api/src/iot.rs`,
//! FR-4 of iot_spatial_reporting_20260714) and the query_guard seam that
//! exposes `iot_reading` to `/query` (FR-5). Mirrors the in-memory SurrealDB
//! idiom of `export_share_test.rs`. READ-BACK assertions throughout.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use fe_api::iot::{ingest_readings, IotIngestRequest};
use fe_api::server::ApiState;
use fe_api::{limits, query_guard};
use fe_database::handlers::iot_reading::IotReadingInput;
use fe_identity::api_token::ApiClaims;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

// ---------------------------------------------------------------------------
// Fixtures (export_share_test idiom)
// ---------------------------------------------------------------------------

async fn setup_test_db() -> Db {
    // The write handler packs HLC timestamps; production init happens during
    // DB startup, which this raw in-memory setup bypasses.
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
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    })
}

async fn seed_petal(db: &Db, petal_id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    let _ = db
        .query("CREATE verse CONTENT { verse_id: 'v1', name: 'V', created_by: 'did:key:z6MkOwner', created_at: $now }")
        .bind(("now", now.clone()))
        .await;
    let _ = db
        .query("CREATE fractal CONTENT { fractal_id: 'f1', verse_id: 'v1', owner_did: 'did:key:z6MkOwner', name: 'F', created_at: $now }")
        .bind(("now", now.clone()))
        .await;
    db.query(
        "CREATE petal CONTENT { petal_id: $pid, fractal_id: 'f1', name: 'P', \
         node_id: 'anchor-node', created_at: $now }",
    )
    .bind(("pid", petal_id.to_string()))
    .bind(("now", now))
    .await
    .unwrap()
    .check()
    .unwrap();
}

/// Seed a node (geometry cast per fe-database/src/AGENTS.md §geometry-inserts).
async fn seed_node(db: &Db, petal_id: &str, node_id: &str, x: f64, z: f64) {
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE node CONTENT { \
         node_id: $nid, petal_id: $pid, display_name: $name, \
         position: <geometry<point>> [$x, $z], elevation: 0.0, \
         rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
         interactive: false, created_at: $now }",
    )
    .bind(("nid", node_id.to_string()))
    .bind(("pid", petal_id.to_string()))
    .bind(("name", format!("node-{node_id}")))
    .bind(("x", x))
    .bind(("z", z))
    .bind(("now", now))
    .await
    .unwrap()
    .check()
    .unwrap();
}

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

fn reading(node_id: &str, metric: &str, value: f64, recorded_at: Option<&str>) -> IotReadingInput {
    IotReadingInput {
        node_id: node_id.to_string(),
        metric: metric.to_string(),
        value,
        units: "C".to_string(),
        recorded_at: recorded_at.map(str::to_string),
    }
}

async fn post_readings(
    state: &Arc<ApiState>,
    claims: ApiClaims,
    petal_id: &str,
    readings: Vec<IotReadingInput>,
) -> Response {
    ingest_readings(
        State(state.clone()),
        Extension(claims),
        Path(petal_id.to_string()),
        Json(IotIngestRequest { readings }),
    )
    .await
    .into_response()
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn count_readings(db: &Db) -> usize {
    let mut res = db.query("SELECT * FROM iot_reading").await.unwrap();
    res.take::<Vec<serde_json::Value>>(0).unwrap().len()
}

// ---------------------------------------------------------------------------
// FR-4 — ingestion endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_batch_persists_and_reads_back() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 1.0, 2.0).await;
    let state = test_state(db);

    let resp = post_readings(
        &state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![
            reading(
                "sensor-1",
                "temperature_c",
                21.5,
                Some("2026-07-15T10:00:00Z"),
            ),
            reading(
                "sensor-1",
                "temperature_c",
                22.5,
                Some("2026-07-15T11:00:00Z"),
            ),
        ],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["accepted"], 2);

    // READ-BACK straight from the table.
    let db = state.db_reader.as_ref().unwrap();
    let mut res = db
        .query("SELECT * FROM iot_reading ORDER BY recorded_at_ms ASC")
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = res.take(0).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["node_id"], "sensor-1");
    assert_eq!(rows[0]["petal_id"], pa.as_str());
    assert_eq!(rows[0]["metric"], "temperature_c");
    assert_eq!(rows[0]["value"].as_f64(), Some(21.5));
    assert_eq!(rows[0]["units"], "C");
    assert_eq!(rows[0]["source_did"], "did:key:z6MkUser");
    assert_eq!(rows[1]["value"].as_f64(), Some(22.5));
}

#[tokio::test]
async fn ingest_rejects_wrong_scope_role_and_foreign_anchor() {
    let db = setup_test_db().await;
    let pa = ulid();
    let pb = ulid();
    seed_petal(&db, &pa).await;
    seed_petal(&db, &pb).await;
    seed_node(&db, &pa, "sensor-a", 0.0, 0.0).await;
    seed_node(&db, &pb, "sensor-b", 0.0, 0.0).await;
    let state = test_state(db);
    let batch = || vec![reading("sensor-a", "temperature_c", 20.0, None)];

    // Wrong scope (different verse) → 403.
    let resp = post_readings(&state, test_claims("VERSE#v2", "editor"), &pa, batch()).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Viewer role (read-only) → 403.
    let resp = post_readings(&state, test_claims("VERSE#v1", "viewer"), &pa, batch()).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Anchor node belongs to petal B → 422, nothing persisted.
    let resp = post_readings(
        &state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![reading("sensor-b", "temperature_c", 20.0, None)],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(count_readings(state.db_reader.as_ref().unwrap()).await, 0);
}

#[tokio::test]
async fn ingest_enforces_batch_cap_and_rejects_empty() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 0.0, 0.0).await;
    let state = test_state(db);

    // Over the batch cap → 413.
    let big: Vec<_> = (0..=limits::IOT_INGEST_MAX_READINGS)
        .map(|i| reading("sensor-1", "temperature_c", i as f64, None))
        .collect();
    let resp = post_readings(&state, test_claims("VERSE#v1", "editor"), &pa, big).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // Empty batch → 400.
    let resp = post_readings(&state, test_claims("VERSE#v1", "editor"), &pa, vec![]).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    assert_eq!(count_readings(state.db_reader.as_ref().unwrap()).await, 0);
}

// ---------------------------------------------------------------------------
// FR-5 seam — iot_reading rows visible to the guarded /query pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn guarded_query_reads_iot_rows_scope_filtered() {
    let db = setup_test_db().await;
    let pa = ulid();
    let pb = ulid();
    seed_petal(&db, &pa).await;
    seed_petal(&db, &pb).await;
    seed_node(&db, &pa, "sensor-a", 0.0, 0.0).await;
    seed_node(&db, &pb, "sensor-b", 0.0, 0.0).await;
    let state = test_state(db);

    for (petal, sensor, val) in [(&pa, "sensor-a", 1.0), (&pb, "sensor-b", 2.0)] {
        let resp = post_readings(
            &state,
            test_claims("VERSE#v1", "editor"),
            petal,
            vec![reading(sensor, "temperature_c", val, None)],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // A petal-A-scoped token querying iot_reading through the guard pipeline
    // must never see petal B's readings.
    let scope = format!("VERSE#v1-FRACTAL#f1-PETAL#{pa}");
    let guarded = query_guard::guard_and_prepare_query(
        &state,
        "test-query",
        limits::QUERY_RATE_PER_SEC,
        "10 queries/sec",
        &scope,
        "SELECT * FROM iot_reading",
    )
    .await
    .expect("guard pipeline accepts iot_reading");
    let db = state.db_reader.as_ref().unwrap();
    let rows = query_guard::run_guarded_query(
        db,
        &guarded,
        &std::collections::HashMap::new(),
        limits::QUERY_ROW_CAP,
    )
    .await
    .expect("guarded query runs");

    assert_eq!(rows.len(), 1, "petal-B reading must be scope-filtered out");
    assert_eq!(rows[0]["petal_id"], pa.as_str());
    assert_eq!(rows[0]["value"].as_f64(), Some(1.0));
}
