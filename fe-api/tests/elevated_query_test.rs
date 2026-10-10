//! `/api/v1/query/elevated` through the full router (fe-api/AGENTS.md
//! §elevated-query): operator opt-in, Owner-only, privilege tables unreachable.

use axum::http::StatusCode;
use fractalengine_test_harness::api::ApiHarness;
use serde_json::json;

const ELEVATED: &str = "/api/v1/query/elevated";

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

async fn node_display_name(h: &ApiHarness, node_id: &str) -> String {
    let mut res =
        h.db.query("SELECT VALUE display_name FROM node WHERE node_id = $id")
            .bind(("id", node_id.to_string()))
            .await
            .expect("read node");
    let names: Vec<String> = res.take(0).expect("display_name");
    names.into_iter().next().expect("node row")
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

#[tokio::test]
async fn owner_reads_and_writes_scene_content_when_enabled() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
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

    let body = elevated(
        &h,
        &owner,
        "UPDATE node SET display_name = $name WHERE node_id = $id",
        json!({ "name": "Renamed", "id": node }),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(node_display_name(&h, &node).await, "Renamed");
}

#[tokio::test]
async fn owner_cannot_rewrite_privilege_rows_through_elevated_sql() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
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
