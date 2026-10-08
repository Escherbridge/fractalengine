//! Integration tests for the M2/F7 distributed-query surfaces (A17): the
//! `/api/v1/query` structured `distributed` mode, the analytics endpoint's
//! merged `iot_reading` table, and the MCP `query_timeseries` tool.
//!
//! The seam boundary under test is `ApiState.distributed_tx`: these tests own
//! the matching receiver and answer `DistributedQueryCall`s the way a sync
//! thread would — replying a canned merged outcome and asserting the exact
//! guarded request the surface approved (role, petal scope resolution +
//! containment, verse derived from the resolved scope, spec/timeout/row-cap).
//! The REAL sync-side fan-out, merge, and honesty metadata are proven by the
//! P2P scenario runner (`fe-test-harness/src/scenarios/distributed_query.rs`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::json;

use fe_api::rest::execute_analytics_query;
use fe_api::server::ApiState;
use fe_api::types::AnalyticsQueryRequest;
use fe_entity_store::EntityStore;
use fe_identity::api_token::ApiClaims;
use fe_runtime::distributed_query::{
    DistributedQueryCall, DistributedQueryCallSender, DistributedQueryMeta,
    DistributedQueryOutcome, DistributedQueryRequest, TsQueryKind,
};

use fractalengine_test_harness::api::ApiHarness;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

// ---------------------------------------------------------------------------
// The fake sync side of the seam
// ---------------------------------------------------------------------------

/// A canned merged outcome: two raw rows plus A16-shaped honesty metadata
/// (one covered shard, one missing shard, one silent host, sharded R=1).
fn canned_outcome() -> DistributedQueryOutcome {
    DistributedQueryOutcome {
        rows: vec![
            json!({
                "reading_id": "reading-1",
                "node_id": "anchor-a",
                "metric": "temperature_c",
                "value": 20.0,
                "units": "C",
                "recorded_at_ms": 1_000,
                "hlc_timestamp": 1,
            }),
            json!({
                "reading_id": "reading-2",
                "node_id": "anchor-a",
                "metric": "temperature_c",
                "value": 22.0,
                "units": "C",
                "recorded_at_ms": 1_500,
                "hlc_timestamp": 2,
            }),
        ],
        meta: DistributedQueryMeta {
            covered_shards: vec!["petal/anchor-a/0".into()],
            missing_shards: vec!["petal/anchor-b/0".into()],
            answered_hosts: vec!["did:key:host-a".into()],
            missing_hosts: vec!["did:key:host-b".into()],
            mode: "sharded".into(),
            replication_factor: 1,
            truncated: false,
        },
        error: None,
    }
}

/// An honest EMPTY outcome (nothing covered, one shard missing its only
/// host) — the shape an all-hosts-offline fan-out returns.
fn empty_outcome() -> DistributedQueryOutcome {
    DistributedQueryOutcome {
        rows: Vec::new(),
        meta: DistributedQueryMeta {
            covered_shards: Vec::new(),
            missing_shards: vec!["petal/anchor-b/0".into()],
            answered_hosts: Vec::new(),
            missing_hosts: vec!["did:key:host-b".into()],
            mode: "sharded".into(),
            replication_factor: 1,
            truncated: false,
        },
        error: None,
    }
}

/// Records every guarded request the seam received.
struct FakeSeam {
    requests: Arc<Mutex<Vec<DistributedQueryRequest>>>,
}

impl FakeSeam {
    fn recorded(&self) -> Vec<DistributedQueryRequest> {
        self.requests.lock().unwrap().clone()
    }
}

/// Spawn the fake sync side of the seam: own the call receiver, record every
/// request, and reply `reply` to each (the thread exits when the harness and
/// its sender drop at test end).
fn fake_sync_seam(reply: DistributedQueryOutcome) -> (FakeSeam, DistributedQueryCallSender) {
    let (tx, rx) = crossbeam::channel::bounded::<DistributedQueryCall>(4);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        while let Ok(call) = rx.recv() {
            recorded.lock().unwrap().push(call.request);
            let _ = call.reply.send(reply.clone());
        }
    });
    (FakeSeam { requests }, tx)
}

// ---------------------------------------------------------------------------
// /api/v1/query distributed mode (full router, real auth middleware)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_distributed_mode_returns_merged_rows_and_honesty_meta() {
    let (seam, tx) = fake_sync_seam(canned_outcome());
    let h = ApiHarness::spawn_with_distributed_tx(Some(tx))
        .await
        .expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "distributed": {
                    "kind": "readings_in_window",
                    "metric": "temperature_c",
                    "start_ms": 1_000,
                    "end_ms": 2_000,
                    "petal_id": seeded.petal_id,
                }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true, "query response: {body}");
    assert_eq!(body["data"]["data"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["data"]["data"][0]["reading_id"], "reading-1");
    assert_eq!(body["data"]["data"][1]["reading_id"], "reading-2");
    // The A16 honesty metadata rides the response verbatim.
    assert_eq!(
        body["data"]["distributed"]["covered_shards"][0],
        "petal/anchor-a/0"
    );
    assert_eq!(
        body["data"]["distributed"]["missing_shards"][0],
        "petal/anchor-b/0"
    );
    assert_eq!(
        body["data"]["distributed"]["answered_hosts"][0],
        "did:key:host-a"
    );
    assert_eq!(
        body["data"]["distributed"]["missing_hosts"][0],
        "did:key:host-b"
    );
    assert_eq!(body["data"]["distributed"]["mode"], "sharded");
    assert_eq!(body["data"]["distributed"]["replication_factor"], 1);
    assert_eq!(body["data"]["distributed"]["truncated"], false);

    // The guarded request the seam saw: the verse is derived from the
    // resolved petal scope (never the body — it has no verse field at all),
    // and spec/timeout/row-cap are the surface's constants.
    let recorded = seam.recorded();
    assert_eq!(recorded.len(), 1, "exactly one fan-out per request");
    let req = &recorded[0];
    assert_eq!(req.verse_id, seeded.verse_id);
    assert_eq!(
        req.spec,
        TsQueryKind::ReadingsInWindow {
            metric: "temperature_c".into(),
            start_ms: 1_000,
            end_ms: 2_000,
            petal_id: seeded.petal_id.clone(),
        }
    );
    assert_eq!(
        req.timeout_ms,
        fe_api::timeseries_query::DISTRIBUTED_QUERY_TIMEOUT_MS
    );
    assert_eq!(
        req.row_cap,
        fe_api::timeseries_query::DISTRIBUTED_QUERY_ROW_CAP
    );
    assert!(!req.request_id.is_empty());
}

#[tokio::test]
async fn query_distributed_mode_rejects_sql_and_spec_together() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "sql": "SELECT * FROM node",
                "distributed": { "kind": "all_readings", "petal_id": seeded.petal_id }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "query response: {body}");
    assert_eq!(
        body["error"],
        "sql and distributed are mutually exclusive — send exactly one"
    );
}

#[tokio::test]
async fn query_distributed_mode_denies_a_petal_outside_the_token_scope() {
    let (seam, tx) = fake_sync_seam(canned_outcome());
    let h = ApiHarness::spawn_with_distributed_tx(Some(tx))
        .await
        .expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    // A valid token for a DIFFERENT verse: the petal resolves, but the token
    // does not cover it — deny-by-default, exactly like every petal-scoped
    // read, and no fan-out ever leaves the process.
    let other = format!("VERSE#{}", ulid::Ulid::new());
    let token = h.mint_token(&other, "viewer");

    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "distributed": { "kind": "all_readings", "petal_id": seeded.petal_id }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "query response: {body}");
    assert_eq!(body["error"], "insufficient scope");
    assert!(
        seam.recorded().is_empty(),
        "a denied request must never reach the seam"
    );
}

#[tokio::test]
async fn query_distributed_mode_rejects_an_unknown_petal() {
    let (seam, tx) = fake_sync_seam(canned_outcome());
    let h = ApiHarness::spawn_with_distributed_tx(Some(tx))
        .await
        .expect("spawn harness");
    let token = h.mint_token("VERSE#anything", "viewer");

    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "distributed": {
                    "kind": "all_readings",
                    "petal_id": ulid::Ulid::new().to_string()
                }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "query response: {body}");
    assert_eq!(body["error"], "unknown petal");
    assert!(seam.recorded().is_empty());
}

#[tokio::test]
async fn query_distributed_mode_requires_the_seam() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "distributed": { "kind": "all_readings", "petal_id": seeded.petal_id }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "query response: {body}");
    assert_eq!(
        body["error"],
        "distributed query transport not configured (no sync-thread seam)"
    );
}

#[tokio::test]
async fn query_distributed_mode_maps_a_dead_seam_to_unavailable() {
    // A dropped receiver is "the sync thread is gone": the send fails and
    // the surface reports the transport, never a hang.
    let (tx, rx) = crossbeam::channel::bounded::<DistributedQueryCall>(1);
    drop(rx);
    let h = ApiHarness::spawn_with_distributed_tx(Some(tx))
        .await
        .expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "distributed": { "kind": "all_readings", "petal_id": seeded.petal_id }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "query response: {body}");
    assert_eq!(body["error"], "distributed query transport unavailable");
}

#[tokio::test]
async fn query_distributed_mode_refuses_promptly_when_the_seam_is_saturated() {
    // The saturated-seam shape (M2 scrutiny round-1 blocker, fixed F23):
    // the bounded(64) seam is FULL and the consumer (the sync thread) is
    // alive but wedged — it never drains. A blocking send here would pin
    // this handler (and, request by request, the API workers) indefinitely;
    // try_send must refuse PROMPTLY with the honest queue-full error.
    const SEAM_CAP: usize = 64;
    let (tx, rx) = crossbeam::channel::bounded::<DistributedQueryCall>(SEAM_CAP);
    for i in 0..SEAM_CAP {
        let (reply_tx, _reply_rx) = crossbeam::channel::bounded(1);
        let call = DistributedQueryCall {
            request: DistributedQueryRequest {
                request_id: format!("wedged-{i}"),
                verse_id: "verse-wedged".into(),
                spec: TsQueryKind::AllReadings {
                    petal_id: "petal-wedged".into(),
                },
                timeout_ms: 1_000,
                row_cap: 0,
            },
            reply: reply_tx,
        };
        tx.send(call).expect("fill the seam to capacity");
    }
    let h = ApiHarness::spawn_with_distributed_tx(Some(tx))
        .await
        .expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let started = std::time::Instant::now();
    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(&token),
            &json!({
                "distributed": { "kind": "all_readings", "petal_id": seeded.petal_id }
            }),
        )
        .await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "query response: {body}");
    assert_eq!(
        body["error"],
        "distributed query transport is saturated — too many queued fan-outs, try again shortly"
    );
    // Prompt refusal: far under the 10 s the reply wait alone would take,
    // and a blocking send would never return at all (the consumer is wedged).
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the saturated seam must refuse promptly, took {elapsed:?}"
    );
    drop(rx); // the wedged consumer stays alive until every assertion ran
}

// ---------------------------------------------------------------------------
// MCP query_timeseries tool (full router, real auth middleware)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_query_timeseries_returns_the_merged_outcome() {
    let (seam, tx) = fake_sync_seam(canned_outcome());
    let h = ApiHarness::spawn_with_distributed_tx(Some(tx))
        .await
        .expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let (status, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": { "name": "query_timeseries", "arguments": {
                    "petal_id": seeded.petal_id,
                    "kind": "window_aggregate",
                    "metric": "temperature_c",
                    "start_ms": 1_000,
                    "end_ms": 2_000
                }}
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["result"]["isError"].is_null(), "tool errored: {body}");
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    let payload: serde_json::Value = serde_json::from_str(text).expect("outcome JSON");
    assert_eq!(payload, serde_json::to_value(canned_outcome()).unwrap());

    // The tool built the structured spec from its flat args; the seam saw
    // the same window aggregate with the scope-derived verse.
    let recorded = seam.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].verse_id, seeded.verse_id);
    assert_eq!(
        recorded[0].spec,
        TsQueryKind::WindowAggregate {
            metric: "temperature_c".into(),
            start_ms: 1_000,
            end_ms: 2_000,
            petal_id: seeded.petal_id.clone(),
        }
    );
}

#[tokio::test]
async fn mcp_query_timeseries_rejects_incomplete_args() {
    // No seam is wired: the argument check must fire BEFORE any transport.
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "viewer");

    let (status, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({
                "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": { "name": "query_timeseries", "arguments": {
                    "petal_id": seeded.petal_id,
                    "kind": "window_aggregate"
                }}
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], true, "tool response: {body}");
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    assert!(
        text.contains("invalid arguments"),
        "want `invalid arguments` in `{text}`"
    );
}

// ---------------------------------------------------------------------------
// Analytics merged iot_reading table (direct handler, like
// analytics_scope_test.rs — the harness has no entity_store)
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

fn claims(scope: &str, role: &str) -> ApiClaims {
    ApiClaims {
        sub: "did:key:z6MkDistributedQueryTester".to_string(),
        scope: scope.to_string(),
        max_role: role.to_string(),
        token_type: "api".to_string(),
        iat: 0,
        exp: u64::MAX,
        jti: "distributed-query-jti".to_string(),
    }
}

fn analytics_state(
    db: Option<Db>,
    store: Arc<EntityStore>,
    distributed_tx: Option<DistributedQueryCallSender>,
) -> Arc<ApiState> {
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
        replication_tx: None,
        distributed_tx,
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

async fn body_json(response: Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn analytics_serves_the_merged_distributed_readings_table() {
    let db = setup_test_db().await;
    let petal_id = ulid::Ulid::new().to_string();
    seed_petal(&db, &petal_id).await;
    let (seam, tx) = fake_sync_seam(canned_outcome());
    let state = analytics_state(Some(db), Arc::new(EntityStore::new()), Some(tx));

    let response = execute_analytics_query(
        State(state),
        Extension(claims("VERSE#v1", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT count(*) AS n, sum(value) AS total FROM iot_reading".to_string(),
            petal_id: petal_id.clone(),
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;

    assert_eq!(body["ok"], true, "analytics response: {body}");
    assert_eq!(body["data"]["data"][0]["n"], 2);
    assert_eq!(body["data"]["data"][0]["total"], 42.0);
    // The honesty metadata rides the analytics surface too.
    assert_eq!(
        body["data"]["distributed"]["covered_shards"][0],
        "petal/anchor-a/0"
    );
    assert_eq!(
        body["data"]["distributed"]["missing_shards"][0],
        "petal/anchor-b/0"
    );

    // An iot_reading reference always fans out the WHOLE-PETAL view
    // (AllReadings) with the scope-derived verse — never a local-shards
    // subset presented as "the readings table".
    let recorded = seam.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].verse_id, "v1");
    assert_eq!(
        recorded[0].spec,
        TsQueryKind::AllReadings {
            petal_id: petal_id.clone()
        }
    );
}

#[tokio::test]
async fn analytics_registers_an_honest_empty_readings_table() {
    let db = setup_test_db().await;
    let petal_id = ulid::Ulid::new().to_string();
    seed_petal(&db, &petal_id).await;
    let (seam, tx) = fake_sync_seam(empty_outcome());
    let state = analytics_state(Some(db), Arc::new(EntityStore::new()), Some(tx));

    let response = execute_analytics_query(
        State(state),
        Extension(claims("VERSE#v1", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT count(*) AS n FROM iot_reading".to_string(),
            petal_id,
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;

    // An empty fan-out registers an honest ZERO-row table (never a parse
    // failure that hides the emptiness) and still carries the metadata that
    // says one shard is missing.
    assert_eq!(body["ok"], true, "analytics response: {body}");
    assert_eq!(body["data"]["data"][0]["n"], 0);
    assert_eq!(
        body["data"]["distributed"]["missing_shards"][0],
        "petal/anchor-b/0"
    );
    assert_eq!(
        body["data"]["distributed"]["covered_shards"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(seam.recorded().len(), 1);
}

#[tokio::test]
async fn analytics_readings_reference_fails_explicitly_without_the_seam() {
    let db = setup_test_db().await;
    let petal_id = ulid::Ulid::new().to_string();
    seed_petal(&db, &petal_id).await;
    let state = analytics_state(Some(db), Arc::new(EntityStore::new()), None);

    // There is deliberately NO local fallback: a sharded-verse deployment
    // presenting local-only rows as "the readings table" would be the
    // dishonest surface — the reference fails explicitly instead.
    let response = execute_analytics_query(
        State(state.clone()),
        Extension(claims("VERSE#v1", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT count(*) AS n FROM iot_reading".to_string(),
            petal_id: petal_id.clone(),
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;
    assert_eq!(body["ok"], false, "analytics response: {body}");
    assert_eq!(
        body["error"],
        "distributed query transport not configured (no sync-thread seam)"
    );

    // The same surface WITHOUT an iot_reading reference is untouched by the
    // seam's absence — nodes-only analytics still answers.
    let response = execute_analytics_query(
        State(state),
        Extension(claims("VERSE#v1", "viewer")),
        Json(AnalyticsQueryRequest {
            sql: "SELECT count(*) AS n FROM nodes".to_string(),
            petal_id,
        }),
    )
    .await
    .into_response();
    let body = body_json(response).await;
    assert_eq!(body["ok"], true, "analytics response: {body}");
    assert_eq!(body["data"]["data"][0]["n"], 0);
}
