//! Petal render-distance mutation flows (REST PUT/GET/DELETE
//! `/api/v1/petals/{p}/render-distance` and MCP `set_petal_render_distance`)
//! through the full router against a serviced API→DB channel, mirroring
//! `terrain_mutation_test.rs`'s harness shape (p2p_asset_streaming_20260718 S6).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;

use fe_runtime::app::PendingApiRequests;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fractalengine_test_harness::api::{body_json_lenient, ApiHarness, SeededHierarchy};

/// Service the harness's API→DB channel with the PRODUCTION reply builders.
fn serve_db_thread(h: &ApiHarness) {
    let rx = h.api_command_receiver();
    let db = (*h.db).clone();
    let blob_store = h
        .state
        .blob_store
        .clone()
        .expect("harness spawned with a blob store");
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("db thread runtime");
        let mut pending = PendingApiRequests::default();
        while let Ok(ApiCommand::DbRequest { cmd, reply_tx }) = rx.recv() {
            pending.enqueue_for(&cmd, reply_tx);
            let reply = match cmd {
                DbCommand::SetPetalRenderDistance {
                    petal_id,
                    render_distance,
                    auth,
                    correlation_id,
                } => rt.block_on(
                    fe_database::handlers::petal_render_distance::set_petal_render_distance_reply(
                        &db,
                        "local-did",
                        petal_id,
                        render_distance,
                        auth,
                        None,
                        &blob_store,
                        correlation_id,
                    ),
                ),
                DbCommand::GetPetalRenderDistance {
                    petal_id,
                    correlation_id,
                } => rt.block_on(
                    fe_database::handlers::petal_render_distance::get_petal_render_distance_reply(
                        &db,
                        petal_id,
                        correlation_id,
                    ),
                ),
                other => DbResult::Error(format!("not serviced by this test: {other:?}")),
            };
            pending.try_deliver(reply);
        }
    });
}

async fn setup() -> (ApiHarness, SeededHierarchy, String) {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "editor");
    serve_db_thread(&h);
    (h, seeded, token)
}

async fn send(
    h: &ApiHarness,
    method: &str,
    petal_id: &str,
    token: &str,
    body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder()
        .method(method)
        .uri(format!("/api/v1/petals/{petal_id}/render-distance"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let resp = h.request(builder.body(body).expect("request")).await;
    let status = resp.status();
    (status, body_json_lenient(resp).await)
}

async fn stored_render_distance(h: &ApiHarness, petal_id: &str, token: &str) -> serde_json::Value {
    let (status, body) = h
        .get(
            &format!("/api/v1/petals/{petal_id}/render-distance"),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["render_distance"].clone()
}

#[tokio::test]
async fn rest_put_persists_render_distance_that_get_reads_back() {
    let (h, seeded, token) = setup().await;

    let (status, body) = send(
        &h,
        "PUT",
        &seeded.petal_id,
        &token,
        Some(&json!({ "render_distance": 1500.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["render_distance"], json!(1500.0));
    assert_eq!(
        stored_render_distance(&h, &seeded.petal_id, &token).await,
        json!(1500.0)
    );
}

#[tokio::test]
async fn rest_delete_clears_to_inherit() {
    let (h, seeded, token) = setup().await;
    let (status, _) = send(
        &h,
        "PUT",
        &seeded.petal_id,
        &token,
        Some(&json!({ "render_distance": 900.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&h, "DELETE", &seeded.petal_id, &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["render_distance"].is_null(), "{body}");
    assert!(stored_render_distance(&h, &seeded.petal_id, &token)
        .await
        .is_null());
}

#[tokio::test]
async fn mcp_set_petal_render_distance_sets_then_clears() {
    let (h, seeded, token) = setup().await;
    let call = |id: u64, render_distance: serde_json::Value| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "name": "set_petal_render_distance",
                "arguments": { "petal_id": seeded.petal_id, "render_distance": render_distance },
            },
        })
    };

    let (status, body) = h
        .post_json("/mcp", Some(&token), &call(1, json!(250.0)))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["result"]["isError"].is_null(), "{body}");
    assert_eq!(
        stored_render_distance(&h, &seeded.petal_id, &token).await,
        json!(250.0)
    );

    let (_, body) = h
        .post_json("/mcp", Some(&token), &call(2, json!(null)))
        .await;
    assert!(body["result"]["isError"].is_null(), "{body}");
    assert!(stored_render_distance(&h, &seeded.petal_id, &token)
        .await
        .is_null());
}

/// A viewer is denied and a foreign petal gets the uniform not-found-or-denied
/// body — in both cases nothing is written.
#[tokio::test]
async fn viewer_denied_and_foreign_petal_answer_uniformly_and_write_nothing() {
    let (h, seeded, editor) = setup().await;
    let viewer = h.mint_token(&seeded.verse_scope(), "viewer");
    let foreign = h.mint_token(&format!("VERSE#{}", ulid::Ulid::new()), "editor");

    let cases: [(&str, &str, StatusCode, &str); 2] = [
        (
            "viewer role",
            &viewer,
            StatusCode::OK,
            "insufficient permissions",
        ),
        (
            "foreign scope",
            &foreign,
            StatusCode::OK,
            fe_api::rest::NOT_FOUND_OR_DENIED,
        ),
    ];
    for (case, token, status, fragment) in cases {
        let (got, resp) = send(
            &h,
            "PUT",
            &seeded.petal_id,
            token,
            Some(&json!({ "render_distance": 100.0 })),
        )
        .await;
        assert_eq!(got, status, "{case}: {resp}");
        assert!(
            resp["error"]
                .as_str()
                .unwrap_or_default()
                .contains(fragment),
            "{case}: {resp}"
        );
    }
    assert!(
        stored_render_distance(&h, &seeded.petal_id, &editor)
            .await
            .is_null(),
        "neither denial wrote anything"
    );
}

/// A non-numeric `render_distance` is refused at the API layer (400) before
/// any DB round trip — mirrors `terrain.rs`'s "config shape" refusal.
#[tokio::test]
async fn an_unparseable_value_is_refused_before_any_db_round_trip() {
    let (h, seeded, token) = setup().await;
    let (status, body) = send(
        &h,
        "PUT",
        &seeded.petal_id,
        &token,
        Some(&json!({ "render_distance": "not-a-number" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        stored_render_distance(&h, &seeded.petal_id, &token)
            .await
            .is_null(),
        "refused before any write"
    );
}
