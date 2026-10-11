//! `/api/v1/query/elevated` through the full router (fe-api/AGENTS.md
//! §elevated-query): operator opt-in, Owner-only, privilege tables
//! unreachable, and the script served by the production DB-thread handler —
//! single writer, atomic op-log, replication + scene broadcast, DB-side TIMEOUT.

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use fe_database::handlers::elevated_query::{elevated_query_reply, ElevatedRequest, ElevatedSinks};
use fe_database::ReplicationEvent;
use fe_runtime::app::PendingApiRequests;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, SceneChange};
use fractalengine_test_harness::api::ApiHarness;
use serde_json::json;

const ELEVATED: &str = "/api/v1/query/elevated";

/// What the simulated DB thread emitted besides its replies.
struct DbThreadOutputs {
    replication: crossbeam::channel::Receiver<ReplicationEvent>,
    scene: tokio::sync::broadcast::Receiver<SceneChange>,
}

/// Service the harness's API→DB channel like the DB thread + reply router:
/// requests are filed in the real `PendingApiRequests` and answered by the
/// production `elevated_query_reply` against the harness DB.
fn serve_db_thread(h: &ApiHarness) -> DbThreadOutputs {
    fe_database::op_log::init_hlc(0);
    let rx = h.api_command_receiver();
    let db = (*h.db).clone();
    let blob_store = h.state.blob_store.clone().expect("harness blob store");
    let (repl_tx, replication) = crossbeam::channel::bounded(64);
    let (scene_tx, scene) = tokio::sync::broadcast::channel(64);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("db thread runtime");
        let mut pending = PendingApiRequests::default();
        while let Ok(ApiCommand::DbRequest { cmd, reply_tx }) = rx.recv() {
            pending.enqueue_for(&cmd, reply_tx);
            let reply = match cmd {
                DbCommand::ElevatedQuery {
                    statements,
                    vars,
                    write_tables,
                    actor_did,
                    correlation_id,
                } => rt.block_on(elevated_query_reply(
                    &db,
                    ElevatedSinks {
                        blob_store: &blob_store,
                        repl_tx: Some(&repl_tx),
                        entity_change_tx: Some(&scene_tx),
                    },
                    ElevatedRequest {
                        statements,
                        vars,
                        write_tables,
                        actor_did,
                        correlation_id,
                    },
                )),
                other => DbResult::Error(format!("not serviced by this test: {other:?}")),
            };
            pending.try_deliver(reply);
        }
    });
    DbThreadOutputs { replication, scene }
}

async fn elevated(
    h: &ApiHarness,
    token: &str,
    sql: &str,
    vars: serde_json::Value,
) -> serde_json::Value {
    let (status, body) = h
        .post_json(ELEVATED, Some(token), &json!({ "sql": sql, "vars": vars }))
        .await;
    assert_eq!(status, StatusCode::OK, "envelope errors ride a 200: {body}");
    body
}

async fn node_row(h: &ApiHarness, node_id: &str) -> serde_json::Value {
    let mut res =
        h.db.query("SELECT display_name, edit_seq FROM node WHERE node_id = $id")
            .bind(("id", node_id.to_string()))
            .await
            .expect("read node");
    let rows: Vec<serde_json::Value> = res.take(0).expect("node rows");
    rows.into_iter().next().expect("node row")
}

async fn node_display_name(h: &ApiHarness, node_id: &str) -> String {
    node_row(h, node_id).await["display_name"]
        .as_str()
        .expect("display_name")
        .to_string()
}

async fn elevated_op_log_rows(h: &ApiHarness) -> Vec<serde_json::Value> {
    let mut res =
        h.db.query("SELECT node_id, payload FROM op_log WHERE op_type = 'ElevatedQuery'")
            .await
            .expect("read op_log");
    res.take(0).expect("op_log rows")
}

#[tokio::test]
async fn elevated_query_is_refused_unless_the_operator_opted_in() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let node = h
        .seed_node(&hier.petal_id, "Original", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");
    let owner = h.mint_token(&hier.verse_scope(), "owner");

    for sql in [
        "SELECT * FROM node",
        "UPDATE node SET display_name = 'pwned'",
    ] {
        let body = elevated(&h, &owner, sql, json!({})).await;
        assert_eq!(body["ok"], false, "{sql}: {body}");
        assert_eq!(
            body["error"], "elevated query endpoint is disabled on this node",
            "{sql}"
        );
        assert!(body["data"].is_null(), "nothing executed: {body}");
    }
    assert_eq!(node_display_name(&h, &node).await, "Original");
}

#[tokio::test]
async fn elevated_query_requires_owner_even_when_enabled() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
    let _db = serve_db_thread(&h);
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let node = h
        .seed_node(&hier.petal_id, "Original", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");

    for role in ["viewer", "editor", "manager"] {
        let token = h.mint_token(&hier.verse_scope(), role);
        let body = elevated(
            &h,
            &token,
            "UPDATE node SET display_name = 'pwned'",
            json!({}),
        )
        .await;
        assert_eq!(body["ok"], false, "{role}: {body}");
        assert_eq!(body["error"], "insufficient permissions (owner required)");
    }
    assert_eq!(node_display_name(&h, &node).await, "Original");
}

/// The ratified flow: an Owner write runs on the DB thread and behaves like a
/// typed edit — one op-log entry, the scene sees the new name, peers get the
/// row (with its bumped `edit_seq`).
#[tokio::test]
async fn owner_write_is_op_logged_replicated_and_broadcast() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
    let mut outputs = serve_db_thread(&h);
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let node = h
        .seed_node(&hier.petal_id, "Original", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");
    let owner = h.mint_token(&hier.verse_scope(), "owner");

    let body = elevated(
        &h,
        &owner,
        "SELECT node_id, display_name FROM node WHERE petal_id = $pid",
        json!({ "pid": hier.petal_id }),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(
        body["data"]["data"],
        json!([{ "node_id": node, "display_name": "Original" }])
    );
    assert!(
        elevated_op_log_rows(&h).await.is_empty(),
        "a read-only script is not op-logged"
    );

    let body = elevated(
        &h,
        &owner,
        "UPDATE node SET display_name = $name WHERE node_id = $id",
        json!({ "name": "Renamed", "id": node }),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    let row = node_row(&h, &node).await;
    assert_eq!(row["display_name"], "Renamed");
    assert_eq!(
        row["edit_seq"], 1,
        "updated nodes bump edit_seq like typed edits"
    );

    let logged = elevated_op_log_rows(&h).await;
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(
        logged[0]["node_id"],
        h.did(),
        "the actor is the token subject"
    );
    assert_eq!(logged[0]["payload"]["write_tables"], json!(["node"]));

    let mut res =
        h.db.query("SELECT op, source_did, payload FROM node_log WHERE node_id = $id")
            .bind(("id", node.clone()))
            .await
            .expect("read node_log");
    let history: Vec<serde_json::Value> = res.take(0).expect("node_log rows");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!(history[0]["op"], "elevated_write");
    assert_eq!(history[0]["source_did"], h.did());
    assert_eq!(history[0]["payload"]["change"], "updated");
    assert_eq!(history[0]["payload"]["row"]["display_name"], "Renamed");

    let event = outputs
        .replication
        .try_recv()
        .expect("one replication event");
    assert!(outputs.replication.try_recv().is_err(), "exactly one");
    assert_eq!(event.table, "node");
    assert_eq!(event.record_id, node);
    assert_eq!(event.verse_id, hier.verse_id);
    assert_eq!(event.petal_id.as_deref(), Some(hier.petal_id.as_str()));
    let blob_store = h.state.blob_store.as_ref().expect("blob store");
    let path = blob_store
        .get_blob_path(&event.content_hash)
        .expect("replicated row blob");
    let replicated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("read blob")).expect("row JSON");
    assert_eq!(replicated["display_name"], "Renamed");
    assert_eq!(
        replicated["edit_seq"], 1,
        "peers receive the bumped sequence"
    );

    match outputs.scene.try_recv().expect("one scene change") {
        SceneChange::NodeAdded { node: dto } => {
            assert_eq!(dto.node_id, node);
            assert_eq!(dto.name, "Renamed");
            assert_eq!(dto.petal_id, hier.petal_id);
        }
        other => panic!("expected NodeAdded, got {other:?}"),
    }
}

/// A script whose later statement fails leaves neither its earlier writes nor
/// an op-log row, and emits nothing.
#[tokio::test]
async fn failing_elevated_script_rolls_back_writes_and_log() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
    let mut outputs = serve_db_thread(&h);
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let node = h
        .seed_node(&hier.petal_id, "Original", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");
    let owner = h.mint_token(&hier.verse_scope(), "owner");

    let body = elevated(
        &h,
        &owner,
        "UPDATE node SET display_name = 'Doomed' WHERE node_id = $id; \
         UPDATE node SET interactive = 'not a bool' WHERE node_id = $id",
        json!({ "id": node }),
    )
    .await;
    assert_eq!(body["ok"], false, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.starts_with("query failed: "), "{error}");
    assert!(
        !error.contains("due to a failed transaction"),
        "the schema violation is reported, not its echo: {error}"
    );

    assert_eq!(node_display_name(&h, &node).await, "Original");
    assert!(elevated_op_log_rows(&h).await.is_empty());
    assert!(outputs.replication.try_recv().is_err());
    assert!(outputs.scene.try_recv().is_err());
}

/// FEAPI-ELEV-TIMEOUT: a long scan hits the DB-side `TIMEOUT 5s` (rolled
/// back, never the API's 7 s backstop), and the DB thread answers the next
/// script.
#[tokio::test]
async fn long_scan_hits_the_db_side_timeout() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
    let _outputs = serve_db_thread(&h);
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    h.db.query(
        "FOR $i IN 0..2000 { CREATE node CONTENT { node_id: <string> $i, petal_id: $pid, \
         display_name: 'n', position: <geometry<point>> [0.0, 0.0], elevation: 0.0, \
         rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], interactive: true, \
         created_at: '2026-01-01T00:00:00Z' } }",
    )
    .bind(("pid", hier.petal_id.clone()))
    .await
    .expect("seed nodes")
    .check()
    .expect("seed nodes ok");
    let owner = h.mint_token(&hier.verse_scope(), "owner");

    let started = Instant::now();
    let body = elevated(
        &h,
        &owner,
        "UPDATE node SET display_name = 'scanned' WHERE count((SELECT node_id FROM node)) > 0",
        json!({}),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(body["ok"], false, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(elapsed < Duration::from_secs(7), "{elapsed:?}: {error}");
    assert!(
        fe_api::query_guard::is_timeout_error(error) && !error.starts_with("query timed out"),
        "the DB-side TIMEOUT fired, not the API backstop: {error}"
    );
    assert!(elevated_op_log_rows(&h).await.is_empty(), "rolled back");

    let body = elevated(
        &h,
        &owner,
        "SELECT count() AS n FROM node WHERE display_name = 'n' GROUP ALL",
        json!({}),
    )
    .await;
    assert_eq!(body["ok"], true, "the DB thread is responsive: {body}");
    assert_eq!(
        body["data"]["data"],
        json!([{ "n": 2000 }]),
        "nothing renamed"
    );
}

/// The endpoint no longer touches `db_reader`: a node without one (the
/// channel-only Windows shape) serves reads and writes.
#[tokio::test]
async fn elevated_works_without_db_reader() {
    let h = ApiHarness::spawn_elevated_without_db_reader()
        .await
        .expect("spawn harness");
    assert!(h.state.db_reader.is_none());
    let _outputs = serve_db_thread(&h);
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let node = h
        .seed_node(&hier.petal_id, "Original", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");
    let owner = h.mint_token(&hier.verse_scope(), "owner");

    let body = elevated(
        &h,
        &owner,
        "UPDATE node SET display_name = 'Renamed' WHERE node_id = $id; \
         SELECT VALUE display_name FROM node WHERE node_id = $id",
        json!({ "id": node }),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    let data = body["data"]["data"].as_array().expect("rows");
    assert_eq!(data.last(), Some(&json!("Renamed")), "{data:?}");
    assert_eq!(node_display_name(&h, &node).await, "Renamed");
}

#[tokio::test]
async fn owner_cannot_rewrite_privilege_rows_through_elevated_sql() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
    let _outputs = serve_db_thread(&h);
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    h.db.query(
        "CREATE role CONTENT { peer_did: 'did:key:z6MkVictim', scope: $scope, role: 'viewer' }",
    )
    .bind(("scope", hier.verse_scope()))
    .await
    .expect("seed role")
    .check()
    .expect("seed role ok");
    let owner = h.mint_token(&hier.verse_scope(), "owner");

    // ≤ 5 calls: the per-user rate limit must not be what refuses them.
    for sql in [
        "UPDATE role SET role = 'owner'",
        "DELETE role",
        "UPDATE node, role SET role = 'owner'",
        "UPDATE verse SET created_by = $who",
    ] {
        let body = elevated(&h, &owner, sql, json!({ "who": "did:key:z6MkAttacker" })).await;
        assert_eq!(body["ok"], false, "{sql}: {body}");
        let err = body["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("is not allowed in elevated queries"),
            "{sql}: {err}"
        );
    }

    let mut res =
        h.db.query("SELECT VALUE role FROM role; SELECT VALUE created_by FROM verse")
            .await
            .expect("read back");
    let roles: Vec<String> = res.take(0).expect("roles");
    let owners: Vec<String> = res.take(1).expect("verse owners");
    assert_eq!(roles, vec!["viewer".to_string()]);
    assert_eq!(owners, vec![h.did()]);
}
