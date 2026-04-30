use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json};
use axum::Extension;
use serde::{Deserialize, Serialize};

use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};

use crate::auth::{require_role, require_role_and_scope};
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
            description: "Get the full verse/fractal/petal/node hierarchy (filtered by token scope)".into(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        },
        ToolDefinition {
            name: "create_verse".into(),
            description: "Create a new verse (requires manager role)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "create_fractal".into(),
            description: "Create a fractal in a verse (requires editor role + verse scope)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "verse_id": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["verse_id", "name"]
            }),
        },
        ToolDefinition {
            name: "create_petal".into(),
            description: "Create a petal in a fractal (requires editor role + fractal scope)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "verse_id": { "type": "string" },
                    "fractal_id": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["verse_id", "fractal_id", "name"]
            }),
        },
        ToolDefinition {
            name: "create_node".into(),
            description: "Create a new node in a petal (requires editor role + scope)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "verse_id": { "type": "string", "description": "Parent verse ID (required for scope enforcement)" },
                    "fractal_id": { "type": "string", "description": "Parent fractal ID (required for scope enforcement)" },
                    "petal_id": { "type": "string" },
                    "name": { "type": "string" },
                    "position": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3
                    }
                },
                "required": ["verse_id", "fractal_id", "petal_id", "name"]
            }),
        },
        ToolDefinition {
            name: "update_transform".into(),
            description: "Update a node's position, rotation, and scale (requires editor role)".into(),
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
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = match req.method.as_str() {
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

        "tools/list" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({ "tools": tool_definitions() })),
            error: None,
        },

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
            handle_tool_call(&state, &claims, req.id, tool_name, arguments).await
        }

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
    claims: &ApiClaims,
    id: Option<serde_json::Value>,
    tool_name: &str,
    args: serde_json::Value,
) -> JsonRpcResponse {
    match tool_name {
        "get_hierarchy" => {
            if require_role(claims, "viewer").is_err() {
                return tool_error(id, "insufficient permissions");
            }

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::GetHierarchy { reply_tx });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(data)) => {
                    let dto: Vec<crate::types::VerseDto> = crate::types::hierarchy_to_dto(&data);
                    // Filter by token scope (same as REST handler)
                    let filtered = crate::rest::filter_hierarchy_by_scope(dto, &claims.scope);
                    tool_result(id, serde_json::to_value(filtered).unwrap_or_default())
                }
                _ => tool_error(id, "hierarchy request failed"),
            }
        }

        "create_verse" => {
            if require_role(claims, "manager").is_err() {
                return tool_error(id, "insufficient permissions");
            }

            let name = str_arg(&args, "name");
            if name.is_empty() {
                return tool_error(id, "name is required");
            }
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                cmd: DbCommand::CreateVerse { name },
                reply_tx,
            });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(DbResult::VerseCreated { id: vid, name: vname })) => {
                    tool_result(id, serde_json::json!({ "id": vid, "name": vname }))
                }
                Ok(Ok(DbResult::Error(e))) => {
                    tracing::error!("create_verse MCP failed: {e}");
                    tool_error(id, "operation failed")
                }
                _ => tool_error(id, "create_verse failed"),
            }
        }

        "create_fractal" => {
            let verse_id = str_arg(&args, "verse_id");
            let name = str_arg(&args, "name");
            if verse_id.is_empty() || name.is_empty() {
                return tool_error(id, "verse_id and name are required");
            }

            let scope = fe_database::build_scope(&verse_id, None, None);
            if require_role_and_scope(claims, "editor", &scope).is_err() {
                return tool_error(id, "insufficient permissions or scope");
            }

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                cmd: DbCommand::CreateFractal { verse_id, name },
                reply_tx,
            });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(DbResult::FractalCreated { id: fid, name: fname, .. })) => {
                    tool_result(id, serde_json::json!({ "id": fid, "name": fname }))
                }
                Ok(Ok(DbResult::Error(e))) => {
                    tracing::error!("create_fractal MCP failed: {e}");
                    tool_error(id, "operation failed")
                }
                _ => tool_error(id, "create_fractal failed"),
            }
        }

        "create_petal" => {
            let verse_id = str_arg(&args, "verse_id");
            let fractal_id = str_arg(&args, "fractal_id");
            let name = str_arg(&args, "name");
            if fractal_id.is_empty() || name.is_empty() {
                return tool_error(id, "fractal_id and name are required");
            }

            if !verse_id.is_empty() {
                let scope = fe_database::build_scope(&verse_id, Some(&fractal_id), None);
                if require_role_and_scope(claims, "editor", &scope).is_err() {
                    return tool_error(id, "insufficient permissions or scope");
                }
            } else if require_role(claims, "editor").is_err() {
                return tool_error(id, "insufficient permissions");
            }

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                cmd: DbCommand::CreatePetal { fractal_id, name },
                reply_tx,
            });
            match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
                Ok(Ok(DbResult::PetalCreated { id: pid, name: pname, .. })) => {
                    tool_result(id, serde_json::json!({ "id": pid, "name": pname }))
                }
                Ok(Ok(DbResult::Error(e))) => {
                    tracing::error!("create_petal MCP failed: {e}");
                    tool_error(id, "operation failed")
                }
                _ => tool_error(id, "create_petal failed"),
            }
        }

        "create_node" => {
            let petal_id = str_arg(&args, "petal_id");
            let name = str_arg(&args, "name");
            let verse_id = str_arg(&args, "verse_id");
            let fractal_id = str_arg(&args, "fractal_id");

            if petal_id.is_empty() || name.is_empty() {
                return tool_error(id, "petal_id and name are required");
            }

            // Scope enforcement: require hierarchy IDs for proper scope check
            if !verse_id.is_empty() && !fractal_id.is_empty() {
                let scope = fe_database::build_scope(&verse_id, Some(&fractal_id), Some(&petal_id));
                if require_role_and_scope(claims, "editor", &scope).is_err() {
                    return tool_error(id, "insufficient permissions or scope");
                }
            } else {
                // Fallback: role-only check when hierarchy IDs not provided.
                // NOTE: This path lacks scope enforcement. Callers should provide
                // verse_id and fractal_id for proper scope validation.
                if require_role(claims, "editor").is_err() {
                    return tool_error(id, "insufficient permissions");
                }
            }

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
                Ok(Ok(DbResult::NodeCreated { id: nid, name: nname, .. })) => {
                    tool_result(id, serde_json::json!({ "id": nid, "name": nname }))
                }
                Ok(Ok(DbResult::Error(e))) => {
                    tracing::error!("create_node MCP failed: {e}");
                    tool_error(id, "operation failed")
                }
                _ => tool_error(id, "create_node failed"),
            }
        }

        "update_transform" => {
            if require_role(claims, "editor").is_err() {
                return tool_error(id, "insufficient permissions");
            }

            let node_id = str_arg(&args, "node_id");
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
                    source_did: claims.sub.clone(),
                });

            drop(reply_rx);
            tool_result(id, serde_json::json!({ "status": "ok" }))
        }

        _ => tool_error(id, &format!("unknown tool: {tool_name}")),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn str_arg(args: &serde_json::Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

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
