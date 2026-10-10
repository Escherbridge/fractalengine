//! Record-id reads through the raw-SQL surfaces (fe-api/AGENTS.md
//! §query-guard, §elevated-query): a `table:key` literal or a string turned
//! into a record (`$v.to_record()`) never names a FROM target, so the table
//! whitelist alone let `/query` (Viewer, any verse) and `/query/elevated`
//! (Owner) dereference `role` rows and other verses' nodes by id.

use axum::http::StatusCode;
use fractalengine_test_harness::api::ApiHarness;
use serde_json::json;

const VICTIM_DID: &str = "did:key:z6MkVictimRoleRow";

async fn post(h: &ApiHarness, path: &str, token: &str, sql: &str) -> serde_json::Value {
    // `v` = a privilege row, `o` = a node in a verse the caller is not scoped to.
    let vars = json!({ "v": "role:victim", "o": other_node_rid(h).await });
    let (status, body) = h
        .post_json(path, Some(token), &json!({ "sql": sql, "vars": vars }))
        .await;
    assert_eq!(status, StatusCode::OK, "envelope errors ride a 200: {body}");
    body
}

async fn other_node_rid(h: &ApiHarness) -> String {
    let mut res =
        h.db.query("SELECT VALUE <string> id FROM node WHERE display_name = 'ForeignSecret'")
            .await
            .expect("read foreign rid");
    let ids: Vec<String> = res.take(0).expect("rid");
    ids.into_iter().next().unwrap_or_default()
}

/// Seeds the caller's verse, a `role:victim` row, and a node in a second verse.
async fn fixture(h: &ApiHarness) -> String {
    let mine = h.seed_hierarchy().await.expect("seed caller hierarchy");
    let foreign = h.seed_hierarchy().await.expect("seed foreign hierarchy");
    h.seed_node(&foreign.petal_id, "ForeignSecret", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed foreign node");
    h.db.query("CREATE role:victim CONTENT { peer_did: $did, scope: $scope, role: 'manager' }")
        .bind(("did", VICTIM_DID))
        .bind(("scope", foreign.verse_scope()))
        .await
        .expect("seed role")
        .check()
        .expect("seed role ok");
    mine.verse_scope()
}

fn assert_refused_without_leak(body: &serde_json::Value, sql: &str) {
    let text = body.to_string();
    assert!(
        !text.contains("z6MkVictimRoleRow") && !text.contains("ForeignSecret"),
        "{sql} leaked: {text}"
    );
    assert_eq!(body["ok"], false, "{sql} should be refused: {body}");
}

#[tokio::test]
async fn viewer_query_cannot_dereference_foreign_records_by_id() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let scope = fixture(&h).await;
    let viewer = h.mint_token(&scope, "viewer");

    // ≤ 10 calls: the /query rate limit must not be what refuses them.
    for sql in [
        "SELECT $v.to_record().* AS leak FROM verse",
        "SELECT $o.to_record().* AS leak FROM verse",
        "SELECT ('ro' + 'le:victim').to_record().* AS leak FROM verse",
        "SELECT role:victim.* AS leak FROM verse",
    ] {
        let body = post(&h, "/api/v1/query", &viewer, sql).await;
        assert_refused_without_leak(&body, sql);
    }

    // Datetime literals (`…T00:00:00Z`) are not record ids — still served.
    let body = post(
        &h,
        "/api/v1/query",
        &viewer,
        "SELECT verse_id FROM verse WHERE created_at > '2000-01-01T00:00:00Z'",
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
}

#[tokio::test]
async fn owner_elevated_query_cannot_read_privilege_rows_by_id() {
    let h = ApiHarness::spawn_with_elevated_query_allowed()
        .await
        .expect("spawn harness");
    let scope = fixture(&h).await;
    let owner = h.mint_token(&scope, "owner");

    // ≤ 5 calls: the elevated rate limit must not be what refuses them.
    for sql in [
        "RETURN [role:victim.*]",
        "RETURN [$v.to_record().*]",
        "SELECT *, role:victim.* AS leak FROM node",
        "RETURN [type::record('role', 'victim').*]",
        "RETURN [r'role:victim'.*]",
    ] {
        let body = post(&h, "/api/v1/query/elevated", &owner, sql).await;
        assert_refused_without_leak(&body, sql);
    }
}
