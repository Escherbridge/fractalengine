use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, Path, State};
use axum::response::IntoResponse;
use axum::Extension;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, TransformUpdate};

use crate::auth::{require_role, require_role_and_scope, require_scope};
use crate::types::{
    ApiResponse, CreatedEntityDto, CreateFractalRequest, CreateNodeRequest,
    CreatePetalRequest, CreateVerseRequest, UpdateTransformRequest, VerseDto,
    hierarchy_to_dto, is_valid_ulid,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/hierarchy — full verse → fractal → petal → node snapshot.
///
/// Scope enforcement: returns only verses the token has access to.
/// A token scoped to `VERSE#v1` sees only that verse. An unscoped or
/// broad-scoped token sees all.
pub async fn get_hierarchy(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<Vec<VerseDto>>::error("insufficient permissions"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::GetHierarchy { reply_tx };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<Vec<VerseDto>>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(data)) => {
            let dto = hierarchy_to_dto(&data);
            // Filter hierarchy by token scope
            let filtered = filter_hierarchy_by_scope(dto, &claims.scope);
            Json(ApiResponse::success(filtered))
        }
        Ok(Err(_)) => Json(ApiResponse::<Vec<VerseDto>>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<Vec<VerseDto>>::error("request timed out")),
    }
}

/// POST /api/v1/verses — create a new verse.
pub async fn create_verse(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<CreateVerseRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "manager") {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient permissions"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateVerse { name: req.name },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::VerseCreated { id, name })) => {
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(_e))) => {
            tracing::error!("create_verse failed: {_e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// POST /api/v1/verses/:verse_id/fractals — create a fractal in a verse.
pub async fn create_fractal(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
    Json(req): Json<CreateFractalRequest>,
) -> impl IntoResponse {
    let scope = fe_database::build_scope(&verse_id, None, None);
    if let Err(_) = require_role_and_scope(&claims, "editor", &scope) {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient permissions or scope"));
    }

    if !is_valid_ulid(&verse_id) {
        return Json(ApiResponse::<CreatedEntityDto>::error("invalid verse_id"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateFractal {
            verse_id: verse_id.clone(),
            name: req.name,
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::FractalCreated { id, name, .. })) => {
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(_e))) => {
            tracing::error!("create_fractal failed: {_e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// POST /api/v1/verses/:verse_id/fractals/:fractal_id/petals — create a petal.
pub async fn create_petal(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((verse_id, fractal_id)): Path<(String, String)>,
    Json(req): Json<CreatePetalRequest>,
) -> impl IntoResponse {
    let scope = fe_database::build_scope(&verse_id, Some(&fractal_id), None);
    if let Err(_) = require_role_and_scope(&claims, "editor", &scope) {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient permissions or scope"));
    }

    if !is_valid_ulid(&verse_id) || !is_valid_ulid(&fractal_id) {
        return Json(ApiResponse::<CreatedEntityDto>::error("invalid verse_id or fractal_id"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreatePetal {
            fractal_id: fractal_id.clone(),
            name: req.name,
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::PetalCreated { id, name, .. })) => {
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(_e))) => {
            tracing::error!("create_petal failed: {_e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// POST /api/v1/verses/:vid/fractals/:fid/petals/:pid/nodes — create a node.
///
/// Hierarchical path: the verse/fractal/petal IDs are in the URL, so the
/// token's scope is validated against the full hierarchy path.
pub async fn create_node(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((verse_id, fractal_id, petal_id)): Path<(String, String, String)>,
    Json(req): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    let scope = fe_database::build_scope(&verse_id, Some(&fractal_id), Some(&petal_id));
    if let Err(_) = require_role_and_scope(&claims, "editor", &scope) {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient permissions or scope"));
    }

    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<CreatedEntityDto>::error("invalid petal_id"));
    }

    let position = req.position.unwrap_or([0.0, 0.0, 0.0]);
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateNode {
            petal_id,
            name: req.name,
            position,
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodeCreated { id, name, .. })) => {
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(_e))) => {
            tracing::error!("create_node failed: {_e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// POST /api/v1/nodes — legacy flat create (for MCP and existing integrations).
///
/// DEPRECATION NOTE: Prefer the hierarchical endpoint
/// `POST /api/v1/verses/:vid/fractals/:fid/petals/:pid/nodes` which enforces
/// scope from the URL path. This endpoint resolves the petal's full scope via
/// a DB query and enforces it against the token scope before proceeding.
pub async fn create_node_legacy(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "editor") {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient permissions"));
    }

    let petal_id = req.petal_id.clone().unwrap_or_default();
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<CreatedEntityDto>::error("invalid petal_id"));
    }

    // Resolve petal scope for enforcement
    if let Some(scope) = resolve_petal_scope(&state, &petal_id).await {
        if let Err(_) = require_scope(&claims, &scope) {
            return Json(ApiResponse::<CreatedEntityDto>::error("insufficient scope"));
        }
    }

    let position = req.position.unwrap_or([0.0, 0.0, 0.0]);
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateNode {
            petal_id,
            name: req.name,
            position,
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodeCreated { id, name, .. })) => {
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(_e))) => {
            tracing::error!("create_node failed: {_e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// PATCH /api/v1/nodes/:node_id/transform — update position/rotation/scale.
///
/// Persists via DB and also broadcasts on the real-time transform channel so
/// WebSocket subscribers receive the update without a DB round-trip.
pub async fn update_transform(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    Json(req): Json<UpdateTransformRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "editor") {
        return Json(ApiResponse::error("insufficient permissions"));
    }

    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::error("invalid node_id"));
    }

    // Resolve node scope for enforcement
    if let Some(scope) = resolve_node_scope(&state, &node_id).await {
        if let Err(_) = require_scope(&claims, &scope) {
            return Json(ApiResponse::error("insufficient scope"));
        }
    }

    // Optimistic broadcast: push transform to WS subscribers + Bevy bridge
    // immediately, BEFORE the DB persist completes.
    let broadcast_receivers = state.transform_broadcast_tx.send(TransformUpdate {
        node_id: node_id.clone(),
        petal_id: String::new(),
        position: req.position,
        rotation: req.rotation,
        scale: req.scale,
        timestamp_ms: now_ms(),
        source_did: claims.sub,
    }).unwrap_or(0);

    // Fire-and-forget DB persist via TransformPersist (bypasses PendingApiRequests).
    // On failure the DB thread emits SceneChange::TransformFailed so WS subscribers
    // receive a rollback with the last-known-good values.
    let _ = state.api_cmd_tx.send(ApiCommand::TransformPersist {
        node_id: node_id.clone(),
        position: req.position,
        rotation: req.rotation,
        scale: req.scale,
    });

    Json(ApiResponse::success(serde_json::json!({
        "node_id": node_id,
        "broadcast_receivers": broadcast_receivers,
        "persist": "queued"
    })))
}

/// GET /api/v1/nodes/:node_id/transform -- read current transform.
///
/// Queries the node's persisted position, rotation, and scale directly from the
/// database via `DbCommand::GetNodeTransform`.
pub async fn get_transform(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    use crate::types::TransformDto;

    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<TransformDto>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<TransformDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement
    if let Some(scope) = resolve_node_scope(&state, &node_id).await {
        if let Err(_) = require_scope(&claims, &scope) {
            return Json(ApiResponse::<TransformDto>::error("insufficient scope"));
        }
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::GetNodeTransform { node_id: node_id.clone() },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<TransformDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodeTransformLoaded { position, rotation, scale, .. })) => {
            Json(ApiResponse::success(TransformDto { position, rotation, scale }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<TransformDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<TransformDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<TransformDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<TransformDto>::error("request timed out")),
    }
}

// ---------------------------------------------------------------------------
// Scope resolution helpers
// ---------------------------------------------------------------------------

/// Resolve a petal_id to its full scope string via DB query.
/// Returns None if resolution fails (petal not found, timeout, etc.)
async fn resolve_petal_scope(state: &crate::server::ApiState, petal_id: &str) -> Option<String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state.api_cmd_tx.send(ApiCommand::DbRequest {
        cmd: DbCommand::ResolvePetalScope { petal_id: petal_id.to_string() },
        reply_tx,
    }).ok()?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

/// Resolve a node_id to its full scope string via DB query.
/// Returns None if resolution fails (node not found, timeout, etc.)
async fn resolve_node_scope(state: &crate::server::ApiState, node_id: &str) -> Option<String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state.api_cmd_tx.send(ApiCommand::DbRequest {
        cmd: DbCommand::ResolveNodeScope { node_id: node_id.to_string() },
        reply_tx,
    }).ok()?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Hierarchy scope filtering
// ---------------------------------------------------------------------------

/// Filter the full hierarchy DTO to only include verses (and their sub-tree)
/// that the token's scope covers.
pub fn filter_hierarchy_by_scope(verses: Vec<VerseDto>, token_scope: &str) -> Vec<VerseDto> {
    // Parse token scope to see what level it covers
    let Ok(parts) = fe_database::parse_scope(token_scope) else {
        // If scope can't be parsed (shouldn't happen for valid tokens), return empty
        return Vec::new();
    };

    verses
        .into_iter()
        .filter_map(|v| {
            // Token must be scoped to this verse
            if v.id != parts.verse_id {
                return None;
            }

            // If token is verse-level, return the full verse tree
            let Some(ref fid) = parts.fractal_id else {
                return Some(v);
            };

            // Token is fractal-scoped: filter to only that fractal
            let fractals: Vec<_> = v.fractals.into_iter().filter_map(|f| {
                if f.id != *fid {
                    return None;
                }

                let Some(ref pid) = parts.petal_id else {
                    return Some(f);
                };

                // Token is petal-scoped: filter to only that petal
                let petals: Vec<_> = f.petals.into_iter().filter(|p| p.id == *pid).collect();
                if petals.is_empty() {
                    return None;
                }
                Some(crate::types::FractalDto {
                    id: f.id,
                    name: f.name,
                    petals,
                })
            }).collect();

            if fractals.is_empty() {
                return None;
            }
            Some(VerseDto {
                id: v.id,
                name: v.name,
                fractals,
            })
        })
        .collect()
}
