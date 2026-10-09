//! MCP tool handlers + input schemas. Every handler runs AFTER the
//! dispatcher's authz gate (`mod.rs::authorize`) and must not re-implement
//! it — the invariant is grep-tested. Thin wrappers over shared cores.

use serde_json::{json, Value};

use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};

use super::{ToolCall, ToolFuture, ToolOutcome};
use crate::endpoint::{caller_auth, load_node};
use crate::upload::{GlbPayload, MAX_ASSET_BYTES};

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// `args[key]` as a string (`""` when absent or not a string).
fn str_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `args[key]` as `[x, y, z]`; absent/`null` → `default`, malformed → error.
fn vec3_arg(args: &Value, key: &str, default: [f32; 3]) -> Result<[f32; 3], String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|_| format!("invalid {key}: expected [x, y, z] numbers")),
    }
}

/// `args[key]` as a finite number.
fn f64_arg(args: &Value, key: &str) -> Result<f64, String> {
    args.get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("invalid {key}: expected a number"))
}

/// The authorized scope's parts (the dispatcher resolved it from the DB).
fn scope_parts(call: &ToolCall<'_>) -> Result<fe_database::ScopeParts, String> {
    call.scope
        .and_then(|s| fe_database::parse_scope(s).ok())
        .ok_or_else(|| "scope unavailable".to_string())
}

/// The authorized petal id (from the resolved scope, never re-read from args).
fn scoped_petal(call: &ToolCall<'_>) -> Result<String, String> {
    scope_parts(call)?
        .petal_id
        .ok_or_else(|| "petal_id is required".to_string())
}

/// The DB-resolved scope string (present for every scoped rule).
fn scope_str<'a>(call: &ToolCall<'a>) -> Result<&'a str, String> {
    call.scope.ok_or_else(|| "scope unavailable".to_string())
}

/// Decode base64 off the async workers, capped before allocation.
async fn decode_base64(text: String, max_len: usize) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || crate::upload::decode_base64_capped(&text, max_len))
        .await
        .map_err(|_| "decode failed".to_string())?
        .map_err(|e| e.message())
}

/// Map a `DbResult::Error` to the generic client text, logging the detail.
fn db_failure(tool: &str, e: &str) -> String {
    tracing::warn!("{tool} MCP failed: {e}");
    "operation failed".to_string()
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

pub(super) fn get_hierarchy(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if call
            .state
            .api_cmd_tx
            .send(ApiCommand::GetHierarchy { reply_tx })
            .is_err()
        {
            tracing::warn!("GetHierarchy send failed (DB bridge gone)");
            return Err("hierarchy request failed".to_string());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
            Ok(Ok(data)) => {
                let dto = crate::types::hierarchy_to_dto(&data);
                // Self-filtering read (ScopeRule::None): narrow to the token scope.
                let filtered = crate::rest::filter_hierarchy_by_scope(dto, &call.claims.scope);
                Ok(serde_json::to_value(filtered).unwrap_or_default())
            }
            _ => Err("hierarchy request failed".to_string()),
        }
    })
}

pub(super) fn create_verse(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let cmd = DbCommand::CreateVerse {
            name: str_arg(call.args, "name").to_string(),
        };
        match crate::rest::db_round_trip(call.state, cmd).await? {
            DbResult::VerseCreated { id, name, .. } => Ok(json!({ "id": id, "name": name })),
            DbResult::Error(e) => Err(db_failure("create_verse", &e)),
            _ => Err("create_verse failed".to_string()),
        }
    })
}

pub(super) fn create_fractal(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let verse_id = scope_parts(&call)?.verse_id;
        let cmd = DbCommand::CreateFractal {
            verse_id,
            name: str_arg(call.args, "name").to_string(),
        };
        match crate::rest::db_round_trip(call.state, cmd).await? {
            DbResult::FractalCreated { id, name, .. } => Ok(json!({ "id": id, "name": name })),
            DbResult::Error(e) => Err(db_failure("create_fractal", &e)),
            _ => Err("create_fractal failed".to_string()),
        }
    })
}

pub(super) fn create_petal(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        // The write target is the DB-resolved fractal, never a raw arg.
        let fractal_id = scope_parts(&call)?
            .fractal_id
            .ok_or_else(|| "fractal_id is required".to_string())?;
        let cmd = DbCommand::CreatePetal {
            fractal_id,
            name: str_arg(call.args, "name").to_string(),
        };
        match crate::rest::db_round_trip(call.state, cmd).await? {
            DbResult::PetalCreated { id, name, .. } => Ok(json!({ "id": id, "name": name })),
            DbResult::Error(e) => Err(db_failure("create_petal", &e)),
            _ => Err("create_petal failed".to_string()),
        }
    })
}

pub(super) fn create_node(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let position = vec3_arg(call.args, "position", [0.0; 3])?;
        let cmd = DbCommand::CreateNode {
            petal_id,
            name: str_arg(call.args, "name").to_string(),
            position,
            correlation_id: None,
        };
        match crate::rest::db_round_trip(call.state, cmd).await? {
            DbResult::NodeCreated { id, name, .. } => Ok(json!({ "id": id, "name": name })),
            DbResult::Error(e) => Err(db_failure("create_node", &e)),
            _ => Err("create_node failed".to_string()),
        }
    })
}

pub(super) fn update_transform(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let node_id = str_arg(call.args, "node_id");
        let position = vec3_arg(call.args, "position", [0.0; 3])?;
        let rotation = vec3_arg(call.args, "rotation", [0.0; 3])?;
        let scale = vec3_arg(call.args, "scale", [1.0; 3])?;
        // Same core as REST PATCH .../transform (fire-and-forget persist).
        crate::rest::persist_transform(call.state, call.claims, node_id, position, rotation, scale);
        Ok(json!({ "status": "ok" }))
    })
}

// ---------------------------------------------------------------------------
// Per-endpoint CRUD (endpoint_api_surface_20260725 FR-4)
// ---------------------------------------------------------------------------

pub(super) fn read_node(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let dto = load_node(call.state, str_arg(call.args, "node_id"), scope_str(&call)?).await?;
        Ok(serde_json::to_value(dto).unwrap_or_default())
    })
}

pub(super) fn node_address(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let node_id = str_arg(call.args, "node_id");
        let scope = scope_str(&call)?;
        let addr = fe_entity_store::NodeAddress::from_scope_and_id(scope, node_id)
            .map_err(|e| format!("cannot address node: {e}"))?;
        Ok(json!({ "node_id": node_id, "address": addr.to_uri(), "scope": scope }))
    })
}

pub(super) fn delete_node(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let node_id = str_arg(call.args, "node_id").to_string();
        let cascade = call
            .args
            .get("cascade")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // fe-policy re-checks Editor+ at the node scope on the DB thread.
        let cmd = if cascade {
            DbCommand::CascadeTombstoneNode {
                node_id: node_id.clone(),
                auth: caller_auth(call.claims),
            }
        } else {
            DbCommand::TombstoneNode {
                node_id: node_id.clone(),
                auth: caller_auth(call.claims),
            }
        };
        match crate::rest::db_round_trip(call.state, cmd).await? {
            DbResult::NodeDeleted { .. } => {
                Ok(json!({ "node_id": node_id, "cascade": cascade, "tombstoned": true }))
            }
            DbResult::Error(e) => {
                tracing::warn!("delete_node MCP denied/failed: {e}");
                Err("operation failed or not permitted".to_string())
            }
            _ => Err("delete_node failed".to_string()),
        }
    })
}

pub(super) fn promote_instance(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let instance_index = call
            .args
            .get("instance_index")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "invalid instance_index".to_string())?;
        let cmd = DbCommand::PromoteInstance {
            petal_id,
            path_id: str_arg(call.args, "path_id").to_string(),
            instance_index,
            auth: caller_auth(call.claims),
        };
        match crate::rest::db_round_trip(call.state, cmd).await? {
            DbResult::NodePromoted {
                node_id,
                newly_promoted,
                ..
            } => Ok(json!({ "node_id": node_id, "newly_promoted": newly_promoted })),
            DbResult::Error(e) => {
                tracing::warn!("promote_instance MCP denied/failed: {e}");
                Err("operation failed or not permitted".to_string())
            }
            _ => Err("promote_instance failed".to_string()),
        }
    })
}

// ---------------------------------------------------------------------------
// M2/F7 distributed timeseries query (A15/A16/A17)
// ---------------------------------------------------------------------------

pub(super) fn query_timeseries(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        use fe_runtime::distributed_query::TsQueryKind;
        let petal_id = scoped_petal(&call)?;
        let metric = str_arg(call.args, "metric").to_string();
        let has_metric = !metric.is_empty();
        let window = match (
            call.args.get("start_ms").and_then(Value::as_i64),
            call.args.get("end_ms").and_then(Value::as_i64),
        ) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };
        // Each kind demands its own fields — a mismatch is a client error,
        // never a defaulted-away silent query.
        let spec = match (str_arg(call.args, "kind"), window) {
            ("window_aggregate", Some((start_ms, end_ms))) if has_metric => {
                TsQueryKind::WindowAggregate {
                    metric,
                    start_ms,
                    end_ms,
                    petal_id,
                }
            }
            ("readings_in_window", Some((start_ms, end_ms))) if has_metric => {
                TsQueryKind::ReadingsInWindow {
                    metric,
                    start_ms,
                    end_ms,
                    petal_id,
                }
            }
            ("latest_per_anchor", _) => TsQueryKind::LatestPerAnchor {
                petal_id,
                metric: has_metric.then_some(metric),
            },
            ("all_readings", _) => TsQueryKind::AllReadings { petal_id },
            _ => {
                return Err("invalid arguments: kind must be one of window_aggregate | latest_per_anchor | readings_in_window | all_readings, with the metric/start_ms/end_ms fields its kind requires".to_string());
            }
        };
        // Shared guarded fan-out (same bridge as /api/v1/query `distributed`).
        let outcome = crate::timeseries_query::run_distributed_timeseries_query(
            call.state,
            call.claims,
            spec,
        )
        .await
        .map_err(|e| e.message().to_string())?;
        if let Some(reason) = &outcome.error {
            return Err(reason.clone());
        }
        Ok(serde_json::to_value(&outcome).unwrap_or(Value::Null))
    })
}

// ---------------------------------------------------------------------------
// F9/A20 sim control: the SAME per-verb fns as /api/v1/sim/*
// ---------------------------------------------------------------------------

fn sim_outcome(result: Result<Value, crate::sim::SimApiError>) -> ToolOutcome {
    result.map_err(|e| e.message())
}

pub(super) fn sim_start(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let script = call.args.get("script").cloned().unwrap_or(Value::Null);
        sim_outcome(crate::sim::start(call.state, call.claims, script).await)
    })
}

pub(super) fn sim_stop(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move { sim_outcome(crate::sim::stop(call.state, call.claims).await) })
}

pub(super) fn sim_status(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move { sim_outcome(crate::sim::status(call.state, call.claims).await) })
}

pub(super) fn sim_step(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        // Absent → 1; present but not a u64 → 0 (rejected as a bad request).
        let n = match call.args.get("n") {
            None | Some(Value::Null) => 1,
            Some(v) => v.as_u64().unwrap_or(0),
        };
        sim_outcome(crate::sim::step(call.state, call.claims, n).await)
    })
}

pub(super) fn sim_inject_fault(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let event = call.args.get("event").cloned().unwrap_or(Value::Null);
        sim_outcome(crate::sim::inject_fault(call.state, call.claims, event).await)
    })
}

// ---------------------------------------------------------------------------
// F13: asset ingest + placement (FR-2 / FR-3)
// ---------------------------------------------------------------------------

pub(super) fn upload_asset(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let payload = GlbPayload::Base64(str_arg(call.args, "data_base64").to_string());
        let asset = crate::upload::ingest_glb(call.state, str_arg(call.args, "name"), payload)
            .await
            .map_err(|e| e.message())?;
        Ok(serde_json::to_value(asset).unwrap_or_default())
    })
}

pub(super) fn place_asset(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let asset_id = str_arg(call.args, "asset_id");
        if !crate::types::is_valid_ulid(asset_id) {
            return Err("invalid asset_id".to_string());
        }
        let placed = crate::upload::place_asset_core(
            call.state,
            &petal_id,
            str_arg(call.args, "name"),
            asset_id,
            vec3_arg(call.args, "position", [0.0; 3])?,
            vec3_arg(call.args, "rotation", [0.0; 3])?,
            vec3_arg(call.args, "scale", [1.0; 3])?,
        )
        .await?;
        Ok(serde_json::to_value(placed).unwrap_or_default())
    })
}

// ---------------------------------------------------------------------------
// F13: properties
// ---------------------------------------------------------------------------

pub(super) fn set_property(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let value = call.args.get("value").cloned().unwrap_or(Value::Null);
        let dto = crate::rest::set_property_core(
            call.state,
            str_arg(call.args, "node_id"),
            str_arg(call.args, "key").to_string(),
            value,
        )
        .await?;
        Ok(serde_json::to_value(dto).unwrap_or_default())
    })
}

pub(super) fn get_properties(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let dto =
            crate::rest::get_properties_core(call.state, str_arg(call.args, "node_id")).await?;
        Ok(serde_json::to_value(dto).unwrap_or_default())
    })
}

pub(super) fn delete_property(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let dto = crate::rest::delete_property_core(
            call.state,
            str_arg(call.args, "node_id"),
            str_arg(call.args, "key"),
        )
        .await?;
        Ok(serde_json::to_value(dto).unwrap_or_default())
    })
}

// ---------------------------------------------------------------------------
// F13: waypoints + GPX
// ---------------------------------------------------------------------------

pub(super) fn create_waypoint(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let req = crate::types::CreateWaypointRequest {
            name: str_arg(call.args, "name").to_string(),
            lat: f64_arg(call.args, "lat")?,
            lon: f64_arg(call.args, "lon")?,
            ele: call.args.get("ele").and_then(Value::as_f64).unwrap_or(0.0),
            description: call
                .args
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            symbol: call
                .args
                .get("symbol")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        let dto = crate::rest::create_waypoint_core(call.state, &petal_id, &req).await?;
        Ok(json!({ "id": dto.id, "name": dto.name }))
    })
}

pub(super) fn move_waypoint(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let req = crate::types::MoveWaypointRequest {
            lat: f64_arg(call.args, "lat")?,
            lon: f64_arg(call.args, "lon")?,
            ele: call.args.get("ele").and_then(Value::as_f64),
        };
        crate::rest::move_waypoint_core(
            call.state,
            str_arg(call.args, "node_id"),
            scope_str(&call)?,
            &req,
        )
        .await
    })
}

pub(super) fn import_gpx(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let text = str_arg(call.args, "data_base64").to_string();
        let bytes = decode_base64(text, crate::limits::MCP_GPX_MAX_BYTES).await?;
        crate::gpx::import_gpx_core(call.state, &petal_id, &bytes).await
    })
}

// ---------------------------------------------------------------------------
// F13: terrain + tilesets + GIS
// ---------------------------------------------------------------------------

pub(super) fn set_petal_terrain(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let Some(terrain) = call.args.get("terrain") else {
            return Err("terrain is required (an object, or null to clear)".to_string());
        };
        crate::terrain::set_petal_terrain_core(terrain)?;
        Ok(Value::Null)
    })
}

pub(super) fn list_tilesets(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        crate::terrain::list_tilesets_core(call.state, call.claims, &petal_id)
            .await
            .map_err(|status| match status.as_u16() {
                403 => "insufficient permissions".to_string(),
                404 => "petal not found".to_string(),
                _ => "tileset lookup failed".to_string(),
            })
    })
}

pub(super) fn install_tileset(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let text = str_arg(call.args, "data_base64").to_string();
        let bytes = decode_base64(text, MAX_ASSET_BYTES).await?;
        crate::terrain::install_tileset_core(call.state, call.claims, &petal_id, &bytes).await
    })
}

pub(super) fn get_gis_nodes(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let petal_id = scoped_petal(&call)?;
        let filter: crate::gis::GisNodesQuery = serde_json::from_value(call.args.clone())
            .map_err(|e| format!("invalid spatial filter: {e}"))?;
        crate::gis::gis_nodes_core(call.state, &petal_id, &filter)
            .await
            .map_err(|(_, msg)| msg)
    })
}

pub(super) fn query(call: ToolCall<'_>) -> ToolFuture<'_> {
    Box::pin(async move {
        let vars: std::collections::HashMap<String, Value> = match call.args.get("vars") {
            None | Some(Value::Null) => Default::default(),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|_| "vars must be an object".to_string())?,
        };
        // Self-filtering (ScopeRule::None): query_guard injects the token scope.
        let dto =
            crate::rest::run_local_query(call.state, call.claims, str_arg(call.args, "sql"), &vars)
                .await?;
        Ok(serde_json::to_value(dto).unwrap_or_default())
    })
}

// ---------------------------------------------------------------------------
// Input schemas (one fn per tool, named like the tool)
// ---------------------------------------------------------------------------

pub(super) mod schema {
    use serde_json::{json, Value};

    fn vec3(desc: &str) -> Value {
        json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": desc })
    }

    const POSITION: &str = "[x, y, z] in world units (meters × the petal map's world_scale; 1 world unit = 1 m when world_scale is 1)";
    const ROTATION: &str =
        "[x, y, z] Euler angles in radians, XYZ order (the in-app inspector displays degrees)";
    const SCALE: &str = "[x, y, z] unitless scale multipliers (1.0 = authored size)";

    pub fn get_hierarchy() -> Value {
        json!({ "type": "object", "properties": {} })
    }

    pub fn create_verse() -> Value {
        json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] })
    }

    pub fn create_fractal() -> Value {
        json!({
            "type": "object",
            "properties": { "verse_id": { "type": "string" }, "name": { "type": "string" } },
            "required": ["verse_id", "name"]
        })
    }

    pub fn create_petal() -> Value {
        json!({
            "type": "object",
            "properties": {
                "fractal_id": { "type": "string", "description": "Parent fractal (its stored verse is the authz anchor)" },
                "verse_id": { "type": "string", "description": "Optional; must match the fractal's stored verse" },
                "name": { "type": "string" }
            },
            "required": ["fractal_id", "name"]
        })
    }

    pub fn create_node() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string", "description": "Parent petal (its stored hierarchy is the authz anchor)" },
                "verse_id": { "type": "string", "description": "Optional; must match the petal's stored verse" },
                "fractal_id": { "type": "string", "description": "Optional; must match the petal's stored fractal" },
                "name": { "type": "string" },
                "position": vec3(POSITION)
            },
            "required": ["petal_id", "name"]
        })
    }

    pub fn update_transform() -> Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "string" },
                "position": vec3(POSITION),
                "rotation": vec3(ROTATION),
                "scale": vec3(SCALE)
            },
            "required": ["node_id", "position", "rotation", "scale"]
        })
    }

    pub fn read_node() -> Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "string", "description": "Node ULID or promoted-stamp instance id (<ulid>#inst-<n>)" }
            },
            "required": ["node_id"]
        })
    }

    pub fn node_address() -> Value {
        json!({ "type": "object", "properties": { "node_id": { "type": "string" } }, "required": ["node_id"] })
    }

    pub fn delete_node() -> Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "string" },
                "cascade": { "type": "boolean", "description": "Tombstone the node's whole descendant subtree in one atomic op" }
            },
            "required": ["node_id"]
        })
    }

    pub fn promote_instance() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string" },
                "path_id": { "type": "string", "description": "Owning path node id whose curve the stamp follows" },
                "instance_index": { "type": "integer", "minimum": 0, "description": "Zero-based instance index within the stamp group" }
            },
            "required": ["petal_id", "path_id", "instance_index"]
        })
    }

    pub fn query_timeseries() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string", "description": "The petal whose shard fabric is queried" },
                "kind": {
                    "type": "string",
                    "enum": ["window_aggregate", "latest_per_anchor", "readings_in_window", "all_readings"],
                    "description": "Query shape: window_aggregate = per-anchor mean/min/max/count over a half-open [start_ms, end_ms) window; latest_per_anchor = newest reading per (anchor, metric); readings_in_window = raw rows in the window; all_readings = the whole merged view of the petal"
                },
                "metric": { "type": "string", "description": "Metric name (required for window_aggregate and readings_in_window; optional filter for latest_per_anchor)" },
                "start_ms": { "type": "integer", "description": "Window start, epoch ms inclusive (required for window_aggregate and readings_in_window)" },
                "end_ms": { "type": "integer", "description": "Window end, epoch ms exclusive (required for window_aggregate and readings_in_window)" }
            },
            "required": ["petal_id", "kind"]
        })
    }

    pub fn sim_start() -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": { "description": "A ScenarioScript object, its JSON text, or a built-in name: default | sharded_query | offline_degraded" }
            },
            "required": ["script"]
        })
    }

    pub fn sim_stop() -> Value {
        json!({ "type": "object", "properties": {} })
    }

    pub fn sim_status() -> Value {
        json!({ "type": "object", "properties": {} })
    }

    pub fn sim_step() -> Value {
        json!({
            "type": "object",
            "properties": {
                "n": { "type": "integer", "minimum": 1, "maximum": fe_runtime::sim_control::MAX_SIM_STEP, "description": "Actions to execute (default 1)" }
            }
        })
    }

    pub fn sim_inject_fault() -> Value {
        json!({
            "type": "object",
            "properties": {
                "event": { "type": "object", "description": "A ScriptedEvent, e.g. {\"kind\":\"peer_offline\",\"peer\":\"alice\"} (at_ms is ignored)" }
            },
            "required": ["event"]
        })
    }

    pub fn upload_asset() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string", "description": "Authz anchor petal (asset rows themselves are node-global)" },
                "name": { "type": "string", "description": "Display/file name for the asset" },
                "data_base64": { "type": "string", "description": "Standard base64 of a GLB (binary glTF 2.0) file" }
            },
            "required": ["petal_id", "name", "data_base64"]
        })
    }

    pub fn place_asset() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string" },
                "asset_id": { "type": "string", "description": "From upload_asset" },
                "name": { "type": "string" },
                "position": vec3(POSITION),
                "rotation": vec3(ROTATION),
                "scale": vec3(SCALE)
            },
            "required": ["petal_id", "asset_id", "name"]
        })
    }

    pub fn set_property() -> Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "string" },
                "key": { "type": "string" },
                "value": { "description": "Any non-null JSON value (use delete_property to remove)" }
            },
            "required": ["node_id", "key", "value"]
        })
    }

    pub fn get_properties() -> Value {
        json!({ "type": "object", "properties": { "node_id": { "type": "string" } }, "required": ["node_id"] })
    }

    pub fn delete_property() -> Value {
        json!({
            "type": "object",
            "properties": { "node_id": { "type": "string" }, "key": { "type": "string" } },
            "required": ["node_id", "key"]
        })
    }

    pub fn create_waypoint() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string" },
                "name": { "type": "string" },
                "lat": { "type": "number", "description": "WGS84 latitude, degrees" },
                "lon": { "type": "number", "description": "WGS84 longitude, degrees" },
                "ele": { "type": "number", "description": "Elevation, meters (default 0)" },
                "description": { "type": "string" },
                "symbol": { "type": "string" }
            },
            "required": ["petal_id", "name", "lat", "lon"]
        })
    }

    pub fn move_waypoint() -> Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "string", "description": "The waypoint node" },
                "lat": { "type": "number" },
                "lon": { "type": "number" },
                "ele": { "type": "number", "description": "Meters (default 0)" }
            },
            "required": ["node_id", "lat", "lon"]
        })
    }

    pub fn import_gpx() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string" },
                "data_base64": { "type": "string", "description": "Standard base64 of a GPX 1.1 document (max 16 MiB decoded)" }
            },
            "required": ["petal_id", "data_base64"]
        })
    }

    pub fn set_petal_terrain() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string" },
                "terrain": { "type": ["object", "null"], "description": "A TerrainConfig object, or null to clear" }
            },
            "required": ["petal_id"]
        })
    }

    pub fn list_tilesets() -> Value {
        json!({ "type": "object", "properties": { "petal_id": { "type": "string" } }, "required": ["petal_id"] })
    }

    pub fn install_tileset() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string", "description": "The petal whose terrain already binds this tileset id" },
                "data_base64": { "type": "string", "description": "Standard base64 of the .hexon terrain-tileset archive" }
            },
            "required": ["petal_id", "data_base64"]
        })
    }

    pub fn get_gis_nodes() -> Value {
        json!({
            "type": "object",
            "properties": {
                "petal_id": { "type": "string" },
                "bbox": { "type": "string", "description": "Local-meter AABB: minx,minz,maxx,maxz" },
                "bbox_ll": { "type": "string", "description": "Lat/lon AABB: minLat,minLon,maxLat,maxLon (needs a petal terrain origin)" },
                "radius": { "type": "number", "description": "Radius in local meters (with cx + cz)" },
                "cx": { "type": "number" },
                "cz": { "type": "number" }
            },
            "required": ["petal_id"]
        })
    }

    pub fn query() -> Value {
        json!({
            "type": "object",
            "properties": {
                "sql": { "type": "string", "description": "One read-only SurrealQL SELECT; the token scope filter is injected" },
                "vars": { "type": "object", "description": "Bind variables" }
            },
            "required": ["sql"]
        })
    }
}
