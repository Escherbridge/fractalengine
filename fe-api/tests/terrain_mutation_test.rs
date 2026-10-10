//! Petal-terrain mutation flows (REST PUT/DELETE `/api/v1/petals/{p}/terrain`
//! and MCP `set_petal_terrain`) through the full router against a serviced
//! API→DB channel: requests are filed in the real `PendingApiRequests` router
//! and answered by the production `set_petal_terrain_reply` against the
//! harness DB, then read back through the REST GET path (`db_reader`).
//! See fe-api AGENTS.md §terrain-mutation.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;

use fe_runtime::app::PendingApiRequests;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fractalengine_test_harness::api::{body_json_lenient, ApiHarness, SeededHierarchy};

/// How the simulated DB thread behaves around each API set.
#[derive(Clone, Copy, PartialEq)]
enum DbMode {
    /// Apply the write with the production reply builder.
    Normal,
    /// Precede every API set with a GUI set (uncorrelated, a different
    /// config) whose echo is offered to the router first.
    GuiRace,
    /// The petal row is deleted after auth but before the write lands.
    PetalVanishes,
}

/// Service the harness's API→DB channel like the DB thread + reply router.
fn serve_db_thread(h: &ApiHarness, mode: DbMode) {
    let rx = h.api_command_receiver();
    let db = (*h.db).clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("db thread runtime");
        let mut pending = PendingApiRequests::default();
        while let Ok(ApiCommand::DbRequest { cmd, reply_tx }) = rx.recv() {
            pending.enqueue_for(&cmd, reply_tx);
            let reply = match cmd {
                DbCommand::SetPetalTerrain {
                    petal_id,
                    terrain,
                    correlation_id,
                } => {
                    if mode == DbMode::PetalVanishes {
                        rt.block_on(async {
                            db.query("DELETE petal WHERE petal_id = $pid")
                                .bind(("pid", petal_id.clone()))
                                .await
                                .expect("delete petal")
                                .check()
                                .expect("delete petal");
                        });
                    }
                    if mode == DbMode::GuiRace {
                        let gui_echo = rt.block_on(
                            fe_database::handlers::petal_terrain::set_petal_terrain_reply(
                                &db,
                                petal_id.clone(),
                                Some(gui_config()),
                                None,
                            ),
                        );
                        assert!(
                            !pending.try_deliver(gui_echo),
                            "a GUI set echo must never satisfy an API terrain waiter"
                        );
                    }
                    rt.block_on(
                        fe_database::handlers::petal_terrain::set_petal_terrain_reply(
                            &db,
                            petal_id,
                            terrain,
                            correlation_id,
                        ),
                    )
                }
                other => DbResult::Error(format!("not serviced by this test: {other:?}")),
            };
            pending.try_deliver(reply);
        }
    });
}

/// A full `TerrainConfig` plus an additive key fe-terrain reads raw (`view_mode`).
fn api_config() -> serde_json::Value {
    let mut config = serde_json::to_value(fe_terrain::config::TerrainConfig {
        enabled: true,
        tileset_hexon_uris: vec!["tileset-a".to_string()],
        max_zoom: 14,
        ..Default::default()
    })
    .expect("serialize TerrainConfig");
    config["view_mode"] = json!("satellite");
    config
}

/// The competing desktop write in the `gui_race` flow.
fn gui_config() -> serde_json::Value {
    serde_json::to_value(fe_terrain::config::TerrainConfig {
        enabled: false,
        tileset_hexon_uris: vec!["tileset-gui".to_string()],
        ..Default::default()
    })
    .expect("serialize TerrainConfig")
}

async fn setup(mode: DbMode) -> (ApiHarness, SeededHierarchy, String) {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let seeded = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&seeded.verse_scope(), "editor");
    serve_db_thread(&h, mode);
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
        .uri(format!("/api/v1/petals/{petal_id}/terrain"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let resp = h.request(builder.body(body).expect("request")).await;
    let status = resp.status();
    (status, body_json_lenient(resp).await)
}

/// The stored terrain as REST GET reports it (`null` when cleared).
async fn stored_terrain(h: &ApiHarness, petal_id: &str, token: &str) -> serde_json::Value {
    let (status, body) = h
        .get(&format!("/api/v1/petals/{petal_id}/terrain"), Some(token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["terrain"].clone()
}

#[tokio::test]
async fn rest_put_persists_terrain_that_get_reads_back() {
    let (h, seeded, token) = setup(DbMode::Normal).await;
    let config = api_config();

    let (status, body) = send(&h, "PUT", &seeded.petal_id, &token, Some(&config)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["terrain"], config,
        "PUT returns the stored echo"
    );

    let stored = stored_terrain(&h, &seeded.petal_id, &token).await;
    assert_eq!(stored, config);
    assert!(
        serde_json::from_value::<fe_terrain::config::TerrainConfig>(stored).is_ok(),
        "what is stored satisfies the strictest reader (DEC-C22)"
    );
}

#[tokio::test]
async fn rest_delete_clears_the_stored_terrain() {
    let (h, seeded, token) = setup(DbMode::Normal).await;
    let (status, _) = send(&h, "PUT", &seeded.petal_id, &token, Some(&api_config())).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&h, "DELETE", &seeded.petal_id, &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["terrain"].is_null(), "{body}");
    assert!(stored_terrain(&h, &seeded.petal_id, &token).await.is_null());
}

/// A desktop user's concurrent set lands first on the DB thread; its
/// uncorrelated echo must not answer the REST caller, who gets exactly its
/// own write — which is also what the petal now holds.
#[tokio::test]
async fn rest_put_answers_with_its_own_write_when_a_gui_set_races_it() {
    let (h, seeded, token) = setup(DbMode::GuiRace).await;
    let config = api_config();

    let (status, body) = send(&h, "PUT", &seeded.petal_id, &token, Some(&config)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["terrain"], config);
    assert_eq!(stored_terrain(&h, &seeded.petal_id, &token).await, config);
}

#[tokio::test]
async fn mcp_set_petal_terrain_sets_then_clears() {
    let (h, seeded, token) = setup(DbMode::Normal).await;
    let config = api_config();
    let call = |id: u64, terrain: serde_json::Value| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "name": "set_petal_terrain",
                "arguments": { "petal_id": seeded.petal_id, "terrain": terrain },
            },
        })
    };

    let (status, body) = h
        .post_json("/mcp", Some(&token), &call(1, config.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["result"]["isError"].is_null(), "{body}");
    assert_eq!(stored_terrain(&h, &seeded.petal_id, &token).await, config);

    let (_, body) = h
        .post_json("/mcp", Some(&token), &call(2, json!(null)))
        .await;
    assert!(body["result"]["isError"].is_null(), "{body}");
    assert!(stored_terrain(&h, &seeded.petal_id, &token).await.is_null());
}

/// Refusals in the required order (role → scope → config shape) never write:
/// each leaves the petal's terrain exactly as it was.
#[tokio::test]
async fn refused_puts_leave_the_stored_terrain_untouched() {
    let (h, seeded, editor) = setup(DbMode::Normal).await;
    let viewer = h.mint_token(&seeded.verse_scope(), "viewer");
    let foreign = h.mint_token(&format!("VERSE#{}", ulid::Ulid::new()), "editor");
    // serde's derived struct visitor accepts this positional array (required
    // fields in declaration order, the rest defaulted) — the column cannot.
    let config = api_config();
    let array_body = json!([
        true,
        config["origin"],
        config["tile_source_url"],
        config["elevation_source"]
    ]);
    assert!(
        serde_json::from_value::<fe_terrain::config::TerrainConfig>(array_body.clone()).is_ok(),
        "precondition: the array alone would pass TerrainConfig parsing"
    );

    // (case, token, body, expected status, error fragment)
    let cases: [(&str, &str, serde_json::Value, StatusCode, &str); 4] = [
        (
            "viewer role",
            &viewer,
            api_config(),
            StatusCode::OK,
            "insufficient permissions",
        ),
        (
            "foreign scope",
            &foreign,
            api_config(),
            StatusCode::OK,
            "insufficient scope",
        ),
        (
            "not a TerrainConfig",
            &editor,
            json!({ "enabled": "yes" }),
            StatusCode::BAD_REQUEST,
            "invalid terrain config",
        ),
        (
            "positional array",
            &editor,
            array_body,
            StatusCode::BAD_REQUEST,
            "must be a JSON object",
        ),
    ];
    for (case, token, body, status, fragment) in cases {
        let (got, resp) = send(&h, "PUT", &seeded.petal_id, token, Some(&body)).await;
        assert_eq!(got, status, "{case}: {resp}");
        assert!(
            resp["error"]
                .as_str()
                .unwrap_or_default()
                .contains(fragment),
            "{case}: {resp}"
        );
        assert!(
            stored_terrain(&h, &seeded.petal_id, &editor)
                .await
                .is_null(),
            "{case}: nothing may be written"
        );
    }
}

/// The DB thread's correlated failure (no petal row left to update) reaches
/// its own caller as 502 without leaking the DB error text.
#[tokio::test]
async fn rest_put_reports_a_failed_write_as_502_without_db_detail() {
    let (h, seeded, token) = setup(DbMode::PetalVanishes).await;
    let (status, body) = send(&h, "PUT", &seeded.petal_id, &token, Some(&api_config())).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"], "terrain write failed", "{body}");
}
