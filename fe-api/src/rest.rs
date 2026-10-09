use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, TransformUpdate};

use crate::auth::{require_role, require_scope};
use crate::types::{
    hierarchy_to_dto, is_valid_scope, is_valid_ulid, ApiResponse, CreateFieldDefRequest,
    CreateFractalRequest, CreateNodeRequest, CreatePetalRequest, CreateVerseRequest,
    CreatedEntityDto, FieldDefDto, PropertiesDto, PropertySetDto, SetPropertyRequest,
    UpdateFieldDefRequest, UpdateTransformRequest, VerseDto,
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

/// The ONE denial text for not-found / out-of-scope / ancestry-mismatch, so a
/// denial never reveals whether an id exists (DEC-C21; §mcp-dispatch).
pub const NOT_FOUND_OR_DENIED: &str = "not found or not permitted";

/// True iff every caller-claimed ancestor id equals the DB-resolved `scope`'s chain.
pub(crate) fn ancestry_matches(
    scope: &str,
    verse_id: Option<&str>,
    fractal_id: Option<&str>,
) -> bool {
    let Ok(parts) = fe_database::parse_scope(scope) else {
        return false;
    };
    verse_id.is_none_or(|v| v == parts.verse_id)
        && fractal_id.is_none_or(|f| parts.fractal_id.as_deref() == Some(f))
}

/// The MCP `HierarchyArgs` discipline for REST creates: the DB-resolved write
/// target exists, the token covers it, and the URL ancestry agrees with it.
fn write_target_authorized(
    claims: &ApiClaims,
    target_scope: Option<&str>,
    verse_id: Option<&str>,
    fractal_id: Option<&str>,
) -> bool {
    target_scope.is_some_and(|scope| {
        require_scope(claims, scope).is_ok() && ancestry_matches(scope, verse_id, fractal_id)
    })
}

/// A REST create denial with a real HTTP status (`{ok:false}` envelope body).
fn create_denied(status: StatusCode, msg: &str) -> Response {
    (status, Json(ApiResponse::<CreatedEntityDto>::error(msg))).into_response()
}

/// A post-authz REST create failure (historical 200 + `{ok:false}` shape).
fn create_failed(msg: impl Into<String>) -> Response {
    Json(ApiResponse::<CreatedEntityDto>::error(msg)).into_response()
}

fn created(id: String, name: String) -> Response {
    Json(ApiResponse::success(CreatedEntityDto { id, name })).into_response()
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
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<Vec<VerseDto>>::error(
            "insufficient permissions",
        ));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::GetHierarchy { reply_tx };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<Vec<VerseDto>>::error(
            "internal channel closed",
        ));
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
    if require_role(&claims, "manager").is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error(
            "insufficient permissions",
        ));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateVerse { name: req.name },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error(
            "internal channel closed",
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::VerseCreated { id, name, .. })) => {
            Json(ApiResponse::success(CreatedEntityDto { id, name }))
        }
        Ok(Ok(DbResult::Error(_e))) => {
            tracing::error!("create_verse failed: {_e}");
            Json(ApiResponse::<CreatedEntityDto>::error("operation failed"))
        }
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error(
            "unexpected response",
        )),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// POST /api/v1/verses/:verse_id/fractals — create a fractal in a verse.
///
/// The verse must exist (DB-resolved) and be covered by the token; a missing
/// or foreign verse denies with [`NOT_FOUND_OR_DENIED`] (404).
pub async fn create_fractal(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
    Json(req): Json<CreateFractalRequest>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return create_denied(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&verse_id) {
        return create_denied(StatusCode::BAD_REQUEST, "invalid verse_id");
    }
    let target = resolve_verse_scope(&state, &verse_id).await;
    if !write_target_authorized(&claims, target.as_deref(), None, None) {
        return create_denied(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }

    let cmd = DbCommand::CreateFractal {
        verse_id,
        name: req.name,
    };
    match db_round_trip(&state, cmd).await {
        Ok(DbResult::FractalCreated { id, name, .. }) => created(id, name),
        Ok(DbResult::Error(e)) => {
            tracing::error!("create_fractal failed: {e}");
            create_failed("operation failed")
        }
        Ok(_) => create_failed("unexpected response"),
        Err(e) => create_failed(e),
    }
}

/// POST /api/v1/verses/:verse_id/fractals/:fractal_id/petals — create a petal.
///
/// Authz anchors on the WRITE TARGET (the fractal) resolved from the DB, never
/// on the URL prefix: the URL `verse_id` must match the fractal's stored verse
/// (DEC-C21 — the REST twin of the MCP decoy wart; §mcp-dispatch).
pub async fn create_petal(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((verse_id, fractal_id)): Path<(String, String)>,
    Json(req): Json<CreatePetalRequest>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return create_denied(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&verse_id) || !is_valid_ulid(&fractal_id) {
        return create_denied(StatusCode::BAD_REQUEST, "invalid verse_id or fractal_id");
    }
    let target = resolve_fractal_scope(&state, &fractal_id).await;
    if !write_target_authorized(&claims, target.as_deref(), Some(&verse_id), None) {
        return create_denied(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }

    let cmd = DbCommand::CreatePetal {
        fractal_id,
        name: req.name,
    };
    match db_round_trip(&state, cmd).await {
        Ok(DbResult::PetalCreated { id, name, .. }) => created(id, name),
        Ok(DbResult::Error(e)) => {
            tracing::error!("create_petal failed: {e}");
            create_failed("operation failed")
        }
        Ok(_) => create_failed("unexpected response"),
        Err(e) => create_failed(e),
    }
}

/// POST /api/v1/verses/:vid/fractals/:fid/petals/:pid/nodes — create a node.
///
/// Authz anchors on the WRITE TARGET (the petal) resolved from the DB; the
/// URL verse/fractal ids must match its stored chain (DEC-C21 — a token for
/// `VERSE#A` can no longer write into a foreign petal by prefixing its own
/// verse in the URL).
pub async fn create_node(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((verse_id, fractal_id, petal_id)): Path<(String, String, String)>,
    Json(req): Json<CreateNodeRequest>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return create_denied(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&verse_id) || !is_valid_ulid(&fractal_id) || !is_valid_ulid(&petal_id) {
        return create_denied(
            StatusCode::BAD_REQUEST,
            "invalid verse_id, fractal_id or petal_id",
        );
    }
    let target = resolve_petal_scope(&state, &petal_id).await;
    if !write_target_authorized(
        &claims,
        target.as_deref(),
        Some(&verse_id),
        Some(&fractal_id),
    ) {
        return create_denied(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }
    create_node_in_petal(&state, petal_id, req).await
}

/// POST /api/v1/nodes — legacy flat create (for MCP and existing integrations).
///
/// DEPRECATION NOTE: Prefer the hierarchical endpoint
/// `POST /api/v1/verses/:vid/fractals/:fid/petals/:pid/nodes`. Both resolve
/// the petal's full scope from the DB and enforce it against the token.
pub async fn create_node_legacy(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<CreateNodeRequest>,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return create_denied(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let petal_id = req.petal_id.clone().unwrap_or_default();
    if !is_valid_ulid(&petal_id) {
        return create_denied(StatusCode::BAD_REQUEST, "invalid petal_id");
    }
    let target = resolve_petal_scope(&state, &petal_id).await;
    if !write_target_authorized(&claims, target.as_deref(), None, None) {
        return create_denied(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }
    create_node_in_petal(&state, petal_id, req).await
}

/// `CreateNode` into an authorized petal (shared by both REST node creates).
async fn create_node_in_petal(
    state: &crate::server::ApiState,
    petal_id: String,
    req: CreateNodeRequest,
) -> Response {
    let cmd = DbCommand::CreateNode {
        petal_id,
        name: req.name,
        position: req.position.unwrap_or([0.0, 0.0, 0.0]),
        correlation_id: None,
    };
    match db_round_trip(state, cmd).await {
        Ok(DbResult::NodeCreated { id, name, .. }) => created(id, name),
        Ok(DbResult::Error(e)) => {
            tracing::error!("create_node failed: {e}");
            create_failed("operation failed")
        }
        Ok(_) => create_failed("unexpected response"),
        Err(e) => create_failed(e),
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
    if require_role(&claims, "editor").is_err() {
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

    let broadcast_receivers = persist_transform(
        &state,
        &claims,
        &node_id,
        req.position,
        req.rotation,
        req.scale,
    );

    Json(ApiResponse::success(serde_json::json!({
        "node_id": node_id,
        "broadcast_receivers": broadcast_receivers,
        "persist": "queued"
    })))
}

/// Transform write core shared by REST `update_transform` and the MCP tool
/// (caller has authorized): optimistic broadcast, fire-and-forget
/// `TransformPersist` (never a pending waiter — the DB thread sends no reply),
/// tracking-alert hook. Returns the broadcast receiver count.
pub(crate) fn persist_transform(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> usize {
    // Optimistic broadcast: WS subscribers + Bevy bridge, BEFORE the DB persist.
    let broadcast_receivers = state
        .transform_broadcast_tx
        .send(TransformUpdate {
            node_id: node_id.to_string(),
            petal_id: String::new(),
            position,
            rotation,
            scale,
            timestamp_ms: now_ms(),
            source_did: claims.sub.clone(),
        })
        .unwrap_or(0);

    // On failure the DB thread emits SceneChange::TransformFailed (WS rollback).
    if state
        .api_cmd_tx
        .send(ApiCommand::TransformPersist {
            node_id: node_id.to_string(),
            position,
            rotation,
            scale,
        })
        .is_err()
    {
        tracing::warn!(node_id, "TransformPersist send failed (DB bridge gone)");
    }

    // Tracking integration: a `tracking_route_id` node gets snap/deviation checks.
    if let Some(ref db) = state.db_reader {
        let db = db.clone();
        let api_cmd_tx = state.api_cmd_tx.clone();
        let tracked_node_id = node_id.to_string();
        tokio::spawn(async move {
            let _ = check_tracking_and_alert(&db, &api_cmd_tx, &tracked_node_id, position).await;
        });
    }
    broadcast_receivers
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

    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<TransformDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<TransformDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<TransformDto>::error(
            "could not resolve node scope",
        ));
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
            Ok(Err(e)) => Json(ApiResponse::<TransformDto>::error(format!(
                "query failed: {e}"
            ))),
            Err(_) => Json(ApiResponse::<TransformDto>::error("request timed out")),
        }
    } else {
        // Fallback: channel-based query
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let cmd = ApiCommand::DbRequest {
            cmd: DbCommand::GetNodeTransform {
                node_id: node_id.clone(),
            },
            reply_tx,
        };
        if state.api_cmd_tx.send(cmd).is_err() {
            return Json(ApiResponse::<TransformDto>::error(
                "internal channel closed",
            ));
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
            Ok(Ok(DbResult::NodeTransformLoaded {
                position,
                rotation,
                scale,
                ..
            })) => Json(ApiResponse::success(TransformDto {
                position,
                rotation,
                scale,
            })),
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
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<PropertySetDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<PropertySetDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement — deny if resolution fails
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<PropertySetDto>::error(
            "could not resolve node scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PropertySetDto>::error("insufficient scope"));
    }

    match set_property_core(&state, &node_id, req.key, req.value).await {
        Ok(dto) => Json(ApiResponse::success(dto)),
        Err(e) => Json(ApiResponse::<PropertySetDto>::error(e)),
    }
}

/// Send one `DbRequest` and await its reply with the standard 5 s budget
/// (error strings are the REST surface's long-standing ones).
pub(crate) async fn db_round_trip(
    state: &crate::server::ApiState,
    cmd: DbCommand,
) -> Result<DbResult, String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest { cmd, reply_tx })
        .is_err()
    {
        tracing::warn!("DbRequest send failed (DB bridge gone)");
        return Err("internal channel closed".to_string());
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err("request cancelled".to_string()),
        Err(_) => Err("request timed out".to_string()),
    }
}

/// Property-set core shared by REST + MCP `set_property` (caller has authorized).
pub(crate) async fn set_property_core(
    state: &crate::server::ApiState,
    node_id: &str,
    key: String,
    value: serde_json::Value,
) -> Result<PropertySetDto, String> {
    let cmd = DbCommand::SetNodeProperty {
        node_id: node_id.to_string(),
        key,
        value,
    };
    match db_round_trip(state, cmd).await? {
        DbResult::NodePropertySet { node_id, key } => Ok(PropertySetDto { node_id, key }),
        DbResult::Error(e) => Err(e),
        _ => Err("unexpected response".to_string()),
    }
}

/// Property-read core shared by REST + MCP `get_properties` (caller has authorized).
pub(crate) async fn get_properties_core(
    state: &crate::server::ApiState,
    node_id: &str,
) -> Result<PropertiesDto, String> {
    let cmd = DbCommand::GetNodeProperties {
        node_id: node_id.to_string(),
    };
    match db_round_trip(state, cmd).await? {
        DbResult::NodePropertiesLoaded {
            node_id,
            properties,
        } => Ok(PropertiesDto {
            node_id,
            properties,
        }),
        DbResult::Error(e) => Err(e),
        _ => Err("unexpected response".to_string()),
    }
}

/// Property-delete core shared by REST + MCP `delete_property` (caller has authorized).
pub(crate) async fn delete_property_core(
    state: &crate::server::ApiState,
    node_id: &str,
    key: &str,
) -> Result<PropertySetDto, String> {
    let cmd = DbCommand::DeleteNodeProperty {
        node_id: node_id.to_string(),
        key: key.to_string(),
    };
    match db_round_trip(state, cmd).await? {
        DbResult::NodePropertyDeleted { .. } => Ok(PropertySetDto {
            node_id: node_id.to_string(),
            key: key.to_string(),
        }),
        DbResult::Error(e) => Err(e),
        _ => Err("unexpected response".to_string()),
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
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<PropertiesDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<PropertiesDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement — deny if resolution fails
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<PropertiesDto>::error(
            "could not resolve node scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PropertiesDto>::error("insufficient scope"));
    }

    match get_properties_core(&state, &node_id).await {
        Ok(dto) => Json(ApiResponse::success(dto)),
        Err(e) => Json(ApiResponse::<PropertiesDto>::error(e)),
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
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<PropertySetDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&node_id) {
        return Json(ApiResponse::<PropertySetDto>::error("invalid node_id"));
    }

    // Resolve node scope for enforcement — deny if resolution fails
    let Some(scope) = resolve_node_scope(&state, &node_id).await else {
        return Json(ApiResponse::<PropertySetDto>::error(
            "could not resolve node scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<PropertySetDto>::error("insufficient scope"));
    }

    match delete_property_core(&state, &node_id, &key).await {
        Ok(dto) => Json(ApiResponse::success(dto)),
        Err(e) => Json(ApiResponse::<PropertySetDto>::error(e)),
    }
}

// ---------------------------------------------------------------------------
// Scope resolution helpers
// ---------------------------------------------------------------------------

/// Resolve a petal_id to its full scope string.
///
/// Uses a direct DB query when `db_reader` is available, falling back to the
/// crossbeam channel otherwise.
pub(crate) async fn resolve_petal_scope(
    state: &crate::server::ApiState,
    petal_id: &str,
) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return direct_resolve_petal_scope(db, petal_id).await;
    }
    // Fallback: channel-based resolution
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::ResolvePetalScope {
                petal_id: petal_id.to_string(),
            },
            reply_tx,
        })
        .ok()?;
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
pub(crate) async fn resolve_node_scope(
    state: &crate::server::ApiState,
    node_id: &str,
) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return direct_resolve_node_scope(db, node_id).await;
    }
    // Fallback: channel-based resolution
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::ResolveNodeScope {
                node_id: node_id.to_string(),
            },
            reply_tx,
        })
        .ok()?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

/// Resolve a fractal_id to its scope string (direct, else `ResolveFractalScope`).
pub(crate) async fn resolve_fractal_scope(
    state: &crate::server::ApiState,
    fractal_id: &str,
) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return direct_resolve_fractal_scope(db, fractal_id).await;
    }
    resolve_scope_via_channel(
        state,
        DbCommand::ResolveFractalScope {
            fractal_id: fractal_id.to_string(),
        },
    )
    .await
}

/// Resolve a verse_id to `VERSE#<id>` iff the verse row exists (direct, else
/// `ResolveVerseScope`).
pub(crate) async fn resolve_verse_scope(
    state: &crate::server::ApiState,
    verse_id: &str,
) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return direct_resolve_verse_scope(db, verse_id).await;
    }
    resolve_scope_via_channel(
        state,
        DbCommand::ResolveVerseScope {
            verse_id: verse_id.to_string(),
        },
    )
    .await
}

/// Channel fallback shared by the fractal/verse resolvers (3 s budget).
async fn resolve_scope_via_channel(
    state: &crate::server::ApiState,
    cmd: DbCommand,
) -> Option<String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest { cmd, reply_tx })
        .ok()?;
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
            let fractals: Vec<_> = v
                .fractals
                .into_iter()
                .filter_map(|f| {
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
                })
                .collect();

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
        .query("SELECT position, elevation, rotation, scale FROM node WHERE node_id = $nid AND tombstone = NONE LIMIT 1")
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

/// A node's stored transform: the direct reader (same parse as `GET
/// .../transform`), else a typed `GetNodeTransform` round trip.
pub(crate) async fn load_node_transform(
    state: &crate::server::ApiState,
    node_id: &str,
) -> Result<crate::types::TransformDto, String> {
    if let Some(ref db) = state.db_reader {
        return match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            direct_get_node_transform(db, node_id),
        )
        .await
        {
            Ok(Ok(Some(transform))) => Ok(transform),
            Ok(Ok(None)) => Err("node not found".to_string()),
            Ok(Err(e)) => {
                tracing::warn!(node_id, "node transform read failed: {e}");
                Err("could not load node transform".to_string())
            }
            Err(_) => Err("request timed out".to_string()),
        };
    }
    let cmd = DbCommand::GetNodeTransform {
        node_id: node_id.to_string(),
    };
    match db_round_trip(state, cmd).await? {
        DbResult::NodeTransformLoaded {
            position,
            rotation,
            scale,
            ..
        } => Ok(crate::types::TransformDto {
            position,
            rotation,
            scale,
        }),
        DbResult::Error(e) => {
            tracing::warn!(node_id, "GetNodeTransform failed: {e}");
            Err("could not load node transform".to_string())
        }
        _ => Err("unexpected response".to_string()),
    }
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

    Some(fe_database::build_scope(
        verse_id,
        Some(&fractal_id),
        Some(petal_id),
    ))
}

/// Resolve a fractal's scope string via a direct DB query.
pub(crate) async fn direct_resolve_fractal_scope(db: &Db, fractal_id: &str) -> Option<String> {
    let mut res = db
        .query("SELECT verse_id FROM fractal WHERE fractal_id = $fid LIMIT 1")
        .bind(("fid", fractal_id.to_string()))
        .await
        .ok()?;
    let rows: Vec<serde_json::Value> = res.take(0).ok()?;
    let verse_id = rows.first()?.get("verse_id")?.as_str()?;
    Some(fe_database::build_scope(verse_id, Some(fractal_id), None))
}

/// Resolve a verse's scope string via a direct DB query (`None` if absent).
pub(crate) async fn direct_resolve_verse_scope(db: &Db, verse_id: &str) -> Option<String> {
    let mut res = db
        .query("SELECT verse_id FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await
        .ok()?;
    let rows: Vec<serde_json::Value> = res.take(0).ok()?;
    let verse_id = rows.first()?.get("verse_id")?.as_str()?;
    Some(fe_database::build_scope(verse_id, None, None))
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

/// Parse a JSON value as a `[f32; 3]` array, using `default` for missing elements.
pub(crate) fn parse_f32_array3(val: &serde_json::Value, default: f32) -> [f32; 3] {
    if let Some(arr) = val.as_array() {
        [
            arr.first()
                .and_then(|v| v.as_f64())
                .unwrap_or(default as f64) as f32,
            arr.get(1)
                .and_then(|v| v.as_f64())
                .unwrap_or(default as f64) as f32,
            arr.get(2)
                .and_then(|v| v.as_f64())
                .unwrap_or(default as f64) as f32,
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
/// Security: full guard pipeline in `query_guard` (rate limit 10 req/s/DID,
/// single-SELECT, keyword blocklist, table whitelist, scope injection) plus
/// the FR-4 row cap + response-size ceiling from `limits`.
///
/// M2/F7 distributed mode (A15/A16/A17): a `distributed` spec instead of `sql`
/// fans the structured query out over the verse's fabric and returns the
/// merged rows with honesty metadata. The guard pipeline (role, petal scope
/// resolution + containment, rate limit) lives in
/// `crate::timeseries_query::run_distributed_timeseries_query` — the same
/// guards every other petal-scoped read applies; the merged rows then pass
/// the SAME row cap (error, not truncate) + byte ceiling as the raw path.
pub async fn execute_query(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<crate::types::QueryRequest>,
) -> impl IntoResponse {
    use crate::types::QueryResultDto;

    if let Some(spec) = req.distributed {
        if !req.sql.trim().is_empty() {
            return Json(ApiResponse::<QueryResultDto>::error(
                "sql and distributed are mutually exclusive — send exactly one",
            ));
        }
        return match crate::timeseries_query::run_distributed_timeseries_query(
            &state, &claims, spec,
        )
        .await
        {
            Ok(outcome) => {
                if let Some(reason) = &outcome.error {
                    return Json(ApiResponse::<QueryResultDto>::error(reason));
                }
                // The merged rows pass the same FR-4 caps as the raw path —
                // error, never a silent truncation.
                if outcome.rows.len() > crate::limits::QUERY_ROW_CAP {
                    return Json(ApiResponse::<QueryResultDto>::error(format!(
                        "row cap exceeded (limit {} rows; narrow the window or the petal's shard count)",
                        crate::limits::QUERY_ROW_CAP
                    )));
                }
                if let Err(e) = crate::query_guard::enforce_byte_ceiling(
                    &outcome.rows,
                    crate::limits::QUERY_MAX_RESPONSE_BYTES,
                    crate::limits::QUERY_MAX_RESPONSE_LABEL,
                ) {
                    return Json(ApiResponse::<QueryResultDto>::error(e));
                }
                // FR-5: stamp the egress CRS resolved from the token's scope.
                let crs = crate::crs::scope_crs(&state, &claims.scope).await;
                Json(ApiResponse::success(QueryResultDto {
                    data: outcome.rows,
                    crs: Some(crs),
                    distributed: Some(outcome.meta),
                }))
            }
            Err(e) => Json(ApiResponse::<QueryResultDto>::error(e.message())),
        };
    }

    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<QueryResultDto>::error(
            "insufficient permissions",
        ));
    }

    match run_local_query(&state, &claims, &req.sql, &req.vars).await {
        Ok(dto) => Json(ApiResponse::success(dto)),
        Err(e) => Json(ApiResponse::<QueryResultDto>::error(e)),
    }
}

/// The `/api/v1/query` local-store path, shared with the MCP `query` tool
/// (caller has checked the role): rate limit (per DID) → SELECT-only
/// validation → token-scope filter injection → row cap → byte ceiling → CRS.
pub(crate) async fn run_local_query(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
    sql: &str,
    vars: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<crate::types::QueryResultDto, String> {
    // Rate limit keyed by sub/DID (not jti) so creating multiple tokens doesn't
    // bypass the limit; then static validation + scope-filter injection.
    let guarded = crate::query_guard::guard_and_prepare_query(
        state,
        &claims.sub,
        crate::limits::QUERY_RATE_PER_SEC,
        "10 queries/sec",
        &claims.scope,
        sql,
    )
    .await?;

    let Some(ref db) = state.db_reader else {
        return Err("query endpoint not available (no db_reader)".to_string());
    };

    let data =
        crate::query_guard::run_guarded_query(db, &guarded, vars, crate::limits::QUERY_ROW_CAP)
            .await?;
    crate::query_guard::enforce_byte_ceiling(
        &data,
        crate::limits::QUERY_MAX_RESPONSE_BYTES,
        crate::limits::QUERY_MAX_RESPONSE_LABEL,
    )?;
    // FR-5: stamp the egress CRS resolved from the token's scope.
    let crs = crate::crs::scope_crs(state, &claims.scope).await;
    Ok(crate::types::QueryResultDto {
        data,
        crs: Some(crs),
        distributed: None,
    })
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

    if require_role(&claims, "manager").is_err() {
        return Json(ApiResponse::<QueryResultDto>::error(
            "insufficient permissions (manager+ required)",
        ));
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
                return Json(ApiResponse::<QueryResultDto>::error(
                    "rate limit exceeded (5 elevated queries/sec)",
                ));
            }
        }
    }

    let sql_trimmed = req.sql.trim();
    let sql_upper = sql_trimmed.to_uppercase();

    // Allowed tables for mutations
    const ELEVATED_TABLES: &[&str] = &[
        "NODE",
        "NODE_LOG",
        "VERSE",
        "FRACTAL",
        "PETAL",
        "FIELD_DEF",
        "MODEL",
        "ASSET",
        "ROLE",
        "ROOM",
        "CRATE_REGISTRY",
        "CRATE_ENTRY",
        "VERSE_MEMBER",
    ];

    // Whole-word DDL ban + all-occurrence target whitelist (2026-07-15
    // security review — substring/first-occurrence checks were bypassable).
    if let Err(e) = crate::query_guard::validate_elevated_sql(&sql_upper, ELEVATED_TABLES) {
        return Json(ApiResponse::<QueryResultDto>::error(e));
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

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        query_builder.await
    })
    .await;

    match result {
        Ok(Ok(mut response)) => {
            let mut data = Vec::new();
            let num = response.num_statements();
            for idx in 0..num {
                match response.take::<Vec<serde_json::Value>>(idx) {
                    Ok(rows) => {
                        data.extend(rows);
                    }
                    Err(_) => break,
                }
            }
            Json(ApiResponse::success(QueryResultDto {
                data,
                crs: None,
                distributed: None,
            }))
        }
        Ok(Err(e)) => Json(ApiResponse::<QueryResultDto>::error(format!(
            "query failed: {e}"
        ))),
        Err(_) => Json(ApiResponse::<QueryResultDto>::error(
            "query timed out (10s)",
        )),
    }
}

/// POST /api/v1/analytics/query — execute a DataFusion SQL query over the in-memory EntityStore.
///
/// RBAC: Viewer+ required. A required petal identifier is resolved through
/// the direct DB reader and must be covered by the token scope before the
/// in-memory table is constructed.
/// Rate limited to 10 req/s per user (shared with `/query` limiter).
///
/// M2/F7 (A17): a SQL reference to `iot_reading` serves the MERGED
/// DISTRIBUTED view of the petal's readings (fanned out over the verse's
/// fabric, deduped by `reading_id`, honesty metadata attached) — not just
/// this node's local shards. The table is registered from the fan-out
/// outcome; an empty outcome registers an honest empty table.
///
/// Accepts `{ "sql": "SELECT ...", "petal_id": "required-petal" }`.
/// Returns JSON rows from the DataFusion result.
pub async fn execute_analytics_query(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<crate::types::AnalyticsQueryRequest>,
) -> impl IntoResponse {
    use crate::types::QueryResultDto;

    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<QueryResultDto>::error(
            "insufficient permissions",
        ));
    }

    if !crate::types::is_valid_ulid(&req.petal_id) {
        return Json(ApiResponse::<QueryResultDto>::error("invalid petal_id"));
    }

    // Require direct scope resolution; see fe-api/AGENTS.md §analytics-query.
    let Some(ref db) = state.db_reader else {
        return Json(ApiResponse::<QueryResultDto>::error(
            "analytics authorization unavailable (no direct DB reader)",
        ));
    };
    let Some(scope) = direct_resolve_petal_scope(db, &req.petal_id).await else {
        return Json(ApiResponse::<QueryResultDto>::error(
            "analytics authorization unavailable (petal scope could not be resolved)",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<QueryResultDto>::error("insufficient scope"));
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
                return Json(ApiResponse::<QueryResultDto>::error(
                    "rate limit exceeded (10 analytics queries/sec)",
                ));
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

    // `petal_id` is required and authorized above. The provider sees only this
    // petal, independently of whether client SQL contains a WHERE clause.
    if let Err(e) = analytics_ctx.register_node_table("nodes", Some(&req.petal_id)) {
        return Json(ApiResponse::<QueryResultDto>::error(format!(
            "failed to register node table: {e}"
        )));
    }

    // M2/F7 (A17): a reference to `iot_reading` serves the merged distributed
    // view — the whole petal's fabric (every metric, all time), not just this
    // node's local shards. There is deliberately NO local fallback: a
    // sharded-verse deployment presenting local-only rows as "the readings
    // table" would be the dishonest surface; without the seam the reference
    // fails explicitly instead.
    let mut distributed_meta = None;
    if crate::timeseries_query::sql_references_table(&translated_sql, "iot_reading") {
        let spec = fe_runtime::distributed_query::TsQueryKind::AllReadings {
            petal_id: req.petal_id.clone(),
        };
        let outcome =
            match crate::timeseries_query::run_distributed_timeseries_query(&state, &claims, spec)
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    return Json(ApiResponse::<QueryResultDto>::error(e.message()));
                }
            };
        if let Some(reason) = &outcome.error {
            return Json(ApiResponse::<QueryResultDto>::error(reason));
        }
        if let Err(e) = analytics_ctx.register_json_rows_table("iot_reading", &outcome.rows) {
            return Json(ApiResponse::<QueryResultDto>::error(format!(
                "failed to register readings table: {e}"
            )));
        }
        distributed_meta = Some(outcome.meta);
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        analytics_ctx.execute_to_json(&translated_sql),
    )
    .await
    {
        Ok(Ok(data)) => Json(ApiResponse::success(QueryResultDto {
            data,
            crs: None,
            distributed: distributed_meta,
        })),
        Ok(Err(e)) => Json(ApiResponse::<QueryResultDto>::error(format!(
            "analytics query failed: {e}"
        ))),
        Err(_) => Json(ApiResponse::<QueryResultDto>::error(
            "analytics query timed out (10s)",
        )),
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
    if require_role(&claims, "manager").is_err() {
        return Json(ApiResponse::<FieldDefDto>::error(
            "insufficient permissions",
        ));
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
        Ok(Ok(DbResult::FieldDefCreated {
            field_def_id,
            scope,
            key,
        })) => Json(ApiResponse::success(FieldDefDto {
            field_def_id,
            scope,
            entity_type: req.entity_type,
            key,
            value_type: req.value_type,
            default_val: req.default_val,
            created_by: claims.sub,
            created_at: String::new(),
        })),
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<FieldDefDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<FieldDefDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<FieldDefDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<FieldDefDto>::error("request timed out")),
    }
}

/// GET /api/v1/field-defs/:scope — list field definitions for a scope.
///
/// RBAC: Viewer+ required, and the token must cover the requested scope.
pub async fn list_field_defs(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(scope): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<Vec<FieldDefDto>>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_scope(&scope) || require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<Vec<FieldDefDto>>::error(NOT_FOUND_OR_DENIED));
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::ListFieldDefs {
            scope: scope.clone(),
        },
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<Vec<FieldDefDto>>::error(
            "internal channel closed",
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::FieldDefsListed { field_defs, .. })) => {
            let dtos: Vec<FieldDefDto> = field_defs
                .into_iter()
                .map(|f| FieldDefDto {
                    field_def_id: f.field_def_id,
                    scope: f.scope,
                    entity_type: f.entity_type,
                    key: f.key,
                    value_type: f.value_type,
                    default_val: f.default_val,
                    created_by: f.created_by,
                    created_at: f.created_at,
                })
                .collect();
            Json(ApiResponse::success(dtos))
        }
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<Vec<FieldDefDto>>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<Vec<FieldDefDto>>::error(
            "unexpected response",
        )),
        Ok(Err(_)) => Json(ApiResponse::<Vec<FieldDefDto>>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<Vec<FieldDefDto>>::error("request timed out")),
    }
}

/// True iff the field def exists and the token covers its STORED scope (the
/// URL carries only the id — DEC-C21 sweep: update/delete had no scope check).
async fn field_def_in_scope(
    state: &crate::server::ApiState,
    claims: &ApiClaims,
    field_def_id: &str,
) -> bool {
    let Some(rows) = crate::gis::run_select(
        state,
        "SELECT scope FROM field_def WHERE field_def_id = $fid LIMIT 1",
        vec![("fid".to_string(), serde_json::json!(field_def_id))],
    )
    .await
    else {
        return false;
    };
    rows.first()
        .and_then(|row| row.get("scope"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|scope| require_scope(claims, scope).is_ok())
}

/// PATCH /api/v1/field-defs/:field_def_id — update a field definition.
///
/// RBAC: Manager+ required, at the field def's DB-resolved scope.
pub async fn update_field_def(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(field_def_id): Path<String>,
    Json(req): Json<UpdateFieldDefRequest>,
) -> impl IntoResponse {
    if require_role(&claims, "manager").is_err() {
        return Json(ApiResponse::<FieldDefDto>::error(
            "insufficient permissions",
        ));
    }
    if !field_def_in_scope(&state, &claims, &field_def_id).await {
        return Json(ApiResponse::<FieldDefDto>::error(NOT_FOUND_OR_DENIED));
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
/// RBAC: Manager+ required, at the field def's DB-resolved scope.
pub async fn delete_field_def(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(field_def_id): Path<String>,
) -> impl IntoResponse {
    if require_role(&claims, "manager").is_err() {
        return Json(ApiResponse::<FieldDefDto>::error(
            "insufficient permissions",
        ));
    }
    if !field_def_in_scope(&state, &claims, &field_def_id).await {
        return Json(ApiResponse::<FieldDefDto>::error(NOT_FOUND_OR_DENIED));
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

// ---------------------------------------------------------------------------
// Waypoint CRUD
// ---------------------------------------------------------------------------

/// POST /api/v1/petals/:petal_id/waypoints — create a waypoint node.
///
/// Projects lat/lon/ele to world-space using the terrain projection.
/// RBAC: Editor+ at the petal's DB-resolved scope (F13: the old
/// `build_scope("", None, Some(petal))` panicked on every call).
pub async fn create_waypoint(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Json(req): Json<crate::types::CreateWaypointRequest>,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&petal_id) {
        return Json(ApiResponse::<CreatedEntityDto>::error("invalid petal_id"));
    }
    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return Json(ApiResponse::<CreatedEntityDto>::error(
            "could not resolve petal scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<CreatedEntityDto>::error("insufficient scope"));
    }
    match create_waypoint_core(&state, &petal_id, &req).await {
        Ok(dto) => Json(ApiResponse::success(dto)),
        Err(e) => Json(ApiResponse::<CreatedEntityDto>::error(e)),
    }
}

/// Waypoint-create core shared by REST + MCP `create_waypoint` (caller has
/// authorized the petal): project, create the node, tag its waypoint properties.
pub(crate) async fn create_waypoint_core(
    state: &crate::server::ApiState,
    petal_id: &str,
    req: &crate::types::CreateWaypointRequest,
) -> Result<CreatedEntityDto, String> {
    let position = match load_petal_projection(state, petal_id).await {
        Some(proj) => match proj.wgs84_to_local(req.lat, req.lon, req.ele) {
            Ok([x, y, z]) => [x as f32, y as f32, z as f32],
            Err(e) => return Err(format!("projection failed: {e}")),
        },
        None => {
            tracing::warn!("no terrain config for petal {petal_id}, using identity projection");
            [req.lat as f32, req.ele as f32, req.lon as f32]
        }
    };

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

    let cmd = DbCommand::CreateNode {
        petal_id: petal_id.to_string(),
        name: req.name.clone(),
        position,
        correlation_id: None,
    };
    match db_round_trip(state, cmd).await? {
        DbResult::NodeCreated { id, name, .. } => {
            // Tag the waypoint properties; a failed tag is logged, not fatal.
            for (key, value) in props {
                if let Err(e) = set_property_core(state, &id, key.clone(), value).await {
                    tracing::warn!("waypoint {id} property {key} not set: {e}");
                }
            }
            Ok(CreatedEntityDto { id, name })
        }
        DbResult::Error(e) => {
            tracing::error!("create_waypoint failed: {e}");
            Err("operation failed".to_string())
        }
        _ => Err("unexpected response".to_string()),
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
    if require_role(&claims, "editor").is_err() {
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
    match move_waypoint_core(&state, &waypoint_id, &scope, &req).await {
        Ok(payload) => Json(ApiResponse::<serde_json::Value>::success(payload)),
        Err(e) => Json(ApiResponse::error(e)),
    }
}

/// Waypoint-move core shared by REST + MCP `move_waypoint`. `node_scope` is
/// the caller-authorized, DB-resolved scope; the petal (for the projection)
/// is read from it rather than re-queried.
pub(crate) async fn move_waypoint_core(
    state: &crate::server::ApiState,
    waypoint_id: &str,
    node_scope: &str,
    req: &crate::types::MoveWaypointRequest,
) -> Result<serde_json::Value, String> {
    let Some(petal_id) = fe_database::parse_scope(node_scope)
        .ok()
        .and_then(|parts| parts.petal_id)
    else {
        return Err("could not resolve node petal".to_string());
    };
    let ele = req.ele.unwrap_or(0.0);

    let proj = load_petal_projection(state, &petal_id).await;
    let position = match &proj {
        Some(p) => match p.wgs84_to_local(req.lat, req.lon, ele) {
            Ok([x, y, z]) => [x as f32, y as f32, z as f32],
            Err(_) => [req.lat as f32, ele as f32, req.lon as f32],
        },
        None => [req.lat as f32, ele as f32, req.lon as f32],
    };

    // A move is a reposition only: keep the node's stored rotation/scale
    // (`TransformPersist` writes all three, so they must be read first).
    let current = load_node_transform(state, waypoint_id).await?;

    // Fire-and-forget via `TransformPersist`: the DB thread emits no
    // `DbResult` for a transform write (it broadcasts TransformFailed instead),
    // so registering a waiter would leave a dangling pending request.
    if state
        .api_cmd_tx
        .send(ApiCommand::TransformPersist {
            node_id: waypoint_id.to_string(),
            position,
            rotation: current.rotation,
            scale: current.scale,
        })
        .is_err()
    {
        tracing::warn!(waypoint_id, "TransformPersist send failed (DB bridge gone)");
    }

    for (key, value) in [
        ("lat", serde_json::json!(req.lat)),
        ("lon", serde_json::json!(req.lon)),
        ("ele", serde_json::json!(ele)),
    ] {
        if let Err(e) = set_property_core(state, waypoint_id, key.to_string(), value).await {
            tracing::warn!("waypoint {waypoint_id} property {key} not updated: {e}");
        }
    }

    Ok(serde_json::json!({
        "node_id": waypoint_id,
        "lat": req.lat,
        "lon": req.lon,
        "ele": ele,
    }))
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

    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&track_id) {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error(
            "invalid track_id",
        ));
    }

    // Resolve node scope
    let Some(scope) = resolve_node_scope(&state, &track_id).await else {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error(
            "could not resolve node scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return Json(ApiResponse::<Vec<ElevationPointDto>>::error(
            "insufficient scope",
        ));
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
                return Json(ApiResponse::<Vec<ElevationPointDto>>::error(format!(
                    "query failed: {e}"
                )))
            }
            Err(_) => {
                return Json(ApiResponse::<Vec<ElevationPointDto>>::error(
                    "request timed out",
                ))
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

    if require_role(&claims, "viewer").is_err() {
        return Json(ApiResponse::<TrackStatsDto>::error(
            "insufficient permissions",
        ));
    }
    if !is_valid_ulid(&track_id) {
        return Json(ApiResponse::<TrackStatsDto>::error("invalid track_id"));
    }

    let Some(scope) = resolve_node_scope(&state, &track_id).await else {
        return Json(ApiResponse::<TrackStatsDto>::error(
            "could not resolve node scope",
        ));
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
                return Json(ApiResponse::<TrackStatsDto>::error(format!(
                    "query failed: {e}"
                )))
            }
            Err(_) => return Json(ApiResponse::<TrackStatsDto>::error("request timed out")),
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
    state: &crate::server::ApiState,
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
            let ele = properties
                .get("origin_ele")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            Some(fe_terrain::projection::Projection::new(lat, lon, ele))
        }
        _ => None,
    }
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
        min_elevation_m: if min_elevation == f64::MAX {
            0.0
        } else {
            min_elevation
        },
        max_elevation_m: if max_elevation == f64::MIN {
            0.0
        } else {
            max_elevation
        },
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
    let Some(route_id) = tracking_route_id else {
        return;
    };

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
    let Some(points) = route_points_json else {
        return;
    };

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

    if route_pts.len() < 2 {
        return;
    }

    let tracker = fe_terrain::iot::PathTracker::new(route_pts);

    // Convert position to [f64; 3] for snap
    let pos = [position[0] as f64, position[1] as f64, position[2] as f64];
    let snap = tracker.snap_to_route(pos);

    // Emit route progress properties via SceneChange::PropertyChanged
    let metrics = [
        ("route_progress", serde_json::json!(snap.route_progress)),
        (
            "distance_remaining_m",
            serde_json::json!(snap.distance_remaining_m),
        ),
        ("deviation_m", serde_json::json!(snap.deviation_m)),
        (
            "nearest_segment_index",
            serde_json::json!(snap.segment_index),
        ),
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
            node_id,
            deviation_m = snap.deviation_m,
            threshold,
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
    use crate::server::ApiState;
    use crate::types::QueryRequest;
    use axum::extract::State;
    use axum::response::IntoResponse;
    use axum::Extension;
    use axum::Json;
    use fe_identity::api_token::ApiClaims;
    use std::collections::HashSet;
    use std::sync::Arc;

    async fn setup_test_db() -> surrealdb::Surreal<surrealdb::engine::local::Db> {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        fe_database::schema::apply_all(&db)
            .await
            .expect("apply schema");
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
        db.query(
            "CREATE asset CONTENT {
            asset_id: 'asset-1',
            name: 'test-model',
            content_type: 'model/gltf-binary',
            size_bytes: 1024,
            created_at: $now,
            content_hash: 'abc123hash'
        }",
        )
        .bind(("now", serde_json::json!(now)))
        .await
        .unwrap()
        .check()
        .unwrap();
        println!("DEBUG: Asset record created");

        println!("DEBUG: Seeding verse/fractal/petal hierarchy for scope row-filter");
        db.query(
            "CREATE verse CONTENT {
            verse_id: 'v1', name: 'V', created_by: 'did:key:z6MkOwner', created_at: $now
        }",
        )
        .bind(("now", serde_json::json!(now)))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(
            "CREATE fractal CONTENT {
            fractal_id: 'f1', verse_id: 'v1', owner_did: 'did:key:z6MkOwner', name: 'F', created_at: $now
        }",
        )
        .bind(("now", serde_json::json!(now)))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(
            "CREATE petal CONTENT {
            petal_id: 'petal-1', fractal_id: 'f1', name: 'P', node_id: 'anchor-node', created_at: $now
        }",
        )
        .bind(("now", serde_json::json!(now)))
        .await
        .unwrap()
        .check()
        .unwrap();
        println!("DEBUG: Hierarchy seeded");

        println!("DEBUG: Creating crate_registry record");
        db.query(
            "CREATE crate_registry CONTENT {
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
        }",
        )
        .bind(("now", serde_json::json!(now)))
        .await
        .unwrap()
        .check()
        .unwrap();
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
            replication_tx: None,
            distributed_tx: None,
            sim_control_tx: None,
            share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
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

        // 1. The asset table is NOT queryable (DEC-C21: removed from the
        // read whitelist — asset rows carry no owner/scope column).
        let req = QueryRequest {
            sql: "SELECT * FROM asset".to_string(),
            vars: std::collections::HashMap::new(),
            distributed: None,
        };
        let res = execute_query(State(state.clone()), Extension(claims.clone()), Json(req)).await;
        let response = res.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!body_json["ok"].as_bool().unwrap_or(true), "{body_json}");
        let err = body_json["error"].as_str().unwrap_or_default();
        assert!(err.contains("'ASSET' are not allowed"), "{err}");
        assert!(
            !body_json.to_string().contains("abc123hash"),
            "no asset row leaks"
        );

        // 2. Query crate_registry
        let req = QueryRequest {
            sql: "SELECT * FROM crate_registry".to_string(),
            vars: std::collections::HashMap::new(),
            distributed: None,
        };
        println!("DEBUG: Executing second query");
        let res = execute_query(State(state.clone()), Extension(claims.clone()), Json(req)).await;
        println!("DEBUG: Second query returned");
        let response = res.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        println!("DEBUG: Reading response body for second query");
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(
            body_json["ok"].as_bool().unwrap_or(false),
            "query failed: {:?}",
            body_json
        );
        let data = body_json["data"]["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["hexon_uri"], "hexon://test-uri");

        // 3. Blocked query (mutation keyword)
        let req = QueryRequest {
            sql: "DELETE FROM asset WHERE asset_id = 'asset-1'".to_string(),
            vars: std::collections::HashMap::new(),
            distributed: None,
        };
        println!("DEBUG: Executing third query (blocked)");
        let res = execute_query(State(state.clone()), Extension(claims.clone()), Json(req)).await;
        println!("DEBUG: Third query returned");
        let response = res.into_response();
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!body_json["ok"].as_bool().unwrap_or(false));
        // `DELETE …` is rejected by the SELECT-only guard ("only SELECT
        // statements are allowed"); the blocked-keyword path says "… is not
        // allowed in queries". Both contain "allowed", so match on that.
        assert!(body_json["error"].as_str().unwrap().contains("allowed"));
    }
}
