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
    CreatePetalRequest, CreateVerseRequest, PropertiesDto, PropertySetDto,
    SetPropertyRequest, UpdateTransformRequest, VerseDto,
    CreateFieldDefRequest, UpdateFieldDefRequest, FieldDefDto,
    hierarchy_to_dto, is_valid_ulid, is_valid_scope,
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
    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<CreatedEntityDto>::error("could not resolve petal scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient scope"));
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
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::error("insufficient scope"));
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

    // Fire-and-forget tracking integration: if this node has a `tracking_route_id`
    // property, compute snap-to-route metrics and check deviation alerts.
    if let Some(ref db) = state.db_reader {
        let db = db.clone();
        let api_cmd_tx = state.api_cmd_tx.clone();
        let tracked_node_id = node_id.clone();
        let pos = req.position;
        tokio::spawn(async move {
            let _ = check_tracking_and_alert(&db, &api_cmd_tx, &tracked_node_id, pos).await;
        });
    }

    Json(ApiResponse::success(serde_json::json!({
        "node_id": node_id,
        "broadcast_receivers": broadcast_receivers,
        "persist": "queued"
    })))
}

/// GET /api/v1/nodes/:node_id/transform -- read current transform.
///
/// When a direct `db_reader` is available, queries SurrealDB directly and
/// bypasses the crossbeam channel. Falls back to `DbCommand::GetNodeTransform`
/// otherwise.
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
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<TransformDto>::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<TransformDto>::error("insufficient scope"));
    }

    if let Some(ref db) = state.db_reader {
        // Direct query — bypass crossbeam channel
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            direct_get_node_transform(db, &node_id),
        )
        .await
        {
            Ok(Ok(Some(transform))) => Json(ApiResponse::success(transform)),
            Ok(Ok(None)) => Json(ApiResponse::<TransformDto>::error("node not found")),
            Ok(Err(e)) => Json(ApiResponse::<TransformDto>::error(format!("query failed: {e}"))),
            Err(_) => Json(ApiResponse::<TransformDto>::error("request timed out")),
        }
    } else {
        // Fallback: channel-based query
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
}

/// PATCH /api/v1/nodes/:node_id/properties — set a custom property.
///
/// RBAC: Editor+ required. Scope resolved from node's parent chain.
pub async fn set_node_property(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
    Json(req): Json<SetPropertyRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "editor") {
        return Json(ApiResponse::<PropertySetDto>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<PropertySetDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement — deny if resolution fails
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<PropertySetDto>::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PropertySetDto>::error("insufficient scope"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::SetNodeProperty {
            node_id: node_id.clone(),
            key: req.key,
            value: req.value,
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<PropertySetDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodePropertySet { node_id, key })) => {
            Json(ApiResponse::success(PropertySetDto { node_id, key }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<PropertySetDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<PropertySetDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<PropertySetDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<PropertySetDto>::error("request timed out")),
    }
}

/// GET /api/v1/nodes/:node_id/properties — read all custom properties.
///
/// RBAC: Viewer+ required. Scope resolved from node's parent chain.
pub async fn get_node_properties(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<PropertiesDto>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<PropertiesDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement — deny if resolution fails
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<PropertiesDto>::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PropertiesDto>::error("insufficient scope"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::GetNodeProperties { node_id: node_id.clone() },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<PropertiesDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodePropertiesLoaded { node_id, properties })) => {
            Json(ApiResponse::success(PropertiesDto { node_id, properties }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<PropertiesDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<PropertiesDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<PropertiesDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<PropertiesDto>::error("request timed out")),
    }
}

/// DELETE /api/v1/nodes/:node_id/properties/:key — delete a custom property.
///
/// RBAC: Editor+ required. Scope resolved from node's parent chain.
pub async fn delete_node_property(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((node_id, key)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "editor") {
        return Json(ApiResponse::<PropertySetDto>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<PropertySetDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement — deny if resolution fails
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<PropertySetDto>::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PropertySetDto>::error("insufficient scope"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::DeleteNodeProperty {
            node_id: node_id.clone(),
            key: key.clone(),
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<PropertySetDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodePropertyDeleted { .. })) => {
            Json(ApiResponse::success(PropertySetDto { node_id, key }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<PropertySetDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<PropertySetDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<PropertySetDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<PropertySetDto>::error("request timed out")),
    }
}

// ---------------------------------------------------------------------------
// Scope resolution helpers
// ---------------------------------------------------------------------------

/// Resolve a petal_id to its full scope string.
///
/// Uses a direct DB query when `db_reader` is available, falling back to the
/// crossbeam channel otherwise.
async fn resolve_petal_scope(state: &crate::server::ApiState, petal_id: &str) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return direct_resolve_petal_scope(db, petal_id).await;
    }
    // Fallback: channel-based resolution
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

/// Resolve a node_id to its full scope string.
///
/// Uses a direct DB query when `db_reader` is available, falling back to the
/// crossbeam channel otherwise. `pub(crate)` so other REST modules (e.g.
/// `assets`) can reuse the same RBAC scope resolution for node-scoped routes.
pub(crate) async fn resolve_node_scope(state: &crate::server::ApiState, node_id: &str) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return direct_resolve_node_scope(db, node_id).await;
    }
    // Fallback: channel-based resolution
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

// ---------------------------------------------------------------------------
// Direct DB read helpers (bypass crossbeam channel)
// ---------------------------------------------------------------------------

/// Type alias for the SurrealDB connection used by direct read helpers.
type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

/// Direct DB query for a node's transform. Returns `None` if the node does not
/// exist. Bypasses the crossbeam→DB-thread round-trip.
pub(crate) async fn direct_get_node_transform(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<crate::types::TransformDto>> {
    let mut res = db
        .query("SELECT position, elevation, rotation, scale FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("direct query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("take failed: {e}"))?;

    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let coords = &row["position"]["coordinates"];
    let x = coords[0].as_f64().unwrap_or(0.0) as f32;
    let z = coords[1].as_f64().unwrap_or(0.0) as f32;
    let y = row["elevation"].as_f64().unwrap_or(0.0) as f32;

    let rotation = parse_f32_array3(&row["rotation"], 0.0);
    let scale = parse_f32_array3(&row["scale"], 1.0);

    Ok(Some(crate::types::TransformDto {
        position: [x, y, z],
        rotation,
        scale,
    }))
}

/// Resolve a petal's full scope string via direct DB queries.
pub(crate) async fn direct_resolve_petal_scope(db: &Db, petal_id: &str) -> Option<String> {
    let mut res = db
        .query("SELECT fractal_id FROM petal WHERE petal_id = $pid LIMIT 1")
        .bind(("pid", petal_id.to_string()))
        .await
        .ok()?;
    let rows: Vec<serde_json::Value> = res.take(0).ok()?;
    let fractal_id = rows.first()?.get("fractal_id")?.as_str()?.to_string();

    let mut res2 = db
        .query("SELECT verse_id FROM fractal WHERE fractal_id = $fid LIMIT 1")
        .bind(("fid", fractal_id.clone()))
        .await
        .ok()?;
    let rows2: Vec<serde_json::Value> = res2.take(0).ok()?;
    let verse_id = rows2.first()?.get("verse_id")?.as_str()?;

    Some(fe_database::build_scope(verse_id, Some(&fractal_id), Some(petal_id)))
}

/// Resolve a node's full scope string via direct DB queries.
pub(crate) async fn direct_resolve_node_scope(db: &Db, node_id: &str) -> Option<String> {
    let mut res = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .ok()?;
    let rows: Vec<serde_json::Value> = res.take(0).ok()?;
    let petal_id = rows.first()?.get("petal_id")?.as_str()?;
    direct_resolve_petal_scope(db, petal_id).await
}

/// Load all nodes for a petal via direct DB query.
pub(crate) async fn direct_load_petal_nodes(
    db: &Db,
    petal_id: &str,
) -> Vec<crate::types::NodeDto> {
    let query_result = db
        .query("SELECT * FROM node WHERE petal_id = $pid ORDER BY created_at ASC")
        .bind(("pid", petal_id.to_string()))
        .await;

    let mut res = match query_result {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let nodes_raw: Vec<serde_json::Value> = match res.take(0) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    nodes_raw
        .iter()
        .map(|n| {
            let coords = &n["position"]["coordinates"];
            let x = coords[0].as_f64().unwrap_or(0.0) as f32;
            let z = coords[1].as_f64().unwrap_or(0.0) as f32;
            let y = n["elevation"].as_f64().unwrap_or(0.0) as f32;

            crate::types::NodeDto {
                id: n["node_id"].as_str().unwrap_or("").to_string(),
                name: n["name"].as_str().unwrap_or("").to_string(),
                petal_id: n["petal_id"].as_str().unwrap_or("").to_string(),
                position: [x, y, z],
                has_asset: n["asset_id"].as_str().is_some(),
                asset_path: n["asset_path"].as_str().map(|s| s.to_string()),
                webpage_url: None,
            }
        })
        .collect()
}

/// Parse a JSON value as a `[f32; 3]` array, using `default` for missing elements.
fn parse_f32_array3(val: &serde_json::Value, default: f32) -> [f32; 3] {
    if let Some(arr) = val.as_array() {
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(default as f64) as f32,
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(default as f64) as f32,
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(default as f64) as f32,
        ]
    } else {
        [default, default, default]
    }
}

// ---------------------------------------------------------------------------
// Query endpoint
// ---------------------------------------------------------------------------

/// POST /api/v1/query — execute a read-only SurrealQL query with scope guards.
///
/// Security: Only SELECT statements are allowed. Scope injection ensures the
/// caller can only see data within their token's scope. Rate-limited to 10 req/s.
pub async fn execute_query(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<crate::types::QueryRequest>,
) -> impl IntoResponse {
    use crate::types::QueryResultDto;

    println!("DEBUG: Inside execute_query - checking role");
    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<QueryResultDto>::error("insufficient permissions"));
    }

    println!("DEBUG: Inside execute_query - acquiring rate limiter lock");
    // Rate limiting: 10 queries/sec per user (keyed by sub/DID, not jti, so
    // creating multiple tokens doesn't bypass the limit).
    {
        let now = std::time::Instant::now();
        let mut limiter = state.query_rate_limiter.lock().await;

        // Evict stale entries older than 10s to prevent unbounded growth.
        limiter.retain(|_, (_, ts)| now.duration_since(*ts) < std::time::Duration::from_secs(10));

        let entry = limiter.entry(claims.sub.clone()).or_insert((0u32, now));
        if now.duration_since(entry.1) > std::time::Duration::from_secs(1) {
            // Reset window
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > 10 {
                return Json(ApiResponse::<QueryResultDto>::error("rate limit exceeded (10 queries/sec)"));
            }
        }
    }

    // Security: only allow single SELECT statements.
    let sql_trimmed = req.sql.trim();

    // Reject semicolons — prevents multi-statement chaining.
    if sql_trimmed.contains(';') {
        return Json(ApiResponse::<QueryResultDto>::error(
            "semicolons are not allowed (single statement only)",
        ));
    }

    let sql_upper = sql_trimmed.to_uppercase();
    if !sql_upper.starts_with("SELECT") {
        return Json(ApiResponse::<QueryResultDto>::error(
            "only SELECT statements are allowed",
        ));
    }

    // Reject any dangerous keyword anywhere in the statement (including subqueries,
    // comments, RETURN wrappers, LET bindings, etc.).
    const BLOCKED_KEYWORDS: &[&str] = &[
        "CREATE", "UPDATE", "DELETE", "DEFINE", "REMOVE", "RELATE", "INSERT",
        "LET", "RETURN", "INFO", "FOR", "THROW", "SLEEP", "BREAK", "LIVE", "KILL",
        "IF", "BEGIN", "COMMIT", "CANCEL",
    ];
    for keyword in BLOCKED_KEYWORDS {
        // Match whole-word-ish: the keyword must appear as an uppercase token,
        // not as a substring of an identifier (e.g. "DELETED" should not match "DELETE").
        // We check that the character before and after the keyword (if any) is not alphanumeric.
        let mut start = 0;
        while let Some(pos) = sql_upper[start..].find(keyword) {
            let abs_pos = start + pos;
            let before_ok = abs_pos == 0
                || !sql_upper.as_bytes()[abs_pos - 1].is_ascii_alphanumeric()
                    && sql_upper.as_bytes()[abs_pos - 1] != b'_';
            let after_pos = abs_pos + keyword.len();
            let after_ok = after_pos >= sql_upper.len()
                || !sql_upper.as_bytes()[after_pos].is_ascii_alphanumeric()
                    && sql_upper.as_bytes()[after_pos] != b'_';
            if before_ok && after_ok {
                return Json(ApiResponse::<QueryResultDto>::error(format!(
                    "{keyword} keyword is not allowed in queries"
                )));
            }
            start = abs_pos + keyword.len();
        }
    }

    // Table whitelist: only allow queries against these tables.
    const ALLOWED_TABLES: &[&str] = &[
        "NODE", "VERSE", "FRACTAL", "PETAL", "NODE_LOG", "FIELD_DEF",
        "ASSET", "MODEL", "ROLE", "ROOM", "CRATE_REGISTRY", "CRATE_ENTRY", "VERSE_MEMBER",
    ];
    if let Some(from_pos) = sql_upper.find("FROM") {
        let after_from = &sql_upper[from_pos + 4..].trim_start();
        let table_name: String = after_from
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !ALLOWED_TABLES.contains(&table_name.as_str()) {
            return Json(ApiResponse::<QueryResultDto>::error(format!(
                "queries against table '{table_name}' are not allowed"
            )));
        }
    }

    // Require db_reader for query endpoint
    let Some(ref db) = state.db_reader else {
        return Json(ApiResponse::<QueryResultDto>::error(
            "query endpoint not available (no db_reader)",
        ));
    };

    // Extract accessible petal_ids from the token scope for scope injection
    let scope_filter = build_scope_filter(&claims.scope);

    // Build the query with scope injection for node table queries
    let final_sql = inject_scope_filter(sql_trimmed, &scope_filter);

    // Build query with variables
    let mut query_builder = db.query(&final_sql);
    for (key, value) in &req.vars {
        query_builder = query_builder.bind((key.clone(), value.clone()));
    }

    println!("DEBUG: Inside execute_query - executing query: {}", final_sql);
    // Execute with timeout
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async { query_builder.await },
    )
    .await;
    println!("DEBUG: Inside execute_query - query execution finished");

    match result {
        Ok(Ok(mut response)) => {
            // Collect all statement results
            let mut data = Vec::new();
            let mut idx = 0usize;
            loop {
                match response.take::<Vec<serde_json::Value>>(idx) {
                    Ok(rows) => {
                        data.extend(rows);
                        idx += 1;
                    }
                    Err(_) => break,
                }
            }
            Json(ApiResponse::success(QueryResultDto { data }))
        }
        Ok(Err(e)) => Json(ApiResponse::<QueryResultDto>::error(format!("query failed: {e}"))),
        Err(_) => Json(ApiResponse::<QueryResultDto>::error("query timed out (5s)")),
    }
}

/// POST /api/v1/query/elevated — execute a SurrealQL statement with mutation support.
///
/// RBAC: Manager+ required. Scope enforced. Allows:
/// - SELECT, CREATE, UPDATE, DELETE against whitelisted tables
/// - DEFINE FUNCTION / REMOVE FUNCTION for stored procedures
/// - LET bindings and RETURN statements
/// - Multi-statement (semicolons allowed)
///
/// Blocked: DEFINE TABLE, DEFINE FIELD, REMOVE TABLE, DEFINE EVENT, INFO,
///          and any system-level DDL that could alter the schema.
///
/// Rate limited to 5 req/s per user.
pub async fn execute_elevated_query(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<crate::types::QueryRequest>,
) -> impl IntoResponse {
    use crate::types::QueryResultDto;

    if let Err(_) = require_role(&claims, "manager") {
        return Json(ApiResponse::<QueryResultDto>::error("insufficient permissions (manager+ required)"));
    }

    // Rate limiting: 5 queries/sec for elevated queries
    {
        let now = std::time::Instant::now();
        let mut limiter = state.query_rate_limiter.lock().await;
        limiter.retain(|_, (_, ts)| now.duration_since(*ts) < std::time::Duration::from_secs(10));
        let key = format!("elevated:{}", claims.sub);
        let entry = limiter.entry(key).or_insert((0u32, now));
        if now.duration_since(entry.1) > std::time::Duration::from_secs(1) {
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > 5 {
                return Json(ApiResponse::<QueryResultDto>::error("rate limit exceeded (5 elevated queries/sec)"));
            }
        }
    }

    let sql_trimmed = req.sql.trim();
    let sql_upper = sql_trimmed.to_uppercase();

    // Block system-level DDL that could alter the core schema
    const BLOCKED_DDL: &[&str] = &[
        "DEFINE TABLE", "DEFINE FIELD", "DEFINE INDEX", "DEFINE EVENT",
        "REMOVE TABLE", "REMOVE FIELD", "REMOVE INDEX",
        "INFO", "SLEEP", "KILL",
    ];
    for ddl in BLOCKED_DDL {
        if sql_upper.contains(ddl) {
            return Json(ApiResponse::<QueryResultDto>::error(format!(
                "{ddl} is not allowed in elevated queries"
            )));
        }
    }

    // Allowed tables for mutations
    const ELEVATED_TABLES: &[&str] = &[
        "NODE", "NODE_LOG", "VERSE", "FRACTAL", "PETAL", "FIELD_DEF",
        "MODEL", "ASSET", "ROLE", "ROOM", "CRATE_REGISTRY", "CRATE_ENTRY", "VERSE_MEMBER",
    ];

    // Check that any FROM/INTO/UPDATE targets are in the allowed list
    for keyword in ["FROM", "INTO", "UPDATE"] {
        if let Some(pos) = sql_upper.find(keyword) {
            let after = &sql_upper[pos + keyword.len()..].trim_start();
            let table_name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !table_name.is_empty() && !ELEVATED_TABLES.contains(&table_name.as_str()) {
                return Json(ApiResponse::<QueryResultDto>::error(format!(
                    "table '{table_name}' is not accessible via elevated queries"
                )));
            }
        }
    }

    let Some(ref db) = state.db_reader else {
        return Json(ApiResponse::<QueryResultDto>::error(
            "elevated query endpoint not available (no db_reader)",
        ));
    };

    let mut query_builder = db.query(sql_trimmed);
    for (key, value) in &req.vars {
        query_builder = query_builder.bind((key.clone(), value.clone()));
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        async { query_builder.await },
    )
    .await;

    match result {
        Ok(Ok(mut response)) => {
            let mut data = Vec::new();
            let mut idx = 0usize;
            loop {
                match response.take::<Vec<serde_json::Value>>(idx) {
                    Ok(rows) => {
                        data.extend(rows);
                        idx += 1;
                    }
                    Err(_) => break,
                }
            }
            Json(ApiResponse::success(QueryResultDto { data }))
        }
        Ok(Err(e)) => Json(ApiResponse::<QueryResultDto>::error(format!("query failed: {e}"))),
        Err(_) => Json(ApiResponse::<QueryResultDto>::error("query timed out (10s)")),
    }
}

/// POST /api/v1/analytics/query — execute a DataFusion SQL query over the in-memory EntityStore.
///
/// RBAC: Viewer+ required. Scope enforced via petal_id scoping.
/// Rate limited to 10 req/s per user (shared with `/query` limiter).
///
/// Accepts `{ "sql": "SELECT ...", "petal_id": "optional-scope" }`.
/// Returns JSON rows from the DataFusion result.
pub async fn execute_analytics_query(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<crate::types::AnalyticsQueryRequest>,
) -> impl IntoResponse {
    use crate::types::QueryResultDto;

    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<QueryResultDto>::error("insufficient permissions"));
    }

    // Rate limiting (shared bucket with /query)
    {
        let now = std::time::Instant::now();
        let mut limiter = state.query_rate_limiter.lock().await;
        limiter.retain(|_, (_, ts)| now.duration_since(*ts) < std::time::Duration::from_secs(10));
        let key = format!("analytics:{}", claims.sub);
        let entry = limiter.entry(key).or_insert((0u32, now));
        if now.duration_since(entry.1) > std::time::Duration::from_secs(1) {
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > 10 {
                return Json(ApiResponse::<QueryResultDto>::error("rate limit exceeded (10 analytics queries/sec)"));
            }
        }
    }

    // Security: only allow SELECT statements
    let sql_trimmed = req.sql.trim();
    if !sql_trimmed.to_uppercase().starts_with("SELECT") {
        return Json(ApiResponse::<QueryResultDto>::error(
            "only SELECT statements are allowed in analytics queries",
        ));
    }

    let Some(ref store) = state.entity_store else {
        return Json(ApiResponse::<QueryResultDto>::error(
            "analytics endpoint not available (no entity_store)",
        ));
    };

    // Translate DuckDB-flavored SQL to DataFusion-compatible SQL
    let translated_sql = fe_query::duckdb_compat::translate(sql_trimmed);

    let analytics_ctx = fe_query::columnar::context::AnalyticsContext::new(store.clone());

    // Register node table scoped to petal if provided
    if let Err(e) = analytics_ctx.register_node_table("nodes", req.petal_id.as_deref()) {
        return Json(ApiResponse::<QueryResultDto>::error(format!(
            "failed to register node table: {e}"
        )));
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        analytics_ctx.execute_to_json(&translated_sql),
    )
    .await
    {
        Ok(Ok(data)) => Json(ApiResponse::success(QueryResultDto { data })),
        Ok(Err(e)) => Json(ApiResponse::<QueryResultDto>::error(format!("analytics query failed: {e}"))),
        Err(_) => Json(ApiResponse::<QueryResultDto>::error("analytics query timed out (10s)")),
    }
}

// ---------------------------------------------------------------------------
// Field definition (property schema) endpoints
// ---------------------------------------------------------------------------

/// POST /api/v1/field-defs — create a new field definition.
///
/// RBAC: Manager+ required. Scope must be valid and within the token's scope.
pub async fn create_field_def(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<CreateFieldDefRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "manager") {
        return Json(ApiResponse::<FieldDefDto>::error("insufficient permissions"));
    }
    if !is_valid_scope(&req.scope) {
        return Json(ApiResponse::<FieldDefDto>::error("invalid scope"));
    }
    if require_scope(&claims, &req.scope).is_err() {
        return Json(ApiResponse::<FieldDefDto>::error("insufficient scope"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateFieldDef {
            scope: req.scope.clone(),
            entity_type: req.entity_type.clone(),
            key: req.key.clone(),
            value_type: req.value_type.clone(),
            default_val: req.default_val.clone(),
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<FieldDefDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::FieldDefCreated { field_def_id, scope, key })) => {
            Json(ApiResponse::success(FieldDefDto {
                field_def_id,
                scope,
                entity_type: req.entity_type,
                key,
                value_type: req.value_type,
                default_val: req.default_val,
                created_by: claims.sub,
                created_at: String::new(),
            }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<FieldDefDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<FieldDefDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<FieldDefDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<FieldDefDto>::error("request timed out")),
    }
}

/// GET /api/v1/field-defs/:scope — list field definitions for a scope.
///
/// RBAC: Viewer+ required.
pub async fn list_field_defs(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(scope): Path<String>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<Vec<FieldDefDto>>::error("insufficient permissions"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::ListFieldDefs { scope: scope.clone() },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<Vec<FieldDefDto>>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::FieldDefsListed { field_defs, .. })) => {
            let dtos: Vec<FieldDefDto> = field_defs.into_iter().map(|f| FieldDefDto {
                field_def_id: f.field_def_id,
                scope: f.scope,
                entity_type: f.entity_type,
                key: f.key,
                value_type: f.value_type,
                default_val: f.default_val,
                created_by: f.created_by,
                created_at: f.created_at,
            }).collect();
            Json(ApiResponse::success(dtos))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<Vec<FieldDefDto>>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<Vec<FieldDefDto>>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<Vec<FieldDefDto>>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<Vec<FieldDefDto>>::error("request timed out")),
    }
}

/// PATCH /api/v1/field-defs/:field_def_id — update a field definition.
///
/// RBAC: Manager+ required.
pub async fn update_field_def(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(field_def_id): Path<String>,
    Json(req): Json<UpdateFieldDefRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "manager") {
        return Json(ApiResponse::<FieldDefDto>::error("insufficient permissions"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::UpdateFieldDef {
            field_def_id: field_def_id.clone(),
            value_type: req.value_type.clone(),
            default_val: req.default_val.clone(),
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<FieldDefDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::FieldDefUpdated { field_def_id })) => {
            Json(ApiResponse::success(FieldDefDto {
                field_def_id,
                scope: String::new(),
                entity_type: String::new(),
                key: String::new(),
                value_type: req.value_type,
                default_val: req.default_val,
                created_by: String::new(),
                created_at: String::new(),
            }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<FieldDefDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<FieldDefDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<FieldDefDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<FieldDefDto>::error("request timed out")),
    }
}

/// DELETE /api/v1/field-defs/:field_def_id — delete a field definition.
///
/// RBAC: Manager+ required.
pub async fn delete_field_def(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(field_def_id): Path<String>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "manager") {
        return Json(ApiResponse::<FieldDefDto>::error("insufficient permissions"));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::DeleteFieldDef {
            field_def_id: field_def_id.clone(),
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<FieldDefDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::FieldDefDeleted { field_def_id })) => {
            Json(ApiResponse::success(FieldDefDto {
                field_def_id,
                scope: String::new(),
                entity_type: String::new(),
                key: String::new(),
                value_type: String::new(),
                default_val: None,
                created_by: String::new(),
                created_at: String::new(),
            }))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<FieldDefDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<FieldDefDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<FieldDefDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<FieldDefDto>::error("request timed out")),
    }
}

/// Build a scope filter clause from the token's scope string.
/// Returns a WHERE clause fragment like `petal_id = 'p1'` or empty for broad access.
fn build_scope_filter(scope: &str) -> String {
    let Ok(parts) = fe_database::parse_scope(scope) else {
        return String::new();
    };
    if let Some(ref petal_id) = parts.petal_id {
        format!("petal_id = '{}'", petal_id.replace('\'', ""))
    } else {
        // Verse or fractal scope — broader access, pass through without filter.
        String::new()
    }
}

/// Inject scope filter into SQL that queries the `node` table.
/// Simple heuristic: if the query touches `FROM node` and we have a scope filter,
/// append/inject a WHERE clause.
fn inject_scope_filter(sql: &str, scope_filter: &str) -> String {
    if scope_filter.is_empty() {
        return sql.to_string();
    }
    let sql_upper = sql.to_uppercase();
    if !sql_upper.contains("FROM NODE") {
        return sql.to_string();
    }
    // If there's already a WHERE, add AND; otherwise add WHERE
    if sql_upper.contains("WHERE") {
        format!("{sql} AND {scope_filter}")
    } else {
        format!("{sql} WHERE {scope_filter}")
    }
}

// ---------------------------------------------------------------------------
// Waypoint CRUD
// ---------------------------------------------------------------------------

/// POST /api/v1/petals/:petal_id/waypoints — create a waypoint node.
///
/// Projects lat/lon/ele to world-space using the terrain projection.
/// RBAC: Editor+ required.
pub async fn create_waypoint(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(req): Json<crate::types::CreateWaypointRequest>,
) -> impl IntoResponse {
    let scope = fe_database::build_scope("", None, Some(&petal_id));
    if let Err(_) = require_role_and_scope(&claims, "editor", &scope) {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient permissions or scope"));
    }

    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<CreatedEntityDto>::error("invalid petal_id"));
    }

    // Project WGS84 to local coordinates using the petal's terrain projection.
    // We read the petal's config from the database first.
    let Some(proj) = load_petal_projection(&state, &petal_id).await else {
        // Fallback: use identity projection
        tracing::warn!("no terrain config for petal {petal_id}, using identity projection");
        let position = [req.lat as f32, req.ele as f32, req.lon as f32];
        return create_waypoint_with_position(&state, &claims, &petal_id, &req, position).await;
    };

    match proj.wgs84_to_local(req.lat, req.lon, req.ele) {
        Ok([x, y, z]) => {
            let position = [x as f32, y as f32, z as f32];
            create_waypoint_with_position(&state, &claims, &petal_id, &req, position).await
        }
        Err(e) => Json(ApiResponse::<CreatedEntityDto>::error(format!(
            "projection failed: {e}"
        ))),
    }
}

async fn create_waypoint_with_position(
    state: &Arc<crate::server::ApiState>,
    _claims: &ApiClaims,
    petal_id: &str,
    req: &crate::types::CreateWaypointRequest,
    position: [f32; 3],
) -> Json<ApiResponse<CreatedEntityDto>> {
    let mut props = serde_json::Map::new();
    props.insert("lat".into(), serde_json::json!(req.lat));
    props.insert("lon".into(), serde_json::json!(req.lon));
    props.insert("ele".into(), serde_json::json!(req.ele));
    props.insert("waypoint".into(), serde_json::json!(true));
    if let Some(ref desc) = req.description {
        props.insert("description".into(), serde_json::json!(desc));
    }
    if let Some(ref sym) = req.symbol {
        props.insert("symbol".into(), serde_json::json!(sym));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: req.name.clone(),
            position,
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodeCreated { id, name, .. })) => {
            // Set waypoint properties
            for (key, value) in props {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
                    cmd: DbCommand::SetNodeProperty {
                        node_id: id.clone(),
                        key,
                        value,
                    },
                    reply_tx: tx,
                });
                // Fire-and-forget: ignore result
                let _ = rx.await;
            }
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(e))) => {
            tracing::error!("create_waypoint failed: {e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// PATCH /api/v1/nodes/:waypoint_id/move — move a waypoint to new lat/lon.
///
/// Reprojects to world-space and updates both properties and transform.
/// RBAC: Editor+ required.
pub async fn move_waypoint(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(waypoint_id): Path<String>,
    Json(req): Json<crate::types::MoveWaypointRequest>,
) -> impl IntoResponse {
    if let Err(_) = require_role(&claims, "editor") {
        return Json(ApiResponse::error("insufficient permissions"));
    }
    if !is_valid_ulid(&waypoint_id) {
        return Json(ApiResponse::error("invalid waypoint_id"));
    }

    // Resolve node scope
    let Some(scope) = resolve_node_scope(&state, &waypoint_id).await else {
        return Json(ApiResponse::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::error("insufficient scope"));
    }

    // Get the petal_id from the node to find the projection
    let Some(petal_id) = resolve_node_petal(&state, &waypoint_id).await else {
        return Json(ApiResponse::error("could not resolve node petal"));
    };

    // Get current elevation if not provided
    let ele = req.ele.unwrap_or_else(|| {
        // Try to get existing elevation from node properties
        0.0
    });

    let proj = load_petal_projection(&state, &petal_id).await;
    let position = match &proj {
        Some(p) => match p.wgs84_to_local(req.lat, req.lon, ele) {
            Ok([x, y, z]) => [x as f32, y as f32, z as f32],
            Err(_) => [req.lat as f32, ele as f32, req.lon as f32],
        },
        None => [req.lat as f32, ele as f32, req.lon as f32],
    };

    // Update transform
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
        cmd: DbCommand::UpdateNodeTransform {
            node_id: waypoint_id.clone(),
            position,
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        },
        reply_tx: tx,
    });
    match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
        Ok(Ok(DbResult::Error(e))) => {
            return Json(ApiResponse::error(format!("transform update failed: {e}")));
        }
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            return Json(ApiResponse::error("transform update channel closed"));
        }
        Err(_) => {
            return Json(ApiResponse::error("transform update timed out"));
        }
    }

    // Update properties
    for (key, value) in [
        ("lat", serde_json::json!(req.lat)),
        ("lon", serde_json::json!(req.lon)),
        ("ele", serde_json::json!(ele)),
    ] {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
            cmd: DbCommand::SetNodeProperty {
                node_id: waypoint_id.clone(),
                key: key.to_string(),
                value,
            },
            reply_tx: tx,
        });
        let _ = rx.await;
    }

    Json(ApiResponse::<serde_json::Value>::success(serde_json::json!({
        "node_id": waypoint_id,
        "lat": req.lat,
        "lon": req.lon,
        "ele": ele,
    })))
}

/// GET /api/v1/nodes/:track_id/elevation-profile — elevation profile for a track.
///
/// Returns an array of { distance_m, elevation_m } points.
/// RBAC: Viewer+ required.
pub async fn get_elevation_profile(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(track_id): Path<String>,
) -> impl IntoResponse {
    use crate::types::ElevationPointDto;

    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&track_id) {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error("invalid track_id"));
    }

    // Resolve node scope
    let Some(scope) = resolve_node_scope(&state, &track_id).await else {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error("insufficient scope"));
    }

    // Try direct DB query first
    if let Some(ref db) = state.db_reader {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            direct_get_elevation_profile(db, &track_id),
        )
        .await
        {
            Ok(Ok(profile)) => return Json(ApiResponse::success(profile)),
            Ok(Err(e)) => {
                return Json(ApiResponse::<Vec<ElevationPointDto>>::error(format!("query failed: {e}")))
            }
            Err(_) => {
                return Json(ApiResponse::<Vec<ElevationPointDto>>::error("request timed out"))
            }
        }
    }

    // Fallback: return empty profile
    Json(ApiResponse::success(Vec::<ElevationPointDto>::new()))
}

/// GET /api/v1/nodes/:track_id/stats — track statistics.
///
/// Returns { total_distance_m, min_elevation_m, max_elevation_m, avg_speed_kmh, duration_seconds }.
/// RBAC: Viewer+ required.
pub async fn get_track_stats(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(track_id): Path<String>,
) -> impl IntoResponse {
    use crate::types::TrackStatsDto;

    if let Err(_) = require_role(&claims, "viewer") {
        return Json(ApiResponse::<TrackStatsDto>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&track_id) {
        return Json(ApiResponse::<TrackStatsDto>::error("invalid track_id"));
    }

    let Some(scope) = resolve_node_scope(&state, &track_id).await else {
        return Json(ApiResponse::<TrackStatsDto>::error("could not resolve node scope"));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<TrackStatsDto>::error("insufficient scope"));
    }

    // Try direct DB query
    if let Some(ref db) = state.db_reader {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            direct_get_track_stats(db, &track_id),
        )
        .await
        {
            Ok(Ok(stats)) => return Json(ApiResponse::success(stats)),
            Ok(Err(e)) => {
                return Json(ApiResponse::<TrackStatsDto>::error(format!("query failed: {e}")))
            }
            Err(_) => {
                return Json(ApiResponse::<TrackStatsDto>::error("request timed out"))
            }
        }
    }

    // Fallback: return default stats
    Json(ApiResponse::success(TrackStatsDto {
        total_distance_m: 0.0,
        min_elevation_m: 0.0,
        max_elevation_m: 0.0,
        avg_speed_kmh: None,
        duration_seconds: None,
    }))
}

// ---------------------------------------------------------------------------
// Direct DB helpers for terrain endpoints
// ---------------------------------------------------------------------------

/// Load a petal's terrain configuration to get its projection.
async fn load_petal_projection(
    state: &Arc<crate::server::ApiState>,
    petal_id: &str,
) -> Option<fe_terrain::projection::Projection> {
    // Try to read terrain config from DB
    if let Some(ref db) = state.db_reader {
        let mut res = db
            .query("SELECT origin_lat, origin_lon, origin_ele FROM terrain_config WHERE petal_id = $pid LIMIT 1")
            .bind(("pid", petal_id.to_string()))
            .await
            .ok()?;
        let rows: Vec<serde_json::Value> = res.take(0).ok()?;
        if let Some(row) = rows.first() {
            let lat = row["origin_lat"].as_f64()?;
            let lon = row["origin_lon"].as_f64()?;
            let ele = row["origin_ele"].as_f64().unwrap_or(0.0);
            return Some(fe_terrain::projection::Projection::new(lat, lon, ele));
        }
    }

    // Fallback: channel-based query to get terrain config from properties
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
        cmd: DbCommand::GetNodeProperties {
            node_id: petal_id.to_string(),
        },
        reply_tx,
    });

    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::NodePropertiesLoaded { properties, .. })) => {
            let lat = properties.get("origin_lat")?.as_f64()?;
            let lon = properties.get("origin_lon")?.as_f64()?;
            let ele = properties.get("origin_ele").and_then(|v| v.as_f64()).unwrap_or(0.0);
            Some(fe_terrain::projection::Projection::new(lat, lon, ele))
        }
        _ => None,
    }
}

/// Resolve a node's petal_id via direct DB query.
async fn resolve_node_petal(state: &Arc<crate::server::ApiState>, node_id: &str) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        let mut res = db
            .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
            .bind(("nid", node_id.to_string()))
            .await
            .ok()?;
        let rows: Vec<serde_json::Value> = res.take(0).ok()?;
        return rows.first()?.get("petal_id")?.as_str().map(String::from);
    }
    None
}

async fn direct_get_elevation_profile(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    track_id: &str,
) -> anyhow::Result<Vec<crate::types::ElevationPointDto>> {
    // Read the node's properties which contain the route data
    let mut res = db
        .query("SELECT properties FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", track_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("take failed: {e}"))?;

    let Some(row) = rows.first() else {
        return Ok(Vec::new());
    };

    // Try to read route_points from properties
    let route_points = row["properties"]["route_points"].as_array();
    let Some(points) = route_points else {
        return Ok(Vec::new());
    };

    let mut profile = Vec::new();
    let mut cumulative_distance = 0.0;

    for window in points.windows(2) {
        let a = &window[0];
        let b = &window[1];
        let lat_a = a["lat"].as_f64().unwrap_or(0.0);
        let lon_a = a["lon"].as_f64().unwrap_or(0.0);
        let _ele_a = a["ele"].as_f64().unwrap_or(0.0);
        let lat_b = b["lat"].as_f64().unwrap_or(0.0);
        let lon_b = b["lon"].as_f64().unwrap_or(0.0);
        let ele_b = b["ele"].as_f64().unwrap_or(0.0);

        let dist = fe_terrain::gpx::stats::haversine_m(lat_a, lon_a, lat_b, lon_b);
        cumulative_distance += dist;

        profile.push(crate::types::ElevationPointDto {
            distance_m: cumulative_distance,
            elevation_m: ele_b,
        });
    }

    Ok(profile)
}

async fn direct_get_track_stats(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    track_id: &str,
) -> anyhow::Result<crate::types::TrackStatsDto> {
    let mut res = db
        .query("SELECT properties FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", track_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("take failed: {e}"))?;

    let Some(row) = rows.first() else {
        return Ok(crate::types::TrackStatsDto {
            total_distance_m: 0.0,
            min_elevation_m: 0.0,
            max_elevation_m: 0.0,
            avg_speed_kmh: None,
            duration_seconds: None,
        });
    };

    let route_points = row["properties"]["route_points"].as_array();
    let Some(points) = route_points else {
        return Ok(crate::types::TrackStatsDto {
            total_distance_m: 0.0,
            min_elevation_m: 0.0,
            max_elevation_m: 0.0,
            avg_speed_kmh: None,
            duration_seconds: None,
        });
    };

    let mut total_distance = 0.0;
    let mut min_elevation = f64::MAX;
    let mut max_elevation = f64::MIN;

    for window in points.windows(2) {
        let a = &window[0];
        let b = &window[1];
        let lat_a = a["lat"].as_f64().unwrap_or(0.0);
        let lon_a = a["lon"].as_f64().unwrap_or(0.0);
        let lat_b = b["lat"].as_f64().unwrap_or(0.0);
        let lon_b = b["lon"].as_f64().unwrap_or(0.0);
        let ele_b = b["ele"].as_f64().unwrap_or(0.0);

        total_distance += fe_terrain::gpx::stats::haversine_m(lat_a, lon_a, lat_b, lon_b);
        min_elevation = min_elevation.min(ele_b);
        max_elevation = max_elevation.max(ele_b);
    }

    // Also check first point's elevation
    if let Some(first) = points.first() {
        let ele = first["ele"].as_f64().unwrap_or(0.0);
        min_elevation = min_elevation.min(ele);
        max_elevation = max_elevation.max(ele);
    }

    // Try to get duration from properties
    let duration = row["properties"]["duration_seconds"].as_f64();
    let avg_speed = if let (Some(dur), Some(dist)) = (duration, Some(total_distance)) {
        if dur > 0.0 {
            Some((dist / 1000.0) / (dur / 3600.0))
        } else {
            None
        }
    } else {
        None
    };

    Ok(crate::types::TrackStatsDto {
        total_distance_m: total_distance,
        min_elevation_m: if min_elevation == f64::MAX { 0.0 } else { min_elevation },
        max_elevation_m: if max_elevation == f64::MIN { 0.0 } else { max_elevation },
        avg_speed_kmh: avg_speed,
        duration_seconds: duration,
    })
}

// ---------------------------------------------------------------------------
// Tracking integration — snap-to-route + deviation alerting
// ---------------------------------------------------------------------------

/// When a transform update is received for a node that has a `tracking_route_id`
/// property, compute snap-to-route metrics and emit property changes. If the
/// node's deviation from its route exceeds `deviation_threshold_m` (default 50m),
/// also emit a `tracking_alert` property.
async fn check_tracking_and_alert(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    api_cmd_tx: &crossbeam::channel::Sender<ApiCommand>,
    node_id: &str,
    position: [f32; 3],
) {
    // Read node properties to find tracking_route_id
    let res = db
        .query("SELECT properties FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.into_iter().next());

    let Some(row) = res else { return };
    let props = &row["properties"];

    let tracking_route_id = props["tracking_route_id"].as_str().map(String::from);
    let Some(route_id) = tracking_route_id else { return };

    // Read the route node's route_points
    let route_res = db
        .query("SELECT properties FROM node WHERE node_id = $rid LIMIT 1")
        .bind(("rid", route_id))
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.into_iter().next());

    let Some(route_row) = route_res else { return };
    let route_props = &route_row["properties"];

    let route_points_json = route_props["route_points"].as_array();
    let Some(points) = route_points_json else { return };

    // Build route points array from JSON
    let route_pts: Vec<[f64; 3]> = points
        .iter()
        .filter_map(|p| {
            let x = p["lat"].as_f64()?;
            let y = p["ele"].as_f64().unwrap_or(0.0);
            let z = p["lon"].as_f64()?;
            Some([x, y, z])
        })
        .collect();

    if route_pts.len() < 2 { return }

    let tracker = fe_terrain::iot::PathTracker::new(route_pts);

    // Convert position to [f64; 3] for snap
    let pos = [position[0] as f64, position[1] as f64, position[2] as f64];
    let snap = tracker.snap_to_route(pos);

    // Emit route progress properties via SceneChange::PropertyChanged
    let metrics = [
        ("route_progress", serde_json::json!(snap.route_progress)),
        ("distance_remaining_m", serde_json::json!(snap.distance_remaining_m)),
        ("deviation_m", serde_json::json!(snap.deviation_m)),
        ("nearest_segment_index", serde_json::json!(snap.segment_index)),
    ];
    for (key, value) in metrics {
        // Fire-and-forget: persist property to DB
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = api_cmd_tx.send(ApiCommand::DbRequest {
            cmd: DbCommand::SetNodeProperty {
                node_id: node_id.to_string(),
                key: key.to_string(),
                value: value.clone(),
            },
            reply_tx: tx,
        });
    }

    // Deviation alerting
    let threshold = props["deviation_threshold_m"].as_f64().unwrap_or(50.0);
    if snap.deviation_m > threshold {
        let alert_value = serde_json::json!("off_route");
        tracing::warn!(
            node_id, deviation_m = snap.deviation_m, threshold,
            "tracking alert: off_route"
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = api_cmd_tx.send(ApiCommand::DbRequest {
            cmd: DbCommand::SetNodeProperty {
                node_id: node_id.to_string(),
                key: "tracking_alert".to_string(),
                value: alert_value,
            },
            reply_tx: tx,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use axum::Json;
    use axum::extract::State;
    use axum::Extension;
    use axum::response::IntoResponse;
    use fe_identity::api_token::ApiClaims;
    use crate::types::QueryRequest;
    use crate::server::ApiState;

    async fn setup_test_db() -> surrealdb::Surreal<surrealdb::engine::local::Db> {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        fe_database::schema::apply_all(&db).await.expect("apply schema");
        db
    }

    #[tokio::test]
    async fn test_execute_query_whitelist() {
        println!("DEBUG: Starting test_execute_query_whitelist");
        println!("DEBUG: Setting up test DB");
        let db = setup_test_db().await;
        println!("DEBUG: DB setup complete");
        
        let now = chrono::Utc::now().to_rfc3339();
        println!("DEBUG: Creating asset record");
        db.query("CREATE asset CONTENT {
            asset_id: 'asset-1',
            name: 'test-model',
            content_type: 'model/gltf-binary',
            size_bytes: 1024,
            created_at: $now,
            content_hash: 'abc123hash'
        }").bind(("now", serde_json::json!(now))).await.unwrap().check().unwrap();
        println!("DEBUG: Asset record created");

        println!("DEBUG: Creating crate_registry record");
        db.query("CREATE crate_registry CONTENT {
            hexon_uri: 'hexon://test-uri',
            manifest_hash: 'manifest-hash-val',
            publisher_did: 'did:key:z6MkPub',
            hexon_type: 'model',
            version: '1.0.0',
            name: 'test-crate',
            tags: '[\"test\"]',
            petal_id: 'petal-1',
            size_bytes: 4096,
            installed_at: $now,
            signature_valid: true
        }").bind(("now", serde_json::json!(now))).await.unwrap().check().unwrap();
        println!("DEBUG: Crate registry record created");

        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap();

        let state = Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key,
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: None,
            cors_origins: vec![],
            db_reader: Some(Arc::new(db)),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
        });

        let claims = ApiClaims {
            sub: "did:key:z6MkUser".to_string(),
            scope: "VERSE#v1".to_string(),
            max_role: "editor".to_string(),
            token_type: "api".to_string(),
            iat: 0,
            exp: u64::MAX,
            jti: "jti-1".to_string(),
        };

        // 1. Query asset
        let req = QueryRequest {
            sql: "SELECT * FROM asset".to_string(),
            vars: std::collections::HashMap::new(),
        };
        println!("DEBUG: Executing first query");
        let res = execute_query(State(state.clone()), Extension(claims.clone()), Json(req)).await;
        println!("DEBUG: First query returned");
        let response = res.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        
        println!("DEBUG: Reading response body for first query");
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(body_json["success"].as_bool().unwrap_or(false), "query failed: {:?}", body_json);
        let data = body_json["data"]["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["asset_id"], "asset-1");

        // 2. Query crate_registry
        let req = QueryRequest {
            sql: "SELECT * FROM crate_registry".to_string(),
            vars: std::collections::HashMap::new(),
        };
        println!("DEBUG: Executing second query");
        let res = execute_query(State(state.clone()), Extension(claims.clone()), Json(req)).await;
        println!("DEBUG: Second query returned");
        let response = res.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        println!("DEBUG: Reading response body for second query");
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(body_json["success"].as_bool().unwrap_or(false), "query failed: {:?}", body_json);
        let data = body_json["data"]["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["hexon_uri"], "hexon://test-uri");

        // 3. Blocked query (mutation keyword)
        let req = QueryRequest {
            sql: "DELETE FROM asset WHERE asset_id = 'asset-1'".to_string(),
            vars: std::collections::HashMap::new(),
        };
        println!("DEBUG: Executing third query (blocked)");
        let res = execute_query(State(state.clone()), Extension(claims.clone()), Json(req)).await;
        println!("DEBUG: Third query returned");
        let response = res.into_response();
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!body_json["success"].as_bool().unwrap_or(false));
        assert!(body_json["error"].as_str().unwrap().contains("not allowed"));
    }
}
