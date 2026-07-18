//! Smoke test for the API-integration harness
//! (fe-test-harness/src/AGENTS.md §api-harness).

use axum::http::StatusCode;
use fractalengine_test_harness::api::ApiHarness;

#[tokio::test]
async fn harness_health_ready_and_authed_roundtrip() {
    let h = ApiHarness::spawn().await.expect("spawn harness");

    // Public health endpoint.
    let (status, body) = h.get("/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    // Readiness probe rides the db_reader ping.
    let (status, body) = h.get("/ready", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");

    // Authed route without a token -> 401 from the real auth middleware.
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let gis_path = format!("/api/v1/petals/{}/gis/nodes", hier.petal_id);
    let (status, _) = h.get(&gis_path, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Seed a node and read it back through the full router with a minted token.
    let node_id = h
        .seed_node(&hier.petal_id, "Smoke Node", [1.0, 5.0, -2.0], None)
        .await
        .expect("seed node");
    let token = h.mint_token(&hier.verse_scope(), "viewer");
    let (status, body) = h.get(&gis_path, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "gis nodes body: {body}");
    let nodes = body["data"]["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], node_id);
    assert_eq!(nodes[0]["display_name"], "Smoke Node");

    // Out-of-scope token -> 403 (scope enforcement is live).
    let foreign = h.mint_token("VERSE#someone-else", "viewer");
    let (status, _) = h.get(&gis_path, Some(&foreign)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
