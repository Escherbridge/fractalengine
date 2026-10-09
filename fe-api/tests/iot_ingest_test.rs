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
use fe_runtime::messages::{DbCommand, DbResult};

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
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
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

// ---------------------------------------------------------------------------
// A11 — the API-path replication emit seam
// ---------------------------------------------------------------------------

/// Same state as [`test_state`] but with the A11 emit seam wired: a content
/// blob store (row bytes → content hash) and the DB→sync replication sender.
fn test_state_with_replication(
    db: Db,
) -> (
    Arc<ApiState>,
    crossbeam::channel::Receiver<fe_database::ReplicationEvent>,
) {
    let (api_cmd_tx, _rx) = crossbeam::channel::bounded(1);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(8);
    // Derive the verify key from a fresh node keypair rather than naming the
    // curve crate directly; this state never verifies a token anyway.
    let keypair = fe_identity::NodeKeypair::generate();
    let verifying_key = keypair.verifying_key();

    let state = Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key,
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: Some(Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new())),
        cors_origins: vec![],
        db_reader: Some(Arc::new(db)),
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        replication_tx: Some(repl_tx),
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    });
    (state, repl_rx)
}

/// A11: the `db_reader` ingestion path publishes one `ReplicationEvent` per
/// accepted row, each naming the verse (derived from the resolved petal scope)
/// and the petal, keyed on that row's `reading_id`.
#[tokio::test]
async fn ingest_publishes_one_replication_event_per_row() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 1.0, 2.0).await;
    let (state, repl_rx) = test_state_with_replication(db.clone());

    let resp = post_readings(
        &state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![
            reading("sensor-1", "temperature_c", 21.5, None),
            reading("sensor-1", "humidity_pct", 55.0, None),
        ],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["accepted"], 2);

    // READ-BACK: the rows are durable before the events that describe them.
    assert_eq!(count_readings(&db).await, 2);

    let mut events = Vec::new();
    while let Ok(evt) = repl_rx.try_recv() {
        events.push(evt);
    }
    assert_eq!(events.len(), 2, "one event per accepted reading");
    for evt in &events {
        assert_eq!(evt.verse_id, "v1", "the event names the verse replica");
        assert_eq!(evt.table, "iot_reading");
        assert_eq!(evt.petal_id.as_deref(), Some(pa.as_str()));
    }
    assert_ne!(
        events[0].record_id, events[1].record_id,
        "each row is keyed by its own reading_id"
    );
}

/// A11: without an emit seam wired (`replication_tx: None`, the harness
/// default) ingestion still succeeds and publishes nothing — the rows remain
/// durable, so a deployment with replication disabled loses no data.
#[tokio::test]
async fn ingest_without_replication_seam_still_persists_rows() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 1.0, 2.0).await;
    let state = test_state(db.clone());

    let resp = post_readings(
        &state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![reading("sensor-1", "temperature_c", 21.5, None)],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(count_readings(&db).await, 1);
}

// ---------------------------------------------------------------------------
// F24 — the DB-thread fallback when db_reader is absent
// ---------------------------------------------------------------------------

/// One command the fallback dispatcher observed, for guard-order assertions.
#[derive(Clone, Debug)]
enum ObservedCommand {
    /// A scope-resolution round-trip (expected before any write; its payload
    /// is irrelevant to the guard-order assertions).
    ScopeResolution,
    InsertIotReadings {
        petal_id: String,
        verse_id: Option<String>,
        source_did: String,
        readings: usize,
    },
}

/// What the test dispatcher does with an `InsertIotReadings` command.
enum InsertBehaviour {
    /// Execute the real DB-thread arm: run `insert_readings_with_replication`
    /// on the Mem DB and reply with the same typed mapping the arm uses
    /// (`IotReadingsInserted` / `IotReadingsRejected` / `Error`).
    Real,
    /// Reply with a fixed `DbResult` instead (simulates a DB/transport
    /// failure behind the seam).
    Fixed(DbResult),
}

struct FallbackHarness {
    state: Arc<ApiState>,
    repl_rx: crossbeam::channel::Receiver<fe_database::ReplicationEvent>,
    observed: Arc<std::sync::Mutex<Vec<ObservedCommand>>>,
    _dispatcher: std::thread::JoinHandle<()>,
}

/// Build the F24 fallback deployment: `db_reader: None` (the Windows
/// SurrealKV per-handle-lock posture) with the API→DB command channel
/// serviced by a dispatcher that mirrors the DB-thread arms the real
/// dispatch loop runs.
fn fallback_harness(db: Db, insert_behaviour: InsertBehaviour) -> FallbackHarness {
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(64);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(64);
    let keypair = fe_identity::NodeKeypair::generate();
    let verifying_key = keypair.verifying_key();

    let state = Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key,
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: None,
        cors_origins: vec![],
        // The F24 precondition: no direct reader (the deployment-platform
        // posture this feature exists to serve).
        db_reader: None,
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        // NOTE: None here — on the fallback path the emit seam belongs to the
        // DB thread (the dispatcher wires it below), not to ApiState.
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    });

    let observed: Arc<std::sync::Mutex<Vec<ObservedCommand>>> = Arc::default();
    let observed_for_thread = Arc::clone(&observed);
    let dispatcher_db = db.clone();
    let dispatcher = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("dispatcher runtime");
        while let Ok(fe_runtime::messages::ApiCommand::DbRequest { cmd, reply_tx }) =
            api_cmd_rx.recv()
        {
            match cmd {
                DbCommand::ResolvePetalScope { petal_id } => {
                    observed_for_thread
                        .lock()
                        .unwrap()
                        .push(ObservedCommand::ScopeResolution);
                    let scope = rt.block_on(async {
                        let mut res = dispatcher_db
                            .query("SELECT fractal_id FROM petal WHERE petal_id = $pid LIMIT 1")
                            .bind(("pid", petal_id.clone()))
                            .await
                            .expect("petal query");
                        let rows: Vec<serde_json::Value> = res.take(0).expect("petal rows");
                        let fractal_id = rows
                            .first()
                            .and_then(|r| r.get("fractal_id"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)?;
                        let mut res2 = dispatcher_db
                            .query("SELECT verse_id FROM fractal WHERE fractal_id = $fid")
                            .bind(("fid", fractal_id.clone()))
                            .await
                            .expect("fractal query");
                        let rows2: Vec<serde_json::Value> = res2.take(0).expect("fractal rows");
                        rows2
                            .first()
                            .and_then(|r| r.get("verse_id"))
                            .and_then(serde_json::Value::as_str)
                            .map(|verse_id| {
                                fe_database::build_scope(
                                    verse_id,
                                    Some(&fractal_id),
                                    Some(&petal_id),
                                )
                            })
                    });
                    // Mirrors the real arm: `scope: None` when unresolvable
                    // (the handler maps that to its 404/None path).
                    let _ = reply_tx.send(DbResult::ScopeResolved { scope });
                }
                DbCommand::InsertIotReadings {
                    petal_id,
                    verse_id,
                    source_did,
                    readings,
                } => {
                    observed_for_thread
                        .lock()
                        .unwrap()
                        .push(ObservedCommand::InsertIotReadings {
                            petal_id: petal_id.clone(),
                            verse_id: verse_id.clone(),
                            source_did: source_did.clone(),
                            readings: readings.len(),
                        });
                    let reply = match &insert_behaviour {
                        InsertBehaviour::Real => {
                            // Exactly the fe-database dispatch arm: durable
                            // first, one ReplicationEvent per accepted row,
                            // typed rejection for validation failures.
                            let blob: fe_runtime::blob_store::BlobStoreHandle =
                                Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
                            match rt.block_on(
                                fe_database::handlers::iot_reading::insert_readings_with_replication(
                                    &dispatcher_db,
                                    &petal_id,
                                    verse_id.as_deref(),
                                    &source_did,
                                    &readings,
                                    Some(&blob),
                                    Some(&repl_tx),
                                ),
                            ) {
                                Ok(written) => DbResult::IotReadingsInserted { petal_id, written },
                                Err(e) => match e.validation_rejection() {
                                    Some(reason) => {
                                        DbResult::IotReadingsRejected { petal_id, reason }
                                    }
                                    None => DbResult::Error(format!(
                                        "IoT readings ingest failed: {e}"
                                    )),
                                },
                            }
                        }
                        InsertBehaviour::Fixed(fixed) => fixed.clone(),
                    };
                    let _ = reply_tx.send(reply);
                }
                other => {
                    let _ = reply_tx.send(DbResult::Error(format!(
                        "unsupported in fallback harness: {other:?}"
                    )));
                }
            }
        }
    });

    FallbackHarness {
        state,
        repl_rx,
        observed,
        _dispatcher: dispatcher,
    }
}

fn observed_inserts(harness: &FallbackHarness) -> Vec<ObservedCommand> {
    harness
        .observed
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, ObservedCommand::InsertIotReadings { .. }))
        .cloned()
        .collect()
}

/// F24 success: with `db_reader` absent (the Windows per-handle-lock posture),
/// ingest rides the DB-thread seam, threads the acting caller's identity,
/// writes durably (READ-BACK), and fires one `ReplicationEvent` per accepted
/// row — no 503 anywhere.
#[tokio::test]
async fn fallback_ingest_without_db_reader_persists_and_emits_per_row() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 1.0, 2.0).await;
    let harness = fallback_harness(db.clone(), InsertBehaviour::Real);

    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![
            reading("sensor-1", "temperature_c", 21.5, None),
            reading("sensor-1", "humidity_pct", 55.0, None),
        ],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "no 503 on the fallback path");
    let body = body_json(resp).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["accepted"], 2);

    // The command carried the resolved petal, its verse (derived from the
    // resolved scope, never the request body), and the acting caller's DID.
    let inserts = observed_inserts(&harness);
    assert_eq!(inserts.len(), 1, "one DB-thread command per batch");
    match &inserts[0] {
        ObservedCommand::InsertIotReadings {
            petal_id,
            verse_id,
            source_did,
            readings,
        } => {
            assert_eq!(petal_id, &pa);
            assert_eq!(verse_id.as_deref(), Some("v1"));
            assert_eq!(source_did, "did:key:z6MkUser");
            assert_eq!(*readings, 2);
        }
        other => panic!("expected an InsertIotReadings observation, got {other:?}"),
    }

    // READ-BACK: the rows are durable.
    assert_eq!(count_readings(&db).await, 2);
    let mut res = db
        .query("SELECT * FROM iot_reading")
        .await
        .expect("read back rows");
    let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
    assert!(rows.iter().all(|r| r["source_did"] == "did:key:z6MkUser"));

    // READ-BACK: one ReplicationEvent per accepted row (the DB-thread arm
    // rides insert_readings_with_replication), each naming the verse + petal.
    let mut events = Vec::new();
    while let Ok(evt) = harness.repl_rx.try_recv() {
        events.push(evt);
    }
    assert_eq!(events.len(), 2, "one event per accepted reading");
    for evt in &events {
        assert_eq!(evt.verse_id, "v1");
        assert_eq!(evt.table, "iot_reading");
        assert_eq!(evt.petal_id.as_deref(), Some(pa.as_str()));
    }
    assert_ne!(events[0].record_id, events[1].record_id);
}

/// F24: the fallback fires ONLY when `db_reader` is `None` — with a reader
/// configured the direct path serves the ingest and no DB-thread command is
/// ever sent.
#[tokio::test]
async fn direct_path_with_db_reader_never_sends_the_fallback_command() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 0.0, 0.0).await;

    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(64);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
    let keypair = fe_identity::NodeKeypair::generate();
    let state = Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key: keypair.verifying_key(),
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
    });

    let resp = post_readings(
        &state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![reading("sensor-1", "temperature_c", 21.5, None)],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The channel stayed empty — the direct path is unchanged.
    assert!(
        matches!(
            api_cmd_rx.try_recv(),
            Err(crossbeam::channel::TryRecvError::Empty)
        ),
        "no DB-thread command may be sent while db_reader is present"
    );
}

/// F24: every guard denies on the fallback path BEFORE any write command is
/// sent — role floor, scope containment, unknown petal, and the per-DID rate
/// limit all behave exactly as on the direct path.
#[tokio::test]
async fn fallback_guards_deny_before_the_seam() {
    let db = setup_test_db().await;
    let pa = ulid();
    seed_petal(&db, &pa).await;
    seed_node(&db, &pa, "sensor-1", 0.0, 0.0).await;
    let harness = fallback_harness(db, InsertBehaviour::Real);
    let batch = || vec![reading("sensor-1", "temperature_c", 20.0, None)];

    // Viewer role → 403, and not even a scope-resolution command is sent
    // (the role floor precedes scope resolution).
    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v1", "viewer"),
        &pa,
        batch(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        harness.observed.lock().unwrap().is_empty(),
        "role denial happens before any channel traffic"
    );

    // Foreign-verse scope → 403; scope was resolved (one command) but NO
    // write reached the seam.
    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v2", "editor"),
        &pa,
        batch(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        observed_inserts(&harness).len(),
        0,
        "scope denial must never reach the write seam"
    );

    // Unknown petal → 404 (scope resolution honestly fails through the
    // channel), still no write.
    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v1", "editor"),
        &ulid(),
        batch(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(observed_inserts(&harness).len(), 0);

    // Rate limit → 429: the first ten batches of the second pass ingest,
    // then the guard refuses with the 11th — and no write command is sent
    // for the refused request.
    for _ in 0..limits::IOT_INGEST_RATE_PER_SEC {
        let resp = post_readings(
            &harness.state,
            test_claims("VERSE#v1", "editor"),
            &pa,
            batch(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let before = observed_inserts(&harness).len();
    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        batch(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        observed_inserts(&harness).len(),
        before,
        "the refused request never reached the write seam"
    );
}

/// F24: the fallback keeps the direct path's typed status mapping — a
/// validation failure returns 422 with byte-identical wording (the typed
/// rejection crossing the seam), and a DB failure returns 502.
#[tokio::test]
async fn fallback_maps_typed_rejection_to_422_and_db_failure_to_502() {
    // 422: a foreign anchor is a validation failure — the DB-thread arm's
    // typed rejection crosses the seam as IotReadingsRejected.
    let db = setup_test_db().await;
    let pa = ulid();
    let pb = ulid();
    seed_petal(&db, &pa).await;
    seed_petal(&db, &pb).await;
    seed_node(&db, &pb, "sensor-b", 0.0, 0.0).await;
    let harness = fallback_harness(db.clone(), InsertBehaviour::Real);

    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v1", "editor"),
        &pa,
        vec![reading("sensor-b", "temperature_c", 20.0, None)],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp).await;
    assert_eq!(
        body["error"], "unknown anchor node 'sensor-b' in this petal",
        "byte-parity with the direct path's IotIngestError wording"
    );
    assert_eq!(count_readings(&db).await, 0, "nothing persisted");

    // 502: a DB failure behind the seam maps to the same failure surface as
    // the direct path's IotIngestError::Db.
    let db2 = setup_test_db().await;
    let pc = ulid();
    seed_petal(&db2, &pc).await;
    seed_node(&db2, &pc, "sensor-1", 0.0, 0.0).await;
    let harness = fallback_harness(
        db2,
        InsertBehaviour::Fixed(DbResult::Error("storage exploded".into())),
    );
    let resp = post_readings(
        &harness.state,
        test_claims("VERSE#v1", "editor"),
        &pc,
        vec![reading("sensor-1", "temperature_c", 20.0, None)],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = body_json(resp).await;
    assert_eq!(body["error"], "reading write failed");
}
