use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json};
use serde::{Deserialize, Serialize};

use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};

use crate::server::ApiState;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

// ---------------------------------------------------------------------------
// MCP tool definitions
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ToolDefinition {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "get_hierarchy".into(),
            description: "Get the full verse/fractal/petal/node hierarchy".into(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        },
        ToolDefinition {
            name: "create_verse".into(),
            description: "Create a new verse".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "create_node".into(),
            description: "Create a new node in a petal".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "petal_id": { "type": "string" },
                    "name": { "type": "string" },
                    "position": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3
                    }
                },
                "required": ["petal_id", "name"]
            }),
        },
        ToolDefinition {
            name: "update_transform".into(),
            description: "Update a node's position, rotation, and scale".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" },
                    "position": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3
                    },
                    "rotation": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3
                    },
                    "scale": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3
                    }
                },
                "required": ["node_id", "position", "rotation", "scale"]
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Main handler
// ---------------------------------------------------------------------------

pub async fn mcp_handler(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = match req.method.as_str() {
        // MCP initialize handshake
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "fractalengine",
                    "version": "0.1.0"
                }
            })),
            error: None,
        },

        // List available tools
        "tools/list" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({ "tools": tool_definitions() })),
            error: None,
        },

        // Invoke a tool
        "tools/call" => {
            let tool_name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            handle_tool_call(&state, req.id, tool_name, arguments).await
        }

        // Notifications — acknowledge but return an empty result
        "notifications/initialized" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({})),
            error: None,
        },

        _ => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("method not found: {}", req.method),
            }),
        },
    };

    Json(response)
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

async fn handle_tool_call(
    state: &ApiState,
    id: Option<serde_json::Value>,
    tool_name: &str,
    args: serde_json::Value,
) -> JsonRpcResponse {
    match tool_name {
        "get_hierarchy" => {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::GetHierarchy { reply_tx });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(data)) => {
                    let dto: Vec<crate::types::VerseDto> = crate::types::hierarchy_to_dto(&data);
                    tool_result(id, serde_json::to_value(dto).unwrap_or_default())
                }
                _ => tool_error(id, "hierarchy request failed"),
            }
        }

        "create_verse" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                return tool_error(id, "name is required");
            }
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                cmd: DbCommand::CreateVerse { name },
                reply_tx,
            });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(DbResult::VerseCreated {
                    id: vid,
                    name: vname,
                })) => tool_result(id, serde_json::json!({ "id": vid, "name": vname })),
                Ok(Ok(DbResult::Error(e))) => tool_error(id, &e),
                _ => tool_error(id, "create_verse failed"),
            }
        }

        "create_node" => {
            let petal_id = args
                .get("petal_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let position: [f32; 3] = args
                .get("position")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0, 0.0, 0.0]);
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                cmd: DbCommand::CreateNode {
                    petal_id,
                    name,
                    position,
                },
                reply_tx,
            });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(DbResult::NodeCreated {
                    id: nid,
                    name: nname,
                    ..
                })) => tool_result(id, serde_json::json!({ "id": nid, "name": nname })),
                Ok(Ok(DbResult::Error(e))) => tool_error(id, &e),
                _ => tool_error(id, "create_node failed"),
            }
        }

        "update_transform" => {
            let node_id = args
                .get("node_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let position: [f32; 3] = args
                .get("position")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0, 0.0, 0.0]);
            let rotation: [f32; 3] = args
                .get("rotation")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0, 0.0, 0.0]);
            let scale: [f32; 3] = args
                .get("scale")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([1.0, 1.0, 1.0]);

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                cmd: DbCommand::UpdateNodeTransform {
                    node_id: node_id.clone(),
                    position,
                    rotation,
                    scale,
                },
                reply_tx,
            });

            // Also fan-out on the real-time broadcast channel
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let _ = state
                .transform_broadcast_tx
                .send(fe_runtime::messages::TransformUpdate {
                    node_id,
                    petal_id: String::new(),
                    position,
                    rotation,
                    scale,
                    timestamp_ms: now,
                    source_did: String::new(),
                });

            // Fire-and-forget: don't wait for the DB ack
            drop(reply_rx);
            tool_result(id, serde_json::json!({ "status": "ok" }))
        }

        _ => tool_error(id, &format!("unknown tool: {tool_name}")),
    }
}

// ---------------------------------------------------------------------------
// MCP content helpers
// ---------------------------------------------------------------------------

fn tool_result(id: Option<serde_json::Value>, content: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(serde_json::json!({
            "content": [{ "type": "text", "text": serde_json::to_string(&content).unwrap_or_default() }]
        })),
        error: None,
    }
}

fn tool_error(id: Option<serde_json::Value>, msg: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(serde_json::json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true
        })),
        error: None,
    }
}
