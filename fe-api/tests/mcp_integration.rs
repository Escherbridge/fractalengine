//! Integration tests for the MCP JSON-RPC endpoint (fe-api/src/mcp/).
//!
//! Two lanes: (1) the full-router API harness (real auth middleware, Mem
//! SurrealDB `db_reader`) for inventory / denial / direct-read paths that
//! never need a DB-thread reply, and (2) direct `mcp_handler` calls against an
//! emulated DB thread (no `db_reader`, so every scope resolution rides the
//! channel and is answered from the emulator's model — see
//! fe-test-harness/src/AGENTS.md §api-harness) for round trips.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use base64::Engine as _;
use serde_json::json;

use fe_api::mcp::{mcp_handler, tool_specs, HierarchyTarget, JsonRpcRequest, ScopeRule};
use fe_api::server::ApiState;
use fe_api::upload::{ingest_glb_with_limit, GlbPayload, UploadError};
use fe_database::RoleLevel;
use fe_identity::api_token::ApiClaims;
use fe_runtime::blob_store::{hash_from_hex, BlobHash, BlobStore, BlobStoreHandle};
use fe_runtime::messages::{
    ApiCommand, DbCommand, DbResult, FractalHierarchyData, NodeHierarchyData, PetalHierarchyData,
    VerseHierarchyData,
};
use fractalengine_test_harness::api::ApiHarness;

/// DEC-C14: the 20-name mcp_scene_primitives vocabulary + 9 shipped extras.
const EXPECTED_TOOLS: [&str; 29] = [
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
    // M2/F7 distributed timeseries query (A17)
    "query_timeseries",
    // F9/A20 sim control (Owner-only; fails closed off a sim lab host)
    "sim_start",
    "sim_stop",
    "sim_status",
    "sim_step",
    "sim_inject_fault",
    // F13 (A26): the 13 remaining mcp_scene_primitives tools
    "upload_asset",
    "place_asset",
    "set_property",
    "get_properties",
    "delete_property",
    "create_waypoint",
    "move_waypoint",
    "import_gpx",
    "set_petal_terrain",
    "list_tilesets",
    "install_tileset",
    "get_gis_nodes",
    "query",
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
    assets: Vec<(String, String)>, // (asset_id, content_hash)
    log: Vec<serde_json::Value>,
}

struct NodeRec {
    id: String,
    petal_id: String,
    name: String,
    position: [f32; 3],
    asset_id: Option<String>,
    properties: serde_json::Map<String, serde_json::Value>,
}

impl Model {
    fn petal_scope(&self, petal_id: &str) -> Option<String> {
        let (_, fid, _) = self.petals.iter().find(|(id, _, _)| id == petal_id)?;
        let (_, vid, _) = self.fractals.iter().find(|(id, _, _)| id == fid)?;
        Some(fe_database::build_scope(vid, Some(fid), Some(petal_id)))
    }

    fn node(&mut self, node_id: &str) -> Option<&mut NodeRec> {
        self.nodes.iter_mut().find(|n| n.id == node_id)
    }

    fn logged(&self, cmd: &str) -> Vec<serde_json::Value> {
        self.log
            .iter()
            .filter(|c| c["cmd"] == cmd)
            .cloned()
            .collect()
    }
}

fn ulid() -> String {
    ulid::Ulid::new().to_string()
}

/// Temp-dir blob store keyed by BLAKE3 (the production FsBlobStore contract).
struct TestBlobStore(tempfile::TempDir);

impl BlobStore for TestBlobStore {
    fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
        let hash = *blake3::hash(bytes).as_bytes();
        std::fs::write(self.0.path().join(hex::encode(hash)), bytes)?;
        Ok(hash)
    }
    fn get_blob_path(&self, hash: &BlobHash) -> Option<std::path::PathBuf> {
        let path = self.0.path().join(hex::encode(hash));
        path.exists().then_some(path)
    }
    fn has_blob(&self, hash: &BlobHash) -> bool {
        self.0.path().join(hex::encode(hash)).exists()
    }
    fn remove_blob(&self, _hash: &BlobHash) -> anyhow::Result<()> {
        Ok(())
    }
}

impl TestBlobStore {
    fn blob_count(&self) -> usize {
        std::fs::read_dir(self.0.path()).unwrap().count()
    }
}

/// Service `api_cmd_rx` like the production DB thread would (in-memory model).
fn spawn_db_emulator(
    rx: crossbeam::channel::Receiver<ApiCommand>,
    model: Arc<Mutex<Model>>,
    store: Option<BlobStoreHandle>,
) {
    tokio::spawn(async move {
        loop {
            match rx.try_recv() {
                Ok(cmd) => handle_command(cmd, &model, store.as_ref()),
                Err(crossbeam::channel::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                Err(crossbeam::channel::TryRecvError::Disconnected) => break,
            }
        }
    });
}

fn hierarchy(m: &Model) -> Vec<VerseHierarchyData> {
    m.verses
        .iter()
        .map(|(vid, vname)| VerseHierarchyData {
            id: vid.clone(),
            name: vname.clone(),
            namespace_id: None,
            timeseries: fe_runtime::timeseries::VerseTimeseriesSettings::default(),
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
                                    has_asset: n.asset_id.is_some(),
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
        .collect()
}

fn handle_command(cmd: ApiCommand, model: &Arc<Mutex<Model>>, store: Option<&BlobStoreHandle>) {
    let mut m = model.lock().expect("model lock");
    match cmd {
        ApiCommand::GetHierarchy { reply_tx } => {
            let _ = reply_tx.send(hierarchy(&m));
        }
        // Fire-and-forget transform persist (the REST + MCP shape).
        ApiCommand::TransformPersist {
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
            if let Some(n) = m.node(&node_id) {
                n.position = position;
            }
        }
        ApiCommand::DbRequest { cmd, reply_tx } => {
            let result = db_command(&mut m, cmd, store);
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}

fn db_command(m: &mut Model, cmd: DbCommand, store: Option<&BlobStoreHandle>) -> DbResult {
    match cmd {
        DbCommand::CreateVerse { name } => {
            let id = ulid();
            m.log.push(json!({ "cmd": "CreateVerse", "name": name }));
            m.verses.push((id.clone(), name.clone()));
            DbResult::VerseCreated {
                id,
                name,
                namespace_id: None,
            }
        }
        DbCommand::CreateFractal { verse_id, name } => {
            let id = ulid();
            m.log
                .push(json!({ "cmd": "CreateFractal", "verse_id": verse_id, "name": name }));
            m.fractals
                .push((id.clone(), verse_id.clone(), name.clone()));
            DbResult::FractalCreated { id, verse_id, name }
        }
        DbCommand::CreatePetal { fractal_id, name } => {
            let id = ulid();
            m.log
                .push(json!({ "cmd": "CreatePetal", "fractal_id": fractal_id, "name": name }));
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
                asset_id: None,
                properties: Default::default(),
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
        DbCommand::ResolvePetalScope { petal_id } => DbResult::ScopeResolved {
            scope: m.petal_scope(&petal_id),
        },
        DbCommand::ResolveNodeScope { node_id } => {
            let petal = m
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .map(|n| n.petal_id.clone());
            DbResult::ScopeResolved {
                scope: petal.and_then(|p| m.petal_scope(&p)),
            }
        }
        DbCommand::ResolveFractalScope { fractal_id } => DbResult::ScopeResolved {
            scope: m
                .fractals
                .iter()
                .find(|(id, _, _)| *id == fractal_id)
                .map(|(_, vid, _)| fe_database::build_scope(vid, Some(&fractal_id), None)),
        },
        DbCommand::ResolveVerseScope { verse_id } => DbResult::ScopeResolved {
            scope: m
                .verses
                .iter()
                .any(|(id, _)| *id == verse_id)
                .then(|| fe_database::build_scope(&verse_id, None, None)),
        },
        DbCommand::SetNodeProperty {
            node_id,
            key,
            value,
        } => {
            m.log
                .push(json!({ "cmd": "SetNodeProperty", "node_id": node_id, "key": key }));
            match m.node(&node_id) {
                Some(n) => {
                    n.properties.insert(key.clone(), value);
                    DbResult::NodePropertySet { node_id, key }
                }
                None => DbResult::Error(format!("SetNodeProperty matched no node {node_id}")),
            }
        }
        DbCommand::GetNodeProperties { node_id } => match m.node(&node_id) {
            Some(n) => DbResult::NodePropertiesLoaded {
                node_id,
                properties: serde_json::Value::Object(n.properties.clone()),
            },
            None => DbResult::Error(format!("no node {node_id}")),
        },
        DbCommand::DeleteNodeProperty { node_id, key } => {
            m.log
                .push(json!({ "cmd": "DeleteNodeProperty", "node_id": node_id, "key": key }));
            if let Some(n) = m.node(&node_id) {
                n.properties.remove(&key);
            }
            DbResult::NodePropertyDeleted { node_id, key }
        }
        // Mirrors create_asset_handler: metadata only; the blob must already exist.
        DbCommand::CreateAsset {
            name,
            content_type,
            size_bytes,
            content_hash,
            correlation_id,
        } => {
            m.log.push(json!({
                "cmd": "CreateAsset", "name": name, "content_type": content_type,
                "size_bytes": size_bytes, "content_hash": content_hash,
            }));
            let present = hash_from_hex(&content_hash)
                .ok()
                .zip(store)
                .is_some_and(|(h, s)| s.has_blob(&h));
            if !present {
                return DbResult::Error("CreateAsset blob is not present in the blob store".into());
            }
            let asset_id = ulid();
            m.assets.push((asset_id.clone(), content_hash.clone()));
            DbResult::AssetCreated {
                asset_id,
                content_hash,
                size_bytes,
                correlation_id,
            }
        }
        DbCommand::CreateNodeWithAsset {
            petal_id,
            name,
            asset_id,
            position,
            correlation_id,
            ..
        } => {
            m.log.push(json!({
                "cmd": "CreateNodeWithAsset", "petal_id": petal_id, "asset_id": asset_id,
            }));
            let Some((_, hash)) = m.assets.iter().find(|(id, _)| *id == asset_id).cloned() else {
                return DbResult::Error(format!(
                    "CreateNodeWithAsset matched no asset with asset_id = {asset_id}"
                ));
            };
            let node_id = ulid();
            m.nodes.push(NodeRec {
                id: node_id.clone(),
                petal_id: petal_id.clone(),
                name: name.clone(),
                position,
                asset_id: Some(asset_id.clone()),
                properties: Default::default(),
            });
            DbResult::GltfImported {
                node_id,
                asset_id,
                petal_id,
                name,
                asset_path: format!("blob://{hash}.glb"),
                position,
                correlation_id,
            }
        }
        DbCommand::TombstoneNode { node_id, .. }
        | DbCommand::CascadeTombstoneNode { node_id, .. } => {
            m.log
                .push(json!({ "cmd": "TombstoneNode", "node_id": node_id }));
            let petal_id = m
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .map(|n| n.petal_id.clone());
            m.nodes.retain(|n| n.id != node_id);
            DbResult::NodeDeleted {
                node_id,
                petal_id: petal_id.unwrap_or_default(),
            }
        }
        DbCommand::GetPetalTerrain { petal_id } => DbResult::PetalTerrainLoaded {
            petal_id,
            terrain: None,
        },
        // gis.rs run_select channel fallback: live nodes of the bound petal.
        DbCommand::RawQuery {
            sql,
            vars,
            correlation_id,
        } => {
            m.log.push(json!({ "cmd": "RawQuery", "sql": sql }));
            let pid = vars.get("pid").and_then(|v| v.as_str()).unwrap_or_default();
            let data = m
                .nodes
                .iter()
                .filter(|n| n.petal_id == pid)
                .map(|n| {
                    json!({
                        "node_id": n.id, "display_name": n.name,
                        "position": { "type": "Point", "coordinates": [n.position[0], n.position[2]] },
                        "elevation": n.position[1],
                        "properties": serde_json::Value::Object(n.properties.clone()),
                    })
                })
                .collect();
            DbResult::QueryResult {
                data,
                correlation_id,
            }
        }
        _ => DbResult::Error("unhandled command in emulator".into()),
    }
}

struct Emu {
    state: Arc<ApiState>,
    model: Arc<Mutex<Model>>,
    store: Arc<TestBlobStore>,
}

/// ApiState wired to the emulator; `with_store` toggles the shared blob store.
fn emu(with_store: bool) -> Emu {
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(64);
    let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let (entity_change_tx, _) = tokio::sync::broadcast::channel(16);
    let store = Arc::new(TestBlobStore(tempfile::tempdir().expect("blob tempdir")));
    let handle: BlobStoreHandle = store.clone();
    let state = Arc::new(ApiState {
        api_cmd_tx,
        transform_broadcast_tx,
        entity_change_tx,
        verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
        revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        blob_store: with_store.then(|| handle.clone()),
        cors_origins: vec![],
        db_reader: None,
        query_rate_limiter: tokio::sync::Mutex::new(HashMap::new()),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    });
    let model = Arc::new(Mutex::new(Model::default()));
    spawn_db_emulator(api_cmd_rx, model.clone(), Some(handle));
    Emu {
        state,
        model,
        store,
    }
}

/// One seeded verse → fractal → petal → node chain in the emulator model.
#[derive(Clone)]
struct Chain {
    verse: String,
    fractal: String,
    petal: String,
    node: String,
}

impl Chain {
    fn verse_scope(&self) -> String {
        format!("VERSE#{}", self.verse)
    }
}

fn seed_chain(model: &Arc<Mutex<Model>>, label: &str) -> Chain {
    let mut m = model.lock().unwrap();
    let chain = Chain {
        verse: ulid(),
        fractal: ulid(),
        petal: ulid(),
        node: ulid(),
    };
    m.verses
        .push((chain.verse.clone(), format!("{label} verse")));
    m.fractals.push((
        chain.fractal.clone(),
        chain.verse.clone(),
        format!("{label} fractal"),
    ));
    m.petals.push((
        chain.petal.clone(),
        chain.fractal.clone(),
        format!("{label} petal"),
    ));
    m.nodes.push(NodeRec {
        id: chain.node.clone(),
        petal_id: chain.petal.clone(),
        name: format!("{label} node"),
        position: [1.0, 2.0, 3.0],
        asset_id: None,
        properties: Default::default(),
    });
    chain
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

/// Poll the emulator log until `cmd` appears (fire-and-forget sends).
async fn await_logged(model: &Arc<Mutex<Model>>, cmd: &str) -> serde_json::Value {
    for _ in 0..500 {
        if let Some(c) = model.lock().unwrap().logged(cmd).pop() {
            return c;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("{cmd} never reached the DB channel");
}

/// A minimal valid GLB: 12-byte header + one empty JSON chunk header.
fn tiny_glb() -> Vec<u8> {
    let json_chunk = br#"{"asset":{"version":"2.0"}} "#; // 28 bytes, 4-byte aligned
    let total = 12 + 8 + json_chunk.len() as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&total.to_le_bytes());
    bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"JSON");
    bytes.extend_from_slice(json_chunk);
    bytes
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
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
        "exact 29-tool inventory (DEC-C14: 24 non-sim + 5 sim control)"
    );
    // tools/list and the dispatch table are one source of truth.
    let table: Vec<&str> = tool_specs().iter().map(|t| t.name).collect();
    assert_eq!(names, table);
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

    // A malformed scope id is rejected by the dispatcher before any lookup.
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
    assert_tool_error(&body, "invalid petal_id");

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
// (e) authz through the real router (real minted tokens, Mem DB scopes)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authz_full_router_role_and_scope() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let foreign = h.seed_hierarchy().await.expect("seed foreign chain");
    let foreign_node = h
        .seed_node(&foreign.petal_id, "foreign", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");

    // No token → 401 from the real auth middleware.
    let (status, _) = h
        .post_json(
            "/mcp",
            None,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let call = |name: &str, args: serde_json::Value| {
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": name, "arguments": args } })
    };

    // Viewer cannot create_verse (manager floor).
    let viewer = h.mint_token("VERSE#v1", "viewer");
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&viewer),
            &call("create_verse", json!({ "name": "V" })),
        )
        .await;
    assert_tool_error(&body, "insufficient permissions");

    // An editor scoped to another verse is denied against the DB-resolved
    // scope of every hierarchy target (no channel round trip needed).
    let editor_a = h.mint_token(&format!("VERSE#{}", ulid()), "editor");
    for (tool, args) in [
        (
            "create_fractal",
            json!({ "verse_id": foreign.verse_id, "name": "F" }),
        ),
        (
            "create_petal",
            json!({ "fractal_id": foreign.fractal_id, "name": "P" }),
        ),
        (
            "create_node",
            json!({ "petal_id": foreign.petal_id, "name": "N" }),
        ),
        (
            "update_transform",
            json!({ "node_id": foreign_node, "position": [0,0,0], "rotation": [0,0,0], "scale": [1,1,1] }),
        ),
    ] {
        let (_, body) = h
            .post_json("/mcp", Some(&editor_a), &call(tool, args))
            .await;
        assert_tool_error(&body, "insufficient scope");
    }

    // A well-formed id that resolves to nothing denies as "not found".
    let editor_foreign = h.mint_token(&foreign.verse_scope(), "editor");
    let (_, body) = h
        .post_json(
            "/mcp",
            Some(&editor_foreign),
            &call("create_node", json!({ "petal_id": ulid(), "name": "N" })),
        )
        .await;
    assert_tool_error(&body, "petal not found");
}

// ---------------------------------------------------------------------------
// (b) full round trip through the real dispatcher (emulated DB thread)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn round_trip_create_read_update_read() {
    let Emu { state, model, .. } = emu(true);

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
    let resp = call_tool(&state, &editor, "get_hierarchy", json!({})).await;
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
    await_logged(&model, "UpdateNodeTransform").await;

    let resp = call_tool(&state, &editor, "get_hierarchy", json!({})).await;
    let node_dto = &tool_json(&resp)[0]["fractals"][0]["petals"][0]["nodes"][0];
    assert_eq!(node_dto["position"], json!([100.5, -2.25, 0.125]));

    // delete_node tombstones it; the hierarchy no longer lists it.
    let resp = call_tool(&state, &editor, "delete_node", json!({ "node_id": nid })).await;
    assert_eq!(tool_json(&resp)["tombstoned"], true);
    let resp = call_tool(&state, &editor, "get_hierarchy", json!({})).await;
    assert_eq!(
        tool_json(&resp)[0]["fractals"][0]["petals"][0]["nodes"],
        json!([])
    );
}

// ---------------------------------------------------------------------------
// (g) per-endpoint CRUD tools (FR-4): role floors + validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_endpoint_crud_tools_role_and_validation() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let ulid = ulid();

    // delete_node / promote_instance require editor: a viewer is rejected before
    // any channel send (fe-policy remains the authoritative gate server-side).
    let viewer = claims(&home.verse_scope(), "viewer");
    let resp = call_tool(&state, &viewer, "delete_node", json!({ "node_id": ulid })).await;
    assert_tool_error(&resp, "insufficient permissions");
    let resp = call_tool(
        &state,
        &viewer,
        "promote_instance",
        json!({ "petal_id": home.petal, "path_id": "p", "instance_index": 0 }),
    )
    .await;
    assert_tool_error(&resp, "insufficient permissions");

    // read_node validates the node id shape (viewer+), rejecting junk ids.
    let resp = call_tool(&state, &viewer, "read_node", json!({ "node_id": "junk" })).await;
    assert_tool_error(&resp, "invalid node_id");

    // A well-formed id with no backing row is unresolvable → denied as not found.
    let editor = claims(&home.verse_scope(), "editor");
    let resp = call_tool(&state, &editor, "read_node", json!({ "node_id": ulid })).await;
    assert_tool_error(&resp, "node not found");

    // promote_instance (authorized) still requires the instance index.
    let resp = call_tool(
        &state,
        &editor,
        "promote_instance",
        json!({ "petal_id": home.petal, "path_id": "path-1" }),
    )
    .await;
    assert_tool_error(&resp, "instance_index is required");
}

// ---------------------------------------------------------------------------
// (f) update_transform unit contract: world units on the wire, no conversion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_transform_world_units_pass_through_unconverted() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let editor = claims(&home.verse_scope(), "editor");

    let resp = call_tool(
        &state,
        &editor,
        "update_transform",
        json!({
            "node_id": home.node,
            "position": [1234.5, -67.25, 0.125],   // world units (meters × world_scale)
            "rotation": [1.5, -0.5, 0.25],          // radians
            "scale": [2.5, 0.5, 1.0]
        }),
    )
    .await;
    assert_eq!(tool_json(&resp), json!({ "status": "ok" }));

    // The persist forwarded by the dispatcher carries the wire values
    // bit-for-bit — no meters/degrees/world_scale conversion in the MCP layer.
    let cmd = await_logged(&model, "UpdateNodeTransform").await;
    assert_eq!(cmd["node_id"], home.node.as_str());
    assert_eq!(cmd["position"], json!([1234.5, -67.25, 0.125]));
    assert_eq!(cmd["rotation"], json!([1.5, -0.5, 0.25]));
    assert_eq!(cmd["scale"], json!([2.5, 0.5, 1.0]));
}

// ---------------------------------------------------------------------------
// (e) the three F13 warts — FIXED (were KNOWN-WEAK role-only paths)
// ---------------------------------------------------------------------------

/// create_node: authz used caller-supplied verse/fractal ids, or none at all.
/// Now the petal's DB-resolved scope decides — a foreign petal is denied with
/// or without (even spoofed) ancestry args; a correctly-scoped token succeeds.
#[tokio::test]
async fn wart_create_node_foreign_petal_is_denied() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let foreign = seed_chain(&model, "foreign");
    let home_editor = claims(&home.verse_scope(), "editor");

    // (1) the old fallback: no ancestry args → used to be role-only.
    let resp = call_tool(
        &state,
        &home_editor,
        "create_node",
        json!({ "petal_id": foreign.petal, "name": "Sneaky" }),
    )
    .await;
    assert_tool_error(&resp, "insufficient scope");
    // (2) spoofed ancestry: the caller's OWN verse/fractal with a foreign petal.
    let resp = call_tool(
        &state,
        &home_editor,
        "create_node",
        json!({ "verse_id": home.verse, "fractal_id": home.fractal,
                "petal_id": foreign.petal, "name": "Sneakier" }),
    )
    .await;
    assert_tool_error(&resp, "hierarchy ids do not match");
    assert!(
        model.lock().unwrap().logged("CreateNode").is_empty(),
        "no write may reach the DB channel"
    );

    // Correctly scoped: succeeds and writes into the resolved petal.
    let resp = call_tool(
        &state,
        &home_editor,
        "create_node",
        json!({ "petal_id": home.petal, "name": "Legit", "position": [1.0, 2.0, 3.0] }),
    )
    .await;
    assert_eq!(tool_json(&resp)["name"], "Legit");
    let created = model.lock().unwrap().logged("CreateNode");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["petal_id"], home.petal.as_str());

    // Strict parsing: a malformed position is an error, never a silent origin.
    let resp = call_tool(
        &state,
        &home_editor,
        "create_node",
        json!({ "petal_id": home.petal, "name": "Bad", "position": "not-an-array" }),
    )
    .await;
    assert_tool_error(&resp, "invalid position");
}

/// create_petal: skipped scope entirely without a verse_id. Now the fractal's
/// DB-resolved scope decides; a petal_id in the args is rejected outright (it
/// would anchor a level deeper than create_petal's write target), so a decoy
/// petal_id cannot redirect authz either.
#[tokio::test]
async fn wart_create_petal_foreign_fractal_is_denied() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let foreign = seed_chain(&model, "foreign");
    let home_editor = claims(&home.verse_scope(), "editor");

    let resp = call_tool(
        &state,
        &home_editor,
        "create_petal",
        json!({ "fractal_id": foreign.fractal, "name": "Sneaky Petal" }),
    )
    .await;
    assert_tool_error(&resp, "insufficient scope");
    // Decoy: authorize against an owned petal while writing a foreign fractal
    // — rejected before any DB call, since create_petal targets a fractal.
    let resp = call_tool(
        &state,
        &home_editor,
        "create_petal",
        json!({ "petal_id": home.petal, "fractal_id": foreign.fractal, "name": "Decoy" }),
    )
    .await;
    assert_tool_error(&resp, "unexpected petal_id");
    assert!(model.lock().unwrap().logged("CreatePetal").is_empty());

    let resp = call_tool(
        &state,
        &home_editor,
        "create_petal",
        json!({ "fractal_id": home.fractal, "name": "Legit Petal" }),
    )
    .await;
    assert_eq!(tool_json(&resp)["name"], "Legit Petal");
    let created = model.lock().unwrap().logged("CreatePetal");
    assert_eq!(created[0]["fractal_id"], home.fractal.as_str());
}

/// update_transform: had NO scope check — any editor could move any node.
/// Now the node's DB-resolved scope decides; nothing is persisted on denial.
#[tokio::test]
async fn wart_update_transform_foreign_node_is_denied() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let foreign = seed_chain(&model, "foreign");
    let home_editor = claims(&home.verse_scope(), "editor");
    let transform = |node: &str| {
        json!({ "node_id": node, "position": [9.0, 9.0, 9.0],
                "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] })
    };

    let resp = call_tool(
        &state,
        &home_editor,
        "update_transform",
        transform(&foreign.node),
    )
    .await;
    assert_tool_error(&resp, "insufficient scope");
    // A nonexistent node no longer reports "ok": unresolvable scope denies.
    let resp = call_tool(&state, &home_editor, "update_transform", transform(&ulid())).await;
    assert_tool_error(&resp, "node not found");
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(model
        .lock()
        .unwrap()
        .logged("UpdateNodeTransform")
        .is_empty());

    let resp = call_tool(
        &state,
        &home_editor,
        "update_transform",
        transform(&home.node),
    )
    .await;
    assert_eq!(tool_json(&resp), json!({ "status": "ok" }));
    assert_eq!(
        await_logged(&model, "UpdateNodeTransform").await["node_id"],
        home.node.as_str()
    );
}

/// Depth escalation (security review, 2026-10-09): the ancestry check alone
/// did not stop a caller anchoring on an id DEEPER than the tool's write
/// target. A petal-scoped Editor token could name its own petal as a decoy
/// on create_fractal/create_petal and escalate into writing a verse-/
/// fractal-level container. Now the write-target depth is rejected outright,
/// and legitimately-scoped callers (verse-scoped → create_fractal,
/// fractal-scoped → create_petal) are unaffected.
#[tokio::test]
async fn wart_depth_escalation_is_denied() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let fractal_scope = fe_database::build_scope(&home.verse, Some(&home.fractal), None);
    let petal_scope = fe_database::build_scope(&home.verse, Some(&home.fractal), Some(&home.petal));

    // (a) petal-scoped Editor token + create_fractal{verse_id, petal_id=own}
    //     → denied with the unexpected-arg error; nothing reaches the channel.
    let petal_editor = claims(&petal_scope, "editor");
    let resp = call_tool(
        &state,
        &petal_editor,
        "create_fractal",
        json!({ "verse_id": home.verse, "petal_id": home.petal, "name": "Escalated Fractal" }),
    )
    .await;
    assert_tool_error(&resp, "unexpected petal_id");
    assert!(model.lock().unwrap().logged("CreateFractal").is_empty());

    // (b) petal-scoped token + create_petal{fractal_id, petal_id=own} → denied.
    let resp = call_tool(
        &state,
        &petal_editor,
        "create_petal",
        json!({ "fractal_id": home.fractal, "petal_id": home.petal, "name": "Escalated Petal" }),
    )
    .await;
    assert_tool_error(&resp, "unexpected petal_id");
    assert!(model.lock().unwrap().logged("CreatePetal").is_empty());

    // (c) verse-scoped Editor token + create_fractal{verse_id} → still succeeds.
    let verse_editor = claims(&home.verse_scope(), "editor");
    let resp = call_tool(
        &state,
        &verse_editor,
        "create_fractal",
        json!({ "verse_id": home.verse, "name": "Legit Fractal" }),
    )
    .await;
    assert_eq!(tool_json(&resp)["name"], "Legit Fractal");

    // (d) fractal-scoped token + create_petal{fractal_id} → still succeeds.
    let fractal_editor = claims(&fractal_scope, "editor");
    let resp = call_tool(
        &state,
        &fractal_editor,
        "create_petal",
        json!({ "fractal_id": home.fractal, "name": "Legit Petal 2" }),
    )
    .await;
    assert_eq!(tool_json(&resp)["name"], "Legit Petal 2");
}

/// The fixes live in the table itself: the wart tools carry DB-resolving
/// rules, each anchored at its own write-target level (2026-10-09: depth
/// escalation hardening replaced the single deepest-id rule with a per-tool
/// `HierarchyTarget`).
#[test]
fn wart_tools_use_db_resolving_scope_rules() {
    let rule = |name: &str| {
        tool_specs()
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.scope_rule)
            .unwrap()
    };
    assert_eq!(
        rule("create_fractal"),
        ScopeRule::HierarchyArgs(HierarchyTarget::Verse)
    );
    assert_eq!(
        rule("create_petal"),
        ScopeRule::HierarchyArgs(HierarchyTarget::Fractal)
    );
    assert_eq!(
        rule("create_node"),
        ScopeRule::HierarchyArgs(HierarchyTarget::Petal)
    );
    assert_eq!(rule("update_transform"), ScopeRule::NodeArg("node_id"));
}

// ---------------------------------------------------------------------------
// A26: table-driven negative RBAC for EVERY tool
// ---------------------------------------------------------------------------

/// The role one step below `level` (Viewer → "none").
fn role_below(level: RoleLevel) -> &'static str {
    match level {
        RoleLevel::Owner => "manager",
        RoleLevel::Manager => "editor",
        RoleLevel::Editor => "viewer",
        RoleLevel::Viewer | RoleLevel::None => "none",
    }
}

#[tokio::test]
async fn every_tool_rejects_a_sub_min_role_token() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    for spec in tool_specs() {
        // Args point at a resource the token's scope DOES cover, so the only
        // reason to deny is the role floor.
        let c = claims(&home.verse_scope(), role_below(spec.min_role));
        let args = json!({
            "petal_id": home.petal, "node_id": home.node,
            "fractal_id": home.fractal, "verse_id": home.verse,
        });
        let resp = call_tool(&state, &c, spec.name, args).await;
        assert_eq!(
            resp["result"]["isError"], true,
            "{} admitted {:?}-1",
            spec.name, spec.min_role
        );
        assert_eq!(
            tool_text(&resp),
            "insufficient permissions",
            "{}",
            spec.name
        );
    }
    assert!(
        model.lock().unwrap().log.is_empty(),
        "no denied call may write"
    );
}

#[tokio::test]
async fn every_scoped_tool_rejects_a_foreign_scope_token() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let foreign = seed_chain(&model, "foreign");
    let mut scoped = 0;
    for spec in tool_specs() {
        if matches!(spec.scope_rule, ScopeRule::None | ScopeRule::Global) {
            continue;
        }
        scoped += 1;
        // Role exactly at the floor; every hierarchy arg names the foreign chain.
        // `HierarchyArgs` rows only get args up to their own write-target depth —
        // anything deeper now fails closed with "unexpected ... id" instead of
        // "insufficient scope", so the blanket assertion below must not hand
        // them an id past their target (2026-10-09 depth-escalation hardening).
        let c = claims(&home.verse_scope(), &spec.min_role.to_string());
        let args = match spec.scope_rule {
            ScopeRule::HierarchyArgs(HierarchyTarget::Verse) => {
                json!({ "verse_id": foreign.verse })
            }
            ScopeRule::HierarchyArgs(HierarchyTarget::Fractal) => {
                json!({ "fractal_id": foreign.fractal, "verse_id": foreign.verse })
            }
            ScopeRule::HierarchyArgs(HierarchyTarget::Petal) => json!({
                "petal_id": foreign.petal, "fractal_id": foreign.fractal, "verse_id": foreign.verse
            }),
            _ => json!({
                "petal_id": foreign.petal, "node_id": foreign.node,
                "fractal_id": foreign.fractal, "verse_id": foreign.verse,
            }),
        };
        let resp = call_tool(&state, &c, spec.name, args).await;
        assert_eq!(
            tool_text(&resp),
            "insufficient scope",
            "{}: {resp}",
            spec.name
        );
    }
    assert_eq!(scoped, 21, "every non-Global/None row is scope-checked");
    assert!(
        model.lock().unwrap().log.is_empty(),
        "no denied call may write"
    );
}

// ---------------------------------------------------------------------------
// H4: GLB upload — where the bytes go
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upload_asset_round_trip_then_place() {
    let Emu {
        state,
        model,
        store,
    } = emu(true);
    let home = seed_chain(&model, "home");
    let editor = claims(&home.verse_scope(), "editor");
    let glb = tiny_glb();

    let resp = call_tool(
        &state,
        &editor,
        "upload_asset",
        json!({ "petal_id": home.petal, "name": "house.glb", "data_base64": b64(&glb) }),
    )
    .await;
    let asset = tool_json(&resp);
    let hash = asset["content_hash"].as_str().expect("hash").to_string();
    assert_eq!(hash, hex::encode(blake3::hash(&glb).as_bytes()));
    assert_eq!(asset["size_bytes"], glb.len());
    // The blob is in the shared store (written API-side) …
    assert!(store.has_blob(&hash_from_hex(&hash).unwrap()));
    // … and the channel carried its metadata (the emulator destructures
    // `CreateAsset` exhaustively — a bytes field would not compile there).
    let create = model.lock().unwrap().logged("CreateAsset");
    assert_eq!(create.len(), 1);
    assert_eq!(create[0]["content_hash"], hash.as_str());
    assert_eq!(create[0]["content_type"], "model/gltf-binary");

    // place_asset binds a node to it; an unknown asset is refused.
    let resp = call_tool(
        &state,
        &editor,
        "place_asset",
        json!({ "petal_id": home.petal, "asset_id": asset["asset_id"], "name": "House",
                "position": [5.0, 0.0, -5.0], "rotation": [0.0, 1.57, 0.0] }),
    )
    .await;
    let placed = tool_json(&resp);
    assert_eq!(placed["asset_path"], format!("blob://{hash}.glb"));
    assert_eq!(placed["petal_id"], home.petal.as_str());
    let resp = call_tool(
        &state,
        &editor,
        "place_asset",
        json!({ "petal_id": home.petal, "asset_id": ulid(), "name": "Ghost" }),
    )
    .await;
    assert_tool_error(&resp, "unknown asset_id");
}

#[tokio::test]
async fn upload_asset_rejects_bad_bytes_and_writes_nothing() {
    let Emu {
        state,
        model,
        store,
    } = emu(true);
    let home = seed_chain(&model, "home");
    let editor = claims(&home.verse_scope(), "editor");
    let upload =
        |data: String| json!({ "petal_id": home.petal, "name": "x.glb", "data_base64": data });

    // JSON .gltf (bad magic), and a v1 GLB, and non-base64 text.
    let resp = call_tool(
        &state,
        &editor,
        "upload_asset",
        upload(b64(br#"{"asset":{}}"#)),
    )
    .await;
    assert_tool_error(&resp, "missing the glTF binary magic");
    let mut v1 = tiny_glb();
    v1[4] = 1;
    let resp = call_tool(&state, &editor, "upload_asset", upload(b64(&v1))).await;
    assert_tool_error(&resp, "unsupported GLB version 1");
    let resp = call_tool(&state, &editor, "upload_asset", upload("@@@".into())).await;
    assert_tool_error(&resp, "not valid standard base64");

    // Oversize, through the same core with an injected cap (no 256 MiB fixture).
    let err = ingest_glb_with_limit(&state, "big.glb", GlbPayload::Base64(b64(&tiny_glb())), 16)
        .await
        .unwrap_err();
    assert_eq!(err, UploadError::TooLarge { max: 16 });

    assert_eq!(store.blob_count(), 0, "nothing written to the blob store");
    assert!(model.lock().unwrap().logged("CreateAsset").is_empty());

    // No blob store configured → fail closed, before decoding anything.
    let Emu {
        state: bare, model, ..
    } = emu(false);
    let home = seed_chain(&model, "home");
    let resp = call_tool(
        &bare,
        &claims(&home.verse_scope(), "editor"),
        "upload_asset",
        json!({ "petal_id": home.petal, "name": "x.glb", "data_base64": b64(&tiny_glb()) }),
    )
    .await;
    assert_tool_error(&resp, "no blob store configured");
}

// ---------------------------------------------------------------------------
// H5: one happy-path dispatch per new tool (scoped token → right core/command)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn property_tools_round_trip() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let editor = claims(&home.verse_scope(), "editor");

    let resp = call_tool(
        &state,
        &editor,
        "set_property",
        json!({ "node_id": home.node, "key": "gis.annotation.title", "value": "Hello" }),
    )
    .await;
    assert_eq!(tool_json(&resp)["key"], "gis.annotation.title");

    let viewer = claims(&home.verse_scope(), "viewer");
    let resp = call_tool(
        &state,
        &viewer,
        "get_properties",
        json!({ "node_id": home.node }),
    )
    .await;
    assert_eq!(
        tool_json(&resp)["properties"]["gis.annotation.title"],
        "Hello"
    );

    // GIS read sees the annotation (run_select → RawQuery channel fallback).
    let resp = call_tool(
        &state,
        &viewer,
        "get_gis_nodes",
        json!({ "petal_id": home.petal }),
    )
    .await;
    let gis = tool_json(&resp);
    assert_eq!(gis["nodes"][0]["node_id"], home.node.as_str());
    assert_eq!(gis["nodes"][0]["annotation"]["title"], "Hello");
    let resp = call_tool(
        &state,
        &viewer,
        "get_gis_nodes",
        json!({ "petal_id": home.petal, "bbox": "100,100,200,200" }),
    )
    .await;
    assert_eq!(tool_json(&resp)["nodes"], json!([]), "bbox filter applies");

    let resp = call_tool(
        &state,
        &editor,
        "delete_property",
        json!({ "node_id": home.node, "key": "gis.annotation.title" }),
    )
    .await;
    tool_json(&resp);
    let resp = call_tool(
        &state,
        &viewer,
        "get_properties",
        json!({ "node_id": home.node }),
    )
    .await;
    assert_eq!(tool_json(&resp)["properties"], json!({}));
}

#[tokio::test]
async fn waypoint_and_gpx_tools_round_trip() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let editor = claims(&home.verse_scope(), "editor");

    let resp = call_tool(
        &state,
        &editor,
        "create_waypoint",
        json!({ "petal_id": home.petal, "name": "Summit", "lat": 47.6, "lon": -122.3, "ele": 56.0 }),
    )
    .await;
    let wp = tool_json(&resp)["id"]
        .as_str()
        .expect("waypoint id")
        .to_string();
    {
        let mut m = model.lock().unwrap();
        let node = m.node(&wp).expect("waypoint node created");
        assert_eq!(node.petal_id, home.petal);
        assert_eq!(node.properties["waypoint"], true);
    }

    let resp = call_tool(
        &state,
        &editor,
        "move_waypoint",
        json!({ "node_id": wp, "lat": 47.7, "lon": -122.4 }),
    )
    .await;
    assert_eq!(tool_json(&resp)["lat"], 47.7);
    assert_eq!(
        await_logged(&model, "UpdateNodeTransform").await["node_id"],
        wp.as_str()
    );

    let gpx = r#"<?xml version="1.0"?><gpx version="1.1" creator="t" xmlns="http://www.topografix.com/GPX/1/1">
      <wpt lat="47.60" lon="-122.33"><ele>5</ele><name>A</name></wpt>
      <trk><name>T</name><trkseg>
        <trkpt lat="47.60" lon="-122.33"><ele>5</ele></trkpt>
        <trkpt lat="47.61" lon="-122.34"><ele>6</ele></trkpt>
      </trkseg></trk></gpx>"#;
    let before = model.lock().unwrap().logged("CreateNode").len();
    let resp = call_tool(
        &state,
        &editor,
        "import_gpx",
        json!({ "petal_id": home.petal, "data_base64": b64(gpx.as_bytes()) }),
    )
    .await;
    let summary = tool_json(&resp);
    assert_eq!(summary["track_count"], 1);
    assert_eq!(summary["errors"], 0);
    let created = model.lock().unwrap().logged("CreateNode").len() - before;
    assert_eq!(summary["nodes_created"], created);
    assert!(created > 0);

    let resp = call_tool(
        &state,
        &editor,
        "import_gpx",
        json!({ "petal_id": home.petal, "data_base64": b64(b"<not-gpx/>") }),
    )
    .await;
    assert_tool_error(&resp, "invalid GPX");
}

#[tokio::test]
async fn terrain_and_tileset_tools_reach_their_cores() {
    let Emu { state, model, .. } = emu(true);
    let home = seed_chain(&model, "home");
    let editor = claims(&home.verse_scope(), "editor");

    // set_petal_terrain mirrors REST: validated, then refused (uncorrelated reply).
    let resp = call_tool(
        &state,
        &editor,
        "set_petal_terrain",
        json!({ "petal_id": home.petal, "terrain": { "bogus": true } }),
    )
    .await;
    assert_tool_error(&resp, "invalid terrain config");
    let resp = call_tool(
        &state,
        &editor,
        "set_petal_terrain",
        json!({ "petal_id": home.petal, "terrain": null }),
    )
    .await;
    assert_tool_error(&resp, "temporarily unavailable");

    // list_tilesets: the petal binds none → empty list.
    let viewer = claims(&home.verse_scope(), "viewer");
    let resp = call_tool(
        &state,
        &viewer,
        "list_tilesets",
        json!({ "petal_id": home.petal }),
    )
    .await;
    assert_eq!(tool_json(&resp), json!([]));

    // install_tileset: past the Manager gate + fe-policy, the core reports
    // the host has no tileset registry (no write attempted).
    let manager = claims(&home.verse_scope(), "manager");
    let resp = call_tool(
        &state,
        &manager,
        "install_tileset",
        json!({ "petal_id": home.petal, "data_base64": b64(b"archive") }),
    )
    .await;
    assert_tool_error(&resp, "tileset registry not available");
}

/// `query` runs the REST /api/v1/query local path: SELECT-only + token-scope
/// filter injection over the real db_reader (full router, petal-scoped token).
#[tokio::test]
async fn query_tool_uses_the_guarded_local_pipeline() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let chain = h.seed_hierarchy().await.expect("seed");
    let other_petal = h
        .seed_petal(&chain.fractal_id, "Other", None)
        .await
        .expect("seed petal");
    let mine = h
        .seed_node(&chain.petal_id, "mine", [0.0, 0.0, 0.0], None)
        .await
        .unwrap();
    h.seed_node(&other_petal, "theirs", [0.0, 0.0, 0.0], None)
        .await
        .unwrap();
    let petal_scope = fe_database::build_scope(
        &chain.verse_id,
        Some(&chain.fractal_id),
        Some(&chain.petal_id),
    );
    let token = h.mint_token(&petal_scope, "viewer");
    let call = |sql: &str| {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "query", "arguments": { "sql": sql } } })
    };

    let (_, body) = h
        .post_json("/mcp", Some(&token), &call("SELECT node_id FROM node"))
        .await;
    let rows = tool_json(&body)["data"].clone();
    assert_eq!(rows, json!([{ "node_id": mine }]), "scope filter injected");

    let (_, body) = h
        .post_json("/mcp", Some(&token), &call("DELETE node"))
        .await;
    assert_tool_error(&body, "only SELECT statements are allowed");
}
