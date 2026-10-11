//! `verse.namespace_id` is the iroh-docs replica read capability
//! (fe-sync/src/AGENTS.md §namespace-id-capability), so `/api/v1/query` must
//! never return it — for the caller's own verse or anyone else's
//! (fe-api/AGENTS.md §query-guard).

use axum::http::StatusCode;
use fractalengine_test_harness::api::ApiHarness;
use serde_json::json;

const OWN_NAMESPACE: &str = "ns0wnReplicaCapability";
const FOREIGN_NAMESPACE: &str = "ns4eignReplicaCapability";

async fn set_namespace(h: &ApiHarness, verse_id: &str, namespace_id: &str) {
    h.db.query("UPDATE verse SET namespace_id = $ns WHERE verse_id = $vid")
        .bind(("ns", namespace_id.to_string()))
        .bind(("vid", verse_id.to_string()))
        .await
        .expect("set namespace_id")
        .check()
        .expect("set namespace_id ok");
}

async fn query(h: &ApiHarness, token: &str, sql: &str) -> serde_json::Value {
    let (status, body) = h
        .post_json(
            "/api/v1/query",
            Some(token),
            &json!({ "sql": sql, "vars": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "envelope errors ride a 200: {body}");
    body
}

fn assert_no_capability_leak(body: &serde_json::Value, sql: &str) {
    let text = body.to_string();
    assert!(
        !text.contains(OWN_NAMESPACE) && !text.contains(FOREIGN_NAMESPACE),
        "{sql} leaked a namespace_id: {text}"
    );
}

#[tokio::test]
async fn query_never_returns_a_verse_namespace_id() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed own hierarchy");
    let foreign = h.seed_hierarchy().await.expect("seed foreign hierarchy");
    set_namespace(&h, &own.verse_id, OWN_NAMESPACE).await;
    set_namespace(&h, &foreign.verse_id, FOREIGN_NAMESPACE).await;
    let viewer = h.mint_token(&own.verse_scope(), "viewer");

    // Whole rows are still served (OMIT parses on the embedded engine), minus the capability
    // column and the record id whose `id.*` would re-fetch it.
    let sql = "SELECT * FROM verse";
    let body = query(&h, &viewer, sql).await;
    assert_eq!(body["ok"], true, "{body}");
    let rows = body["data"]["data"].as_array().expect("rows array");
    assert!(
        rows.iter()
            .any(|row| row["verse_id"].as_str() == Some(own.verse_id.as_str())),
        "{body}"
    );
    for row in rows {
        let row = row.as_object().expect("row object");
        assert!(!row.contains_key("namespace_id"), "{row:?}");
        assert!(!row.contains_key("id"), "{row:?}");
    }
    assert_no_capability_leak(&body, sql);

    let sql = "SELECT id.* AS full FROM verse";
    let body = query(&h, &viewer, sql).await;
    assert_eq!(body["ok"], true, "{body}");
    assert_no_capability_leak(&body, sql);

    for sql in [
        "SELECT namespace_id FROM verse",
        "SELECT verse_id FROM verse WHERE namespace_id = 'ns4eignReplicaCapability'",
    ] {
        let body = query(&h, &viewer, sql).await;
        assert_eq!(body["ok"], false, "{sql}: {body}");
        assert_eq!(
            body["error"],
            fe_api::query_guard::NAMESPACE_ID_REFUSAL,
            "{sql}"
        );
    }
}
