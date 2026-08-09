use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_api::rest::execute_analytics_query;
use fe_api::server::ApiState;
use fe_api::types::AnalyticsQueryRequest;
use fe_entity_store::{EntitySnapshot, EntityStore};
use fe_identity::api_token::ApiClaims;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

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

fn claims(scope: &str, role: &str) -> ApiClaims {
    ApiClaims {
        sub: "did:key:z6MkAnalyticsTester".to_string(),
        scope: scope.to_string(),
        max_role: role.to_string(),
        token_type: "api".to_string(),
        iat: 0,
        exp: u64::MAX,
        jti: "analytics-jti".to_string(),
    }
}

fn state(db: Option<Db>, store: Arc<EntityStore>) -> Arc<ApiState> {
    let (api_cmd_tx, _rx) = crossbeam::channel::bounded(1);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);

    Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: None,
        cors_origins: vec![],
        db_reader: db.map(Arc::new),
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: Some(store),
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    })
}

async fn seed_petal(db: &Db, petal_id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE verse CONTENT { verse_id: 'v1', name: 'V', created_by: 'did:key:owner', created_at: $now }",
    )
    .bind(("now", now.clone()))
    .await
    .unwrap()
    .check()
    .unwrap();
    db.query(
        "CREATE fractal CONTENT { fractal_id: 'f1', verse_id: 'v1', owner_did: 'did:key:owner', name: 'F', created_at: $now }",
    )
    .bind(("now", now.clone()))
    .await
    .unwrap()
    .check()
    .unwrap();
    db.query(
        "CREATE petal CONTENT { petal_id: $pid, fractal_id: 'f1', name: 'P', node_id: 'anchor', created_at: $now }",
    )
    .bind(("pid", petal_id.to_string()))
    .bind(("now", now))
    .await
    .unwrap()
    .check()
    .unwrap();
}

fn snapshot(node_id: &str, petal_id: &str) -> EntitySnapshot {
    EntitySnapshot {
        node_id: node_id.to_string(),
        petal_id: petal_id.to_string(),
        position: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0],
        scale: [1.0, 1.0, 1.0],
        properties: None,
        updated_at_ms: 0,
        node_log: vec![],
    }
}

async fn body_json(response: Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn analytics_scopes_the_registered_table_to_the_authorized_petal() {
    let db = setup_test_db().await;
    let petal_id = ulid::Ulid::new().to_string();
    seed_petal(&db, &petal_id).await;

    let store = Arc::new(EntityStore::new());
    store.upsert(snapshot("allowed", &petal_id));
    store.upsert(snapshot("foreign", &ulid::Ulid::new().to_string()));
    let state = state(Some(db), store);

    let response = execute_analytics_query(
        State(state),
        Extension(claims("VERSE#v1", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT node_id FROM nodes ORDER BY node_id".to_string(),
            petal_id: petal_id.clone(),
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;

    assert_eq!(body["ok"], true, "analytics response: {body}");
    assert_eq!(
        body["data"]["data"],
        serde_json::json!([{ "node_id": "allowed" }])
    );
}

#[tokio::test]
async fn analytics_rejects_a_petal_outside_the_token_scope() {
    let db = setup_test_db().await;
    let petal_id = ulid::Ulid::new().to_string();
    seed_petal(&db, &petal_id).await;
    let state = state(Some(db), Arc::new(EntityStore::new()));

    let response = execute_analytics_query(
        State(state),
        Extension(claims("VERSE#other", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT node_id FROM nodes".to_string(),
            petal_id,
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;

    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "insufficient scope");
}

#[tokio::test]
async fn analytics_fails_closed_without_a_direct_scope_reader() {
    let petal_id = ulid::Ulid::new().to_string();
    let state = state(None, Arc::new(EntityStore::new()));

    let response = execute_analytics_query(
        State(state),
        Extension(claims("VERSE#v1", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT node_id FROM nodes".to_string(),
            petal_id,
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;

    assert_eq!(body["ok"], false);
    assert_eq!(
        body["error"],
        "analytics authorization unavailable (no direct DB reader)"
    );
}

#[test]
fn analytics_request_requires_a_petal_id() {
    assert!(
        serde_json::from_value::<AnalyticsQueryRequest>(serde_json::json!({
            "sql": "SELECT node_id FROM nodes"
        }))
        .is_err()
    );
}
