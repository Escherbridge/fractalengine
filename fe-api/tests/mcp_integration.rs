//! Integration tests for the MCP JSON-RPC endpoint (fe-api/src/mcp.rs).
//!
//! Two lanes: (1) the full-router API harness for auth/inventory/error paths
//! that never reach the DB thread, and (2) direct `mcp_handler` calls against
//! an emulated DB thread (the harness never services `api_cmd_rx`, so
//! channel-round-trip tools would time out — see fe-test-harness/src/AGENTS.md
//! §api-harness) for the create → read → update round trip.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde_json::json;

use fe_api::mcp::{mcp_handler, JsonRpcRequest};
use fe_api::server::ApiState;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{
    ApiCommand, DbCommand, DbResult, FractalHierarchyData, NodeHierarchyData, PetalHierarchyData,
    VerseHierarchyData,
};
use fractalengine_test_harness::api::ApiHarness;

const EXPECTED_TOOLS: [&str; 10] = [
    "get_hierarchy",
    "create_verse",
    "create_fractal",
    "create_petal",
    "create_node",
    "update_transform",
    // Per-endpoint CRUD (endpoint_api_surface_20260725 FR-4)
    "read_node",
    "node_address",
    "delete_node",
    "promote_instance",
];

// ---------------------------------------------------------------------------
// Lane 2 plumbing: emulated DB thread behind a real ApiState
// ---------------------------------------------------------------------------

/// In-memory stand-in for the DB thread's state, plus a DbCommand audit log.
#[derive(Default)]
struct Model {
    verses: Vec<(String, String)>,
    fractals: Vec<(String, String, String)>, // (id, verse_id, name)
    petals: Vec<(String, String, String)>,   // (id, fractal_id, name)
    nodes: Vec<NodeRec>,
    log: Vec<serde_json::Value>,
}

struct NodeRec {
    id: String,
    petal_id: String,
    name: String,
    position: [f32; 3],
}

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

/// Service `api_cmd_rx` like the production DB thread would (in-memory model).
fn spawn_db_emulator(rx: crossbeam::channel::Receiver<ApiCommand>, model: Arc<Mutex<Model>>) {
    tokio::spawn(async move {
        loop {
            match rx.try_recv() {
                Ok(cmd) => handle_command(cmd, &model),
                Err(crossbeam::channel::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                Err(crossbeam::channel::TryRecvError::Disconnected) => break,
            }
        }
    });
}

fn handle_command(cmd: ApiCommand, model: &Arc<Mutex<Model>>) {
    let mut m = model.lock().expect("model lock");
    match cmd {
        ApiCommand::GetHierarchy { reply_tx } => {
            let data: Vec<VerseHierarchyData> = m
                .verses
                .iter()
                .map(|(vid, vname)| VerseHierarchyData {
                    id: vid.clone(),
                    name: vname.clone(),
                    namespace_id: None,
                    fractals: m
                        .fractals
                        .iter()
                        .filter(|(_, v, _)| v == vid)
                        .map(|(fid, _, fname)| FractalHierarchyData {
                            id: fid.clone(),
                            name: fname.clone(),
                            petals: m
                                .petals
                                .iter()
                                .filter(|(_, f, _)| f == fid)
                                .map(|(pid, _, pname)| PetalHierarchyData {
                                    id: pid.clone(),
                                    name: pname.clone(),
                                    nodes: m
                                        .nodes
                                        .iter()
                                        .filter(|n| n.petal_id == *pid)
                                        .map(|n| NodeHierarchyData {
                                            id: n.id.clone(),
                                            name: n.name.clone(),
                                            has_asset: false,
                                            position: n.position,
                                            asset_path: None,
                                            petal_id: n.petal_id.clone(),
                                            webpage_url: None,
                                        })
                                        .collect(),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect();
            let _ = reply_tx.send(data);
        }
        ApiCommand::DbRequest { cmd, reply_tx } => {
            let result = match cmd {
                DbCommand::CreateVerse { name } => {
                    let id = ulid();
                    m.log.push(json!({ "cmd": "CreateVerse", "name": name }));
                    m.verses.push((id.clone(), name.clone()));
                    DbResult::VerseCreated { id, name }
                }
                DbCommand::CreateFractal { verse_id, name } => {
                    let id = ulid();
                    m.log.push(
                        json!({ "cmd": "CreateFractal", "verse_id": verse_id, "name": name }),
                    );
                    m.fractals
                        .push((id.clone(), verse_id.clone(), name.clone()));
                    DbResult::FractalCreated { id, verse_id, name }
                }
                DbCommand::CreatePetal { fractal_id, name } => {
                    let id = ulid();
                    m.log.push(
                        json!({ "cmd": "CreatePetal", "fractal_id": fractal_id, "name": name }),
                    );
                    m.petals
                        .push((id.clone(), fractal_id.clone(), name.clone()));
                    DbResult::PetalCreated {
                        id,
                        fractal_id,
                        name,
                    }
                }
                DbCommand::CreateNode {
                    petal_id,
                    name,
                    position,
                    correlation_id,
                } => {
                    let id = ulid();
                    m.log.push(json!({
                        "cmd": "CreateNode", "petal_id": petal_id,
                        "name": name, "position": position.to_vec(),
                    }));
                    m.nodes.push(NodeRec {
                        id: id.clone(),
                        petal_id: petal_id.clone(),
                        name: name.clone(),
                        position,
                    });
                    DbResult::NodeCreated {
                        id,
                        petal_id,
                        name,
                        has_asset: false,
                        correlation_id,
                        position,
                    }
                }
                DbCommand::UpdateNodeTransform {
                    node_id,
                    position,
                    rotation,
                    scale,
                } => {
                    m.log.push(json!({
                        "cmd": "UpdateNodeTransform", "node_id": node_id,
                        "position": position.to_vec(), "rotation": rotation.to_vec(),
                        "scale": scale.to_vec(),
                    }));
                    if let Some(n) = m.nodes.iter_mut().find(|n| n.id == node_id) {
                        n.position = position;
                    }
                    DbResult::Pong // mcp.rs drops its reply_rx; value unused
                }
                _ => DbResult::Error("unhandled command in emulator".into()),
            };
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}

/// ApiState wired to the emulator (MCP never touches db_reader).
fn emu_state() -> (Arc<ApiState>, Arc<Mutex<Model>>) {
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(64);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(16);
    let state = Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: None,
        cors_origins: vec![],
        db_reader: None,
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    });
    let model = Arc::new(Mutex::new(Model::default()));
    spawn_db_emulator(api_cmd_rx, model.clone());
    (state, model)
}

fn claims(scope: &str, role: &str) -> ApiClaims {
    ApiClaims {
        sub: "did:key:z6MkMcpTester".to_string(),
        scope: scope.to_string(),
        max_role: role.to_string(),
        token_type: "api".to_string(),
        iat: 0,
        exp: u64::MAX,
        jti: "jti-mcp-test".to_string(),
    }
}

/// Fire a JSON-RPC request straight at the real MCP handler.
async fn rpc(
    state: &Arc<ApiState>,
    c: &ApiClaims,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(7)),
        method: method.into(),
        params,
    };
    let resp = mcp_handler(State(state.clone()), Extension(c.clone()), Json(req))
        .await
        .into_response();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn call_tool(
    state: &Arc<ApiState>,
    c: &ApiClaims,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    rpc(
        state,
        c,
        "tools/call",
        json!({ "name": tool, "arguments": args }),
    )
    .await
}

/// Text payload of a tool response's first content block.
fn tool_text(resp: &serde_json::Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no tool text in {resp}"))
        .to_string()
}

/// Parse a successful tool response's JSON payload (asserts isError absent).
fn tool_json(resp: &serde_json::Value) -> serde_json::Value {
    assert!(resp["result"]["isError"].is_null(), "tool errored: {resp}");
    serde_json::from_str(&tool_text(resp)).expect("tool payload JSON")
}

fn assert_tool_error(resp: &serde_json::Value, needle: &str) {
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    let text = tool_text(resp);
    assert!(text.contains(needle), "want `{needle}` in `{text}`");
}

// ---------------------------------------------------------------------------
// (a) tools/list inventory + initialize (full router)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tools_list_inventory() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let token = h.mint_token("VERSE#v1", "viewer");

    let (status, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names, EXPECTED_TOOLS,
        "exact 10-tool inventory (6 base + 4 CRUD)"
    );
    // FR-4: the per-endpoint delete tool now exists.
    assert!(names.contains(&"delete_node"));
    for t in tools {
        assert!(t["description"].is_string(), "{t}");
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
    }

    // initialize + notifications/initialized handshake.
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize" }),
        )
        .await;
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(body["result"]["serverInfo"]["name"], "fractalengine");
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "notifications/initialized" }),
        )
        .await;
    assert!(body["error"].is_null(), "{body}");
}

// ---------------------------------------------------------------------------
// (d) unknown method / unknown tool (full router)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_method_and_unknown_tool() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let token = h.mint_token("VERSE#v1", "viewer");

    // Unknown JSON-RPC method → -32601 error object.
    let (status, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/bogus" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32601);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("method not found"),
        "{body}"
    );
    assert!(body["result"].is_null());

    // Unknown tool → in-band tool error (isError content), not a panic.
    let (status, body) = h
        .post_json(
            "/mcp",
            Some(&token),
            &json!({
                "jsonrpc": "2.0", "id": 10, "method": "tools/call",
                "params": { "name": "nonexistent_tool", "arguments": {} }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_tool_error(&body, "unknown tool: nonexistent_tool");
}

// ---------------------------------------------------------------------------
// (c) malformed arguments → error, not panic (full router)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_arguments_error_not_panic() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let manager = h.mint_token("VERSE#v1", "manager");

    // Missing required args → validation tool error (before any channel send).
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&manager),
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "create_node", "arguments": { "petal_id": "p1" } }
            }),
        )
        .await;
    assert_tool_error(&body, "petal_id and name are required");

    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&manager),
            &json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "create_verse", "arguments": {} }
            }),
        )
        .await;
    assert_tool_error(&body, "name is required");

    // Non-object arguments degrade to {} → same validation error.
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&manager),
            &json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "create_verse", "arguments": 42 }
            }),
        )
        .await;
    assert_tool_error(&body, "name is required");

    // params without a tool name dispatches as tool "" → unknown tool.
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&manager),
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {} }),
        )
        .await;
    assert_tool_error(&body, "unknown tool");

    // Syntactically invalid JSON body → axum rejection (4xx), not a panic.
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {manager}"))
        .body(Body::from("{not json"))
        .expect("build request");
    let resp = h.request(req).await;
    assert!(resp.status().is_client_error(), "{}", resp.status());
}

// ---------------------------------------------------------------------------
// (e) authz — current behavior (full router, real minted tokens)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authz_insufficient_role_current_behavior() {
    let h = ApiHarness::spawn().await.expect("spawn harness");

    // No token → 401 from the real auth middleware.
    let (status, _) = h
        .post_json(
            "/mcp",
            None,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Viewer cannot create_verse (manager floor).
    let viewer = h.mint_token("VERSE#v1", "viewer");
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&viewer),
            &json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "create_verse", "arguments": { "name": "V" } }
            }),
        )
        .await;
    assert_tool_error(&body, "insufficient permissions");

    // Editor scoped to verse A cannot create_fractal in verse B.
    let editor_a = h.mint_token("VERSE#verse-a", "editor");
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&editor_a),
            &json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "create_fractal",
                            "arguments": { "verse_id": "verse-b", "name": "F" } }
            }),
        )
        .await;
    assert_tool_error(&body, "insufficient permissions or scope");

    // Viewer cannot update_transform (editor floor).
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&viewer),
            &json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": { "name": "update_transform",
                            "arguments": { "node_id": "n1", "position": [0,0,0],
                                           "rotation": [0,0,0], "scale": [1,1,1] } }
            }),
        )
        .await;
    assert_tool_error(&body, "insufficient permissions");
}

// ---------------------------------------------------------------------------
// (b) full round trip through the real dispatcher (emulated DB thread)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn round_trip_create_read_update_read() {
    let (state, model) = emu_state();

    // create_verse (manager).
    let resp = call_tool(
        &state,
        &claims("VERSE#any", "manager"),
        "create_verse",
        json!({ "name": "MCP Verse" }),
    )
    .await;
    let verse = tool_json(&resp);
    let vid = verse["id"].as_str().expect("verse id").to_string();
    assert_eq!(verse["name"], "MCP Verse");

    // create_fractal / create_petal / create_node under the new verse's scope.
    let editor = claims(&format!("VERSE#{vid}"), "editor");
    let resp = call_tool(
        &state,
        &editor,
        "create_fractal",
        json!({ "verse_id": vid, "name": "MCP Fractal" }),
    )
    .await;
    let fid = tool_json(&resp)["id"]
        .as_str()
        .expect("fractal id")
        .to_string();

    let resp = call_tool(
        &state,
        &editor,
        "create_petal",
        json!({ "verse_id": vid, "fractal_id": fid, "name": "MCP Petal" }),
    )
    .await;
    let pid = tool_json(&resp)["id"]
        .as_str()
        .expect("petal id")
        .to_string();

    let resp = call_tool(
        &state,
        &editor,
        "create_node",
        json!({
            "verse_id": vid, "fractal_id": fid, "petal_id": pid,
            "name": "MCP Node", "position": [12.5, 3.25, -7.75]
        }),
    )
    .await;
    let node = tool_json(&resp);
    let nid = node["id"].as_str().expect("node id").to_string();
    assert_eq!(node["name"], "MCP Node");

    // Read back via get_hierarchy: the node is where we put it.
    let resp = rpc(
        &state,
        &editor,
        "tools/call",
        json!({ "name": "get_hierarchy", "arguments": {} }),
    )
    .await;
    let hier = tool_json(&resp);
    let node_dto = &hier[0]["fractals"][0]["petals"][0]["nodes"][0];
    assert_eq!(hier[0]["id"], vid.as_str());
    assert_eq!(node_dto["id"], nid.as_str());
    assert_eq!(node_dto["position"], json!([12.5, 3.25, -7.75]));

    // update_transform, then read back the change.
    let resp = call_tool(
        &state,
        &editor,
        "update_transform",
        json!({
            "node_id": nid, "position": [100.5, -2.25, 0.125],
            "rotation": [0.0, 0.0, 0.0], "scale": [2.0, 2.0, 2.0]
        }),
    )
    .await;
    assert_eq!(tool_json(&resp), json!({ "status": "ok" }));

    // update_transform replies before the DB applies — wait for the emulator.
    for _ in 0..500 {
        if model.lock().unwrap().nodes[0].position == [100.5, -2.25, 0.125] {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let resp = rpc(
        &state,
        &editor,
        "tools/call",
        json!({ "name": "get_hierarchy", "arguments": {} }),
    )
    .await;
    let node_dto = &tool_json(&resp)[0]["fractals"][0]["petals"][0]["nodes"][0];
    assert_eq!(node_dto["position"], json!([100.5, -2.25, 0.125]));

    // Delete leg skipped: no delete tool exists (see tools_list_inventory).
}

// ---------------------------------------------------------------------------
// (g) per-endpoint CRUD tools (FR-4): role floors + validation, no DB reader
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_endpoint_crud_tools_role_and_validation() {
    let (state, _model) = emu_state(); // no db_reader — scope resolution yields None
    let ulid = ulid::Ulid::new().to_string();

    // delete_node / promote_instance require editor: a viewer is rejected before
    // any channel send (fe-policy remains the authoritative gate server-side).
    let viewer = claims("VERSE#v1", "viewer");
    let resp = call_tool(&state, &viewer, "delete_node", json!({ "node_id": ulid })).await;
    assert_tool_error(&resp, "insufficient permissions");
    let resp = call_tool(
        &state,
        &viewer,
        "promote_instance",
        json!({ "petal_id": ulid, "path_id": "p", "instance_index": 0 }),
    )
    .await;
    assert_tool_error(&resp, "insufficient permissions");

    // read_node validates the node id shape (viewer+), rejecting junk ids.
    let resp = call_tool(&state, &viewer, "read_node", json!({ "node_id": "junk" })).await;
    assert_tool_error(&resp, "invalid node_id");

    // A well-formed id with no backing store resolves to "node not found"
    // (typed error, never a panic).
    let editor = claims("VERSE#v1", "editor");
    let resp = call_tool(&state, &editor, "read_node", json!({ "node_id": ulid })).await;
    assert_tool_error(&resp, "node not found");

    // promote_instance requires the instance index.
    let resp = call_tool(
        &state,
        &editor,
        "promote_instance",
        json!({ "petal_id": ulid, "path_id": "path-1" }),
    )
    .await;
    assert_tool_error(&resp, "instance_index is required");
}

// ---------------------------------------------------------------------------
// (f) update_transform unit contract: world units on the wire, no conversion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_transform_world_units_pass_through_unconverted() {
    let (state, model) = emu_state();
    let editor = claims("VERSE#v1", "editor");

    let resp = call_tool(
        &state,
        &editor,
        "update_transform",
        json!({
            "node_id": "n-units",
            "position": [1234.5, -67.25, 0.125],   // world units (meters × world_scale)
            "rotation": [1.5, -0.5, 0.25],          // radians
            "scale": [2.5, 0.5, 1.0]
        }),
    )
    .await;
    assert_eq!(tool_json(&resp), json!({ "status": "ok" }));

    // The DbCommand forwarded by the dispatcher carries the wire values
    // bit-for-bit — no meters/degrees/world_scale conversion in mcp.rs.
    let cmd = 'found: {
        for _ in 0..500 {
            {
                let m = model.lock().unwrap();
                if let Some(c) = m.log.iter().find(|c| c["cmd"] == "UpdateNodeTransform") {
                    break 'found c.clone();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("UpdateNodeTransform never reached the DB channel");
    };
    assert_eq!(cmd["node_id"], "n-units");
    assert_eq!(cmd["position"], json!([1234.5, -67.25, 0.125]));
    assert_eq!(cmd["rotation"], json!([1.5, -0.5, 0.25]));
    assert_eq!(cmd["scale"], json!([2.5, 0.5, 1.0]));
}

// ---------------------------------------------------------------------------
// (e) known-weak authz warts — assert CURRENT behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scope_warts_current_behavior_known_weak() {
    let (state, model) = emu_state();
    // Editor scoped to an UNRELATED verse throughout.
    let foreign_editor = claims("VERSE#unrelated", "editor");

    // KNOWN-WEAK (mcp_scene_primitives_20260716): create_node without
    // verse_id/fractal_id falls back to a role-only check — an out-of-scope
    // editor can create nodes in any petal.
    let resp = call_tool(
        &state,
        &foreign_editor,
        "create_node",
        json!({ "petal_id": "someone-elses-petal", "name": "Sneaky", "position": [1.0, 2.0, 3.0] }),
    )
    .await;
    let node = tool_json(&resp); // succeeds today
    assert_eq!(node["name"], "Sneaky");

    // KNOWN-WEAK (mcp_scene_primitives_20260716): create_petal without a
    // verse_id skips the scope check entirely.
    let resp = call_tool(
        &state,
        &foreign_editor,
        "create_petal",
        json!({ "fractal_id": "someone-elses-fractal", "name": "Sneaky Petal" }),
    )
    .await;
    assert_eq!(tool_json(&resp)["name"], "Sneaky Petal"); // succeeds today

    // KNOWN-WEAK (mcp_scene_primitives_20260716): update_transform has no
    // scope check at all, and reports "ok" without awaiting the DB result —
    // even for a nonexistent node.
    let resp = call_tool(
        &state,
        &foreign_editor,
        "update_transform",
        json!({ "node_id": "does-not-exist", "position": [0,0,0],
                "rotation": [0,0,0], "scale": [1,1,1] }),
    )
    .await;
    assert_eq!(tool_json(&resp), json!({ "status": "ok" }));

    // Lenient parsing: a malformed position silently defaults to the origin
    // (no error to the caller) — asserted as current behavior.
    let resp = call_tool(
        &state,
        &foreign_editor,
        "create_node",
        json!({ "petal_id": "p1", "name": "Origin Default", "position": "not-an-array" }),
    )
    .await;
    tool_json(&resp);
    for _ in 0..500 {
        {
            let m = model.lock().unwrap();
            if let Some(n) = m.nodes.iter().find(|n| n.name == "Origin Default") {
                assert_eq!(n.position, [0.0, 0.0, 0.0]);
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("Origin Default node never created");
}
