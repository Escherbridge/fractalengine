//! MCP JSON-RPC endpoint (`POST /mcp`): one table-driven dispatcher — see
//! `fe-api/AGENTS.md` §mcp-dispatch (ToolSpec/ScopeRule contract, DEC-C14/C15).
//!
//! Authz lives ONLY in [`authorize`]: role → DB-resolved scope → containment,
//! before any handler runs. Handlers (`tools.rs`) never call `require_*`.

mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json};
use axum::Extension;
use fe_database::RoleLevel;
use serde::{Deserialize, Serialize};

use fe_identity::api_token::ApiClaims;

use crate::auth::{require_role_level, require_scope};
use crate::endpoint::is_addressable_node_id;
use crate::server::ApiState;
use crate::types::is_valid_ulid;

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
// The tool table (FR-5 / DEC-C15)
// ---------------------------------------------------------------------------

/// How the dispatcher derives the resource scope a call must be covered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeRule {
    /// Self-filtering read: the handler narrows output to the token scope.
    None,
    /// No resource scope exists (verse creation, sim lab) — role-only by design.
    Global,
    /// `args[key]` is a petal id; its DB-resolved scope must be covered.
    PetalArg(&'static str),
    /// `args[key]` is a node id; its DB-resolved scope must be covered.
    NodeArg(&'static str),
    /// The id AT the tool's write-target level is DB-resolved; every
    /// shallower id present must match that stored chain; any id DEEPER
    /// than the target is rejected (2026-10-09 depth-escalation hardening —
    /// the anchor and the write target can never be two different levels).
    HierarchyArgs(HierarchyTarget),
}

/// The hierarchy level a `HierarchyArgs` tool writes into — authz anchors
/// exactly here, never at a deeper id the caller happens to supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyTarget {
    /// create_fractal writes a fractal INTO a verse.
    Verse,
    /// create_petal writes a petal INTO a fractal.
    Fractal,
    /// create_node writes a node INTO a petal.
    Petal,
}

/// One authorized call, handed to a handler only after [`authorize`] passed.
pub(crate) struct ToolCall<'a> {
    pub state: &'a ApiState,
    pub claims: &'a ApiClaims,
    pub args: &'a serde_json::Value,
    /// DB-resolved scope for `PetalArg`/`NodeArg`/`HierarchyArgs`; `None` otherwise.
    pub scope: Option<&'a str>,
}

/// A handler's payload, or the client-facing error text.
pub(crate) type ToolOutcome = Result<serde_json::Value, String>;
pub(crate) type ToolFuture<'a> = Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>;
pub(crate) type ToolHandler = for<'a> fn(ToolCall<'a>) -> ToolFuture<'a>;

/// One MCP tool: `tools/list` and `tools/call` both derive from [`TOOLS`].
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: fn() -> serde_json::Value,
    pub min_role: RoleLevel,
    pub scope_rule: ScopeRule,
    pub(crate) handler: ToolHandler,
}

/// The full tool table (read-only view for tests and docs).
pub fn tool_specs() -> &'static [ToolSpec] {
    TOOLS
}

macro_rules! tool {
    ($name:ident, $role:ident, $rule:expr, $desc:expr) => {
        ToolSpec {
            name: stringify!($name),
            description: $desc,
            input_schema: tools::schema::$name,
            min_role: RoleLevel::$role,
            scope_rule: $rule,
            handler: tools::$name,
        }
    };
}

use HierarchyTarget::{Fractal, Petal, Verse};
use ScopeRule::{Global, HierarchyArgs, NodeArg, PetalArg};

/// DEC-C14: the 20-name mcp_scene_primitives vocabulary + 9 shipped extras = 29.
static TOOLS: &[ToolSpec] = &[
    tool!(get_hierarchy, Viewer, ScopeRule::None,
        "Get the full verse/fractal/petal/node hierarchy, filtered to the token scope. Requires viewer role."),
    tool!(create_verse, Manager, Global,
        "Create a new verse. Requires manager role (verses have no parent scope)."),
    tool!(create_fractal, Editor, HierarchyArgs(Verse),
        "Create a fractal in an existing verse; a fractal_id or petal_id in the args is rejected (would escalate past the verse anchor). Requires editor role + the verse's scope."),
    tool!(create_petal, Editor, HierarchyArgs(Fractal),
        "Create a petal in an existing fractal; authorized against the fractal's stored hierarchy (verse_id, if sent, must match it; a petal_id in the args is rejected). Requires editor role + scope."),
    tool!(create_node, Editor, HierarchyArgs(Petal),
        "Create an empty node in an existing petal; authorized against the petal's stored hierarchy (verse_id/fractal_id, if sent, must match it). Requires editor role + scope."),
    tool!(update_transform, Editor, NodeArg("node_id"),
        "Update a node's position, rotation, and scale (persisted asynchronously, broadcast to live subscribers). Requires editor role + the node's scope."),
    tool!(read_node, Viewer, NodeArg("node_id"),
        "Read an object's full payload by node id (common fields + type-specific stamp/earthwork/path data). Tombstoned nodes read as not found. Requires viewer role + scope."),
    tool!(node_address, Viewer, NodeArg("node_id"),
        "Resolve a node id to its stable fe://verse/fractal/petal/node endpoint URI. Requires viewer role + scope."),
    tool!(delete_node, Editor, NodeArg("node_id"),
        "Sync-safe tombstone delete of a node (never a raw drop; survives P2P merge). Set cascade=true to tombstone the whole subtree atomically. Requires editor role + scope; authorized by fe-policy."),
    tool!(promote_instance, Editor, PetalArg("petal_id"),
        "Materialize a full addressable node for a single stamp instance (lazy promotion, idempotent). Requires editor role + petal scope; authorized by fe-policy."),
    tool!(query_timeseries, Viewer, PetalArg("petal_id"),
        "Run a distributed timeseries query over a petal's sharded IoT fabric. The query fans out to online peers hosting the petal's shards and merges commutatively (aggregate: mean/min/max/count as an exact monoid; raw rows: union deduped by reading_id; latest: max timestamp per anchor). Results carry covered/missing shard honesty metadata — an offline peer's shards are invisible at replication factor 1, served from a surviving mirror at R>=2. Requires viewer role + petal scope."),
    tool!(sim_start, Owner, Global,
        "Start a deterministic simulation-lab session (fe-sim): spawns in-process simulated peers on a virtual network and prepares the scripted fleet. One session at a time. Requires owner role and a host started as a sim lab."),
    tool!(sim_stop, Owner, Global,
        "Stop the live sim session: settle every peer, return the canonical outcome fingerprint, and tear it down. Requires owner role."),
    tool!(sim_status, Owner, Global,
        "Snapshot the live sim session (peers + online state, action cursor, sim clock offset, readings ingested, injected faults, network counters); {running:false} when idle. Requires owner role."),
    tool!(sim_step, Owner, Global,
        "Execute the next n scripted actions (fleet ticks, scripted faults, distributed queries) of the live sim session; returns the queries completed. Requires owner role."),
    tool!(sim_inject_fault, Owner, Global,
        "Apply a network fault to the live sim session NOW: peer_offline / peer_online {peer}, partition {groups}, heal, set_latency {latency_ms}. Requires owner role."),
    // --- F13: the 13 mcp_scene_primitives tools (DEC-C14) ---
    tool!(upload_asset, Editor, PetalArg("petal_id"),
        "Upload a GLB (binary glTF 2.0, textures embedded) as base64; stored content-addressed (BLAKE3) and registered as an asset row. Max 11 MiB decoded over MCP (the /mcp body cap is 16 MiB); larger GLBs (up to 256 MiB) go through REST multipart POST /api/v1/petals/{petal_id}/assets. Returns {asset_id, content_hash, size_bytes}. Requires editor role + the anchor petal's scope."),
    tool!(place_asset, Editor, PetalArg("petal_id"),
        "Create a node in a petal bound to an uploaded asset_id, at position (world units) with Euler XYZ rotation (radians) and scale. The asset must be unplaced (fresh upload) or already used by a node within the token's scope. Requires editor role + petal scope."),
    tool!(set_property, Editor, NodeArg("node_id"),
        "Set one custom property (any JSON value) on a node. Requires editor role + the node's scope."),
    tool!(get_properties, Viewer, NodeArg("node_id"),
        "Read all custom properties of a node. Requires viewer role + the node's scope."),
    tool!(delete_property, Editor, NodeArg("node_id"),
        "Delete one custom property from a node. Requires editor role + the node's scope."),
    tool!(create_waypoint, Editor, PetalArg("petal_id"),
        "Create a waypoint node at WGS84 lat/lon/ele, projected through the petal's terrain origin. Requires editor role + petal scope."),
    tool!(move_waypoint, Editor, NodeArg("node_id"),
        "Move a waypoint node to a new WGS84 lat/lon (ele optional), reprojecting its transform. Requires editor role + the node's scope."),
    tool!(import_gpx, Editor, PetalArg("petal_id"),
        "Import a GPX document (base64, max 11 MiB decoded, max 10000 points) as track/waypoint nodes in a petal. Requires editor role + petal scope."),
    tool!(set_petal_terrain, Editor, PetalArg("petal_id"),
        "Set (object) or clear (null) a petal's terrain config. Currently refused after validation exactly like REST PUT/DELETE terrain: durable SetPetalTerrain replies are not yet correlated. Requires editor role + petal scope."),
    tool!(list_tilesets, Viewer, PetalArg("petal_id"),
        "List the installed terrain tilesets bound to a petal's terrain config. Requires viewer role + petal scope."),
    tool!(install_tileset, Editor, PetalArg("petal_id"),
        "Install a .hexon terrain-tileset archive (base64, max 2 MiB decoded — the REST install route's cap) already bound only to this petal's terrain. Requires editor role + petal scope; authorized by fe-policy (Install)."),
    tool!(get_gis_nodes, Viewer, PetalArg("petal_id"),
        "List a petal's geo-positioned nodes with gis.annotation.* data; optional single spatial filter (bbox, bbox_ll, or radius+cx+cz). Requires viewer role + petal scope."),
    tool!(query, Viewer, ScopeRule::None,
        "Run a read-only SurrealQL SELECT through the /api/v1/query guard pipeline (rate limit, SELECT-only, table whitelist, token-scope filter injection, row/byte caps). Requires viewer role."),
];

// ---------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ToolDefinition {
    name: &'static str,
    description: &'static str,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

fn tool_definitions() -> Vec<ToolDefinition> {
    TOOLS
        .iter()
        .map(|t| ToolDefinition {
            name: t.name,
            description: t.description,
            input_schema: (t.input_schema)(),
        })
        .collect()
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
            let mut params = req.params;
            // Moved out, never cloned: a large base64 argument exists once (DEC-C21).
            let arguments = params
                .get_mut("arguments")
                .map(serde_json::Value::take)
                .unwrap_or(serde_json::Value::Null);
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
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
// The dispatcher: lookup → authorize → required args → handler
// ---------------------------------------------------------------------------

async fn handle_tool_call(
    state: &ApiState,
    claims: &ApiClaims,
    id: Option<serde_json::Value>,
    tool_name: &str,
    args: serde_json::Value,
) -> JsonRpcResponse {
    let Some(spec) = TOOLS.iter().find(|t| t.name == tool_name) else {
        return tool_error(id, &format!("unknown tool: {tool_name}"));
    };
    // Non-object arguments degrade to `{}` (then fail the required-args check).
    let args = if args.is_object() {
        args
    } else {
        serde_json::json!({})
    };

    let scope = match authorize(state, claims, spec, &args).await {
        Ok(scope) => scope,
        Err(reason) => {
            tracing::warn!(tool = spec.name, sub = %claims.sub, %reason, "MCP tool call denied");
            return tool_error(id, &reason);
        }
    };
    if let Err(msg) = check_required_args(spec, &args) {
        return tool_error(id, &msg);
    }
    let call = ToolCall {
        state,
        claims,
        args: &args,
        scope: scope.as_deref(),
    };
    match (spec.handler)(call).await {
        Ok(payload) => tool_result(id, payload),
        Err(msg) => tool_error(id, &msg),
    }
}

/// Ancestor ids a `HierarchyArgs` caller claimed ABOVE the write target.
#[derive(Default)]
struct ClaimedAncestry<'a> {
    verse_id: Option<&'a str>,
    fractal_id: Option<&'a str>,
}

/// THE MCP authz gate: role floor, then the spec's scope rule resolved from
/// the DB (never from caller-supplied ancestry), then token containment, and
/// only THEN the claimed-ancestry match. Not-found, out-of-scope and
/// ancestry-mismatch all deny with ONE message (DEC-C21 — no existence
/// oracle). Returns the resolved scope for the handler.
async fn authorize(
    state: &ApiState,
    claims: &ApiClaims,
    spec: &ToolSpec,
    args: &serde_json::Value,
) -> Result<Option<String>, String> {
    if require_role_level(claims, spec.min_role).is_err() {
        return Err("insufficient permissions".to_string());
    }
    let (resolved, claimed) = match spec.scope_rule {
        ScopeRule::None | ScopeRule::Global => return Ok(None),
        ScopeRule::PetalArg(key) => {
            let petal_id = id_arg(args, key, is_valid_ulid)?;
            let scope = crate::rest::resolve_petal_scope(state, petal_id).await;
            (scope, ClaimedAncestry::default())
        }
        ScopeRule::NodeArg(key) => {
            let node_id = id_arg(args, key, is_addressable_node_id)?;
            let scope = crate::rest::resolve_node_scope(state, node_id).await;
            (scope, ClaimedAncestry::default())
        }
        ScopeRule::HierarchyArgs(target) => resolve_hierarchy_scope(state, args, target).await?,
    };
    let denied = || crate::rest::NOT_FOUND_OR_DENIED.to_string();
    let scope = resolved.ok_or_else(denied)?;
    if require_scope(claims, &scope).is_err() {
        return Err(denied());
    }
    // Ancestry is checked AFTER containment, so a mismatch reveals nothing
    // about a container the token cannot see.
    if !crate::rest::ancestry_matches(&scope, claimed.verse_id, claimed.fractal_id) {
        return Err(denied());
    }
    Ok(Some(scope))
}

/// `args[key]` as a non-empty id passing `valid`.
fn id_arg<'a>(
    args: &'a serde_json::Value,
    key: &str,
    valid: fn(&str) -> bool,
) -> Result<&'a str, String> {
    match args.get(key).and_then(|v| v.as_str()) {
        None | Some("") => Err(format!("{key} is required")),
        Some(id) if !valid(id) => Err(format!("invalid {key}")),
        Some(id) => Ok(id),
    }
}

/// `HierarchyArgs`: resolve the id AT the tool's write-target level from the
/// DB — never the deepest id present — and return it with the ids SHALLOWER
/// than the target (which [`authorize`] matches against the stored chain
/// after containment; closes the cross-container decoy wart). Any id DEEPER
/// than the target is rejected outright before any DB call (2026-10-09
/// depth-escalation hardening — a petal-scoped token must not
/// create_fractal/create_petal by naming its own petal as an anchor one or
/// two levels below the actual write target; see fe-api/AGENTS.md
/// §mcp-dispatch). `Ok((None, _))` = the target does not resolve.
async fn resolve_hierarchy_scope<'a>(
    state: &ApiState,
    args: &'a serde_json::Value,
    target: HierarchyTarget,
) -> Result<(Option<String>, ClaimedAncestry<'a>), String> {
    let present = |key: &str| -> Result<Option<&'a str>, String> {
        match args.get(key).and_then(|v| v.as_str()) {
            None | Some("") => Ok(None),
            Some(id) if is_valid_ulid(id) => Ok(Some(id)),
            Some(_) => Err(format!("invalid {key}")),
        }
    };
    let verse_id = present("verse_id")?;
    let fractal_id = present("fractal_id")?;
    let petal_id = present("petal_id")?;

    Ok(match target {
        HierarchyTarget::Verse => {
            if fractal_id.is_some() {
                return Err("unexpected fractal_id: create_fractal targets a verse".to_string());
            }
            if petal_id.is_some() {
                return Err("unexpected petal_id: create_fractal targets a verse".to_string());
            }
            let verse_id = verse_id.ok_or("verse_id is required")?;
            let scope = crate::rest::resolve_verse_scope(state, verse_id).await;
            (scope, ClaimedAncestry::default())
        }
        HierarchyTarget::Fractal => {
            if petal_id.is_some() {
                return Err("unexpected petal_id: create_petal targets a fractal".to_string());
            }
            let fractal_id = fractal_id.ok_or("fractal_id is required")?;
            let scope = crate::rest::resolve_fractal_scope(state, fractal_id).await;
            let claimed = ClaimedAncestry {
                verse_id,
                fractal_id: None,
            };
            (scope, claimed)
        }
        HierarchyTarget::Petal => {
            let petal_id = petal_id.ok_or("petal_id is required")?;
            let scope = crate::rest::resolve_petal_scope(state, petal_id).await;
            (
                scope,
                ClaimedAncestry {
                    verse_id,
                    fractal_id,
                },
            )
        }
    })
}

/// Enforce the schema's `required` list (absent, `null`, or `""` = missing).
fn check_required_args(spec: &ToolSpec, args: &serde_json::Value) -> Result<(), String> {
    let schema = (spec.input_schema)();
    let missing: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| k.as_str())
        .filter(|k| match args.get(*k) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(s)) => s.is_empty(),
            Some(_) => false,
        })
        .collect();
    match missing.as_slice() {
        [] => Ok(()),
        [one] => Err(format!("{one} is required")),
        [init @ .., last] => Err(format!("{} and {last} are required", init.join(", "))),
    }
}

// ---------------------------------------------------------------------------
// Response helpers
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

#[cfg(test)]
mod tests {
    use super::*;

    /// FR-5 invariant: authz lives only in the dispatcher — no handler body
    /// calls a `require_*` guard or resolves its own scope.
    #[test]
    fn handlers_contain_no_authz_calls() {
        let handlers = include_str!("tools.rs");
        for needle in [
            "require_role",
            "require_scope",
            "require_role_and_scope",
            "require_role_level",
            "resolve_petal_scope",
            "resolve_node_scope",
            "resolve_fractal_scope",
            "resolve_verse_scope",
        ] {
            assert!(
                !handlers.contains(needle),
                "tools.rs must not call {needle}"
            );
        }
        let dispatcher = include_str!("mod.rs");
        let body = dispatcher
            .split("// The dispatcher:")
            .nth(1)
            .and_then(|rest| rest.split("// Response helpers").next())
            .expect("dispatcher section");
        assert_eq!(body.matches("require_role_level(").count(), 1);
        assert_eq!(body.matches("require_scope(").count(), 1);
    }

    /// Table integrity: unique names, a real role floor on every row (a
    /// `RoleLevel::None` floor would admit any token), descriptions name it,
    /// and every schema is an object schema.
    #[test]
    fn table_rows_are_well_formed() {
        let mut names = std::collections::HashSet::new();
        for spec in TOOLS {
            assert!(names.insert(spec.name), "duplicate tool {}", spec.name);
            assert_ne!(
                spec.min_role,
                RoleLevel::None,
                "{} has no role floor",
                spec.name
            );
            assert!(
                spec.description
                    .contains(&format!("{} role", spec.min_role)),
                "{} description must state its {} role",
                spec.name,
                spec.min_role
            );
            let schema = (spec.input_schema)();
            assert_eq!(schema["type"], "object", "{}", spec.name);
            if let ScopeRule::PetalArg(key) | ScopeRule::NodeArg(key) = spec.scope_rule {
                assert!(
                    schema["required"]
                        .as_array()
                        .is_some_and(|r| r.iter().any(|k| k == key)),
                    "{}: scope arg {key} must be required in its schema",
                    spec.name
                );
            }
        }
        assert_eq!(TOOLS.len(), 29, "DEC-C14: 24 non-sim + 5 sim");
    }

    #[test]
    fn required_args_message_lists_only_the_missing() {
        let spec = TOOLS.iter().find(|t| t.name == "create_node").unwrap();
        let err = check_required_args(spec, &serde_json::json!({ "petal_id": "x" })).unwrap_err();
        assert_eq!(err, "name is required");
        let spec = TOOLS.iter().find(|t| t.name == "promote_instance").unwrap();
        let err = check_required_args(spec, &serde_json::json!({ "petal_id": "p" })).unwrap_err();
        assert_eq!(err, "path_id and instance_index are required");
    }
}
