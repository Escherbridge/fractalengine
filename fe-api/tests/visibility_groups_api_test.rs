//! Visibility-Group registry flows (REST `/api/v1/petals/{p}/visibility-groups`
//! and MCP `list_visibility_groups` / `set_visibility_group`) through the full
//! router against a serviced API→DB channel: requests are filed in the real
//! `PendingApiRequests` router and answered by the production
//! `visibility_groups` reply builders against the harness DB — including the
//! DB-thread fe-policy re-check. See fe-api AGENTS.md §visibility-groups.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};

use fe_database::handlers::visibility_groups::{
    get_petal_visibility_groups_reply, set_petal_visibility_groups_reply,
};
use fe_runtime::app::PendingApiRequests;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fractalengine_test_harness::api::{body_json_lenient, ApiHarness, SeededHierarchy};

/// Service the harness's API→DB channel like the DB thread + reply router.
fn serve_db_thread(h: &ApiHarness) {
    let rx = h.api_command_receiver();
    let db = (*h.db).clone();
    let local_did = h.did();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("db thread runtime");
        let mut pending = PendingApiRequests::default();
        while let Ok(ApiCommand::DbRequest { cmd, reply_tx }) = rx.recv() {
            pending.enqueue_for(&cmd, reply_tx);
            let reply = match cmd {
                DbCommand::GetPetalVisibilityGroups {
                    petal_id,
                    correlation_id,
                } => rt.block_on(get_petal_visibility_groups_reply(
                    &db,
                    petal_id,
                    correlation_id,
                )),
                DbCommand::SetPetalVisibilityGroups {
                    petal_id,
                    groups,
                    auth,
                    correlation_id,
                } => rt.block_on(set_petal_visibility_groups_reply(
                    &db,
                    &local_did,
                    petal_id,
                    groups,
                    auth,
                    correlation_id,
                )),
                other => DbResult::Error(format!("not serviced by this test: {other:?}")),
            };
            pending.try_deliver(reply);
        }
    });
}

async fn setup() -> (ApiHarness, SeededHierarchy) {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    serve_db_thread(&h);
    (h, seeded)
}

async fn send(
    h: &ApiHarness,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let resp = h.request(builder.body(body).expect("request")).await;
    let status = resp.status();
    (status, body_json_lenient(resp).await)
}

fn registry_path(petal_id: &str) -> String {
    format!("/api/v1/petals/{petal_id}/visibility-groups")
}

fn registry() -> Value {
    json!([
        {"id": "utilities", "name": "Utilities", "visible": false, "order": 0.0},
        {"id": "phase-2", "name": "Phase 2", "visible": true, "order": 1.0}
    ])
}

/// The stored registry as an Editor's GET reports it.
async fn stored(h: &ApiHarness, petal_id: &str, token: &str) -> Value {
    let (status, body) = send(h, "GET", &registry_path(petal_id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["groups"].clone()
}

#[tokio::test]
async fn a_viewer_reads_but_cannot_replace_and_an_editor_round_trips() {
    let (h, seeded) = setup().await;
    let viewer = h.mint_token(&seeded.verse_scope(), "viewer");
    let editor = h.mint_token(&seeded.verse_scope(), "editor");
    let path = registry_path(&seeded.petal_id);

    assert_eq!(stored(&h, &seeded.petal_id, &viewer).await, json!([]));

    let (_, body) = send(
        &h,
        "PUT",
        &path,
        &viewer,
        Some(&json!({"groups": registry()})),
    )
    .await;
    assert_eq!(body["ok"], json!(false), "{body}");
    assert_eq!(body["error"], json!("insufficient permissions"));
    assert_eq!(stored(&h, &seeded.petal_id, &editor).await, json!([]));

    let (status, body) = send(
        &h,
        "PUT",
        &path,
        &editor,
        Some(&json!({"groups": registry()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["groups"],
        registry(),
        "PUT echoes the stored registry"
    );
    assert_eq!(stored(&h, &seeded.petal_id, &viewer).await, registry());

    // A malformed registry is a 400 that never reaches the store.
    let dup = json!({"groups": [
        {"id": "g", "name": "A", "visible": true, "order": 0.0},
        {"id": "g", "name": "B", "visible": true, "order": 1.0}
    ]});
    let (status, _) = send(&h, "PUT", &path, &editor, Some(&dup)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(stored(&h, &seeded.petal_id, &editor).await, registry());
}

#[tokio::test]
async fn patch_upserts_one_group_and_delete_removes_it() {
    let (h, seeded) = setup().await;
    let editor = h.mint_token(&seeded.verse_scope(), "editor");
    let path = registry_path(&seeded.petal_id);
    send(
        &h,
        "PUT",
        &path,
        &editor,
        Some(&json!({"groups": registry()})),
    )
    .await;

    let (status, body) = send(
        &h,
        "PATCH",
        &format!("{path}/utilities"),
        &editor,
        Some(&json!({"visible": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["groups"][0]["visible"], json!(true));
    assert_eq!(body["data"]["groups"][0]["name"], json!("Utilities"));

    let (status, body) = send(
        &h,
        "PATCH",
        &format!("{path}/roads"),
        &editor,
        Some(&json!({"name": "Roads"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["groups"][2],
        json!({"id": "roads", "name": "Roads", "visible": true, "order": 2.0})
    );

    let (status, body) = send(&h, "DELETE", &format!("{path}/phase-2"), &editor, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<&str> = body["data"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["utilities", "roads"]);

    let (status, _) = send(&h, "DELETE", &format!("{path}/phase-2"), &editor, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "deleting an unknown group");
}

#[tokio::test]
async fn a_foreign_petal_is_denied_exactly_like_a_missing_one() {
    let (h, seeded) = setup().await;
    let foreign = h.mint_token(&format!("VERSE#{}", ulid::Ulid::new()), "editor");
    let editor = h.mint_token(&seeded.verse_scope(), "editor");
    let missing = ulid::Ulid::new().to_string();

    for method in ["GET", "PUT"] {
        let body = (method == "PUT").then(|| json!({"groups": registry()}));
        let (_, foreign_body) = send(
            &h,
            method,
            &registry_path(&seeded.petal_id),
            &foreign,
            body.as_ref(),
        )
        .await;
        let (_, missing_body) =
            send(&h, method, &registry_path(&missing), &editor, body.as_ref()).await;
        assert_eq!(
            foreign_body["error"],
            json!(fe_api::rest::NOT_FOUND_OR_DENIED)
        );
        assert_eq!(
            foreign_body, missing_body,
            "{method}: foreign and missing petals must be indistinguishable"
        );
    }
    assert_eq!(stored(&h, &seeded.petal_id, &editor).await, json!([]));
}

#[tokio::test]
async fn mcp_set_then_list_visibility_groups() {
    let (h, seeded) = setup().await;
    let editor = h.mint_token(&seeded.verse_scope(), "editor");
    let viewer = h.mint_token(&seeded.verse_scope(), "viewer");
    let call = |id: u64, name: &str, arguments: Value| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        })
    };
    let set = |id: u64, group: Value| {
        let mut args = group;
        args["petal_id"] = json!(seeded.petal_id);
        call(id, "set_visibility_group", args)
    };

    let (status, body) = h
        .post_json(
            "/mcp",
            Some(&editor),
            &set(
                1,
                json!({"group_id": "utilities", "name": "Utilities", "visible": false}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["result"]["isError"].is_null(), "{body}");

    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&viewer),
            &call(
                2,
                "list_visibility_groups",
                json!({"petal_id": seeded.petal_id}),
            ),
        )
        .await;
    assert!(body["result"]["isError"].is_null(), "{body}");
    let listed: Value = serde_json::from_str(
        body["result"]["content"][0]["text"]
            .as_str()
            .expect("text content"),
    )
    .expect("tool payload is JSON");
    assert_eq!(
        listed["groups"],
        json!([{"id": "utilities", "name": "Utilities", "visible": false, "order": 0.0}])
    );

    // A Viewer cannot write through MCP either.
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&viewer),
            &set(3, json!({"group_id": "utilities", "delete": true})),
        )
        .await;
    assert_eq!(body["result"]["isError"], json!(true), "{body}");
    assert_eq!(
        body["result"]["content"][0]["text"],
        json!("insufficient permissions")
    );
    assert_eq!(
        stored(&h, &seeded.petal_id, &editor)
            .await
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&editor),
            &set(4, json!({"group_id": "utilities", "delete": true})),
        )
        .await;
    assert!(body["result"]["isError"].is_null(), "{body}");
    assert_eq!(stored(&h, &seeded.petal_id, &editor).await, json!([]));
}
