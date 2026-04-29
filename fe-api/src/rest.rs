use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, Path, State};
use axum::response::IntoResponse;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, TransformUpdate};

use crate::types::{
    ApiResponse, CreatedEntityDto, UpdateTransformRequest,
    CreateNodeRequest, CreateVerseRequest, VerseDto,
    hierarchy_to_dto,
};

// ---------------------------------------------------------------------------
// Helper
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

/// GET /api/hierarchy — full verse → fractal → petal → node snapshot.
pub async fn get_hierarchy(
    State(state): State<Arc<crate::server::ApiState>>,
) -> impl IntoResponse {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::GetHierarchy { reply_tx };
    if state.api_cmd_tx.send(cmd).is_err() {
        return Json(ApiResponse::<Vec<VerseDto>>::error("internal channel closed"));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(data)) => Json(ApiResponse::success(hierarchy_to_dto(&data))),
        Ok(Err(_)) => Json(ApiResponse::<Vec<VerseDto>>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<Vec<VerseDto>>::error("request timed out")),
    }
}

/// POST /api/verses — create a new verse.
pub async fn create_verse(
    State(state): State<Arc<crate::server::ApiState>>,
    Json(req): Json<CreateVerseRequest>,
) -> impl IntoResponse {
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
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<CreatedEntityDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// POST /api/nodes — create a new node inside a petal.
pub async fn create_node(
    State(state): State<Arc<crate::server::ApiState>>,
    Json(req): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    let position = req.position.unwrap_or([0.0, 0.0, 0.0]);
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: DbCommand::CreateNode {
            petal_id: req.petal_id,
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
        Ok(Ok(DbResult::Error(e))) => Json(ApiResponse::<CreatedEntityDto>::error(e)),
        Ok(Ok(_)) => Json(ApiResponse::<CreatedEntityDto>::error("unexpected response")),
        Ok(Err(_)) => Json(ApiResponse::<CreatedEntityDto>::error("request cancelled")),
        Err(_) => Json(ApiResponse::<CreatedEntityDto>::error("request timed out")),
    }
}

/// PATCH /api/nodes/:node_id/transform — update position/rotation/scale.
///
/// Persists via DB and also broadcasts on the real-time transform channel so
/// WebSocket subscribers receive the update without a DB round-trip.
pub async fn update_transform(
    State(state): State<Arc<crate::server::ApiState>>,
    Path(node_id): Path<String>,
    Json(req): Json<UpdateTransformRequest>,
) -> impl IntoResponse {
    // Fire-and-forget DB persist — we don't wait for the result.
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    let _ = state.api_cmd_tx.send(ApiCommand::DbRequest {
        cmd: DbCommand::UpdateNodeTransform {
            node_id: node_id.clone(),
            position: req.position,
            rotation: req.rotation,
            scale: req.scale,
        },
        reply_tx,
    });

    // Broadcast real-time update to WebSocket subscribers.
    let _ = state.transform_broadcast_tx.send(TransformUpdate {
        node_id: node_id.clone(),
        petal_id: String::new(), // caller doesn't know petal_id for now
        position: req.position,
        rotation: req.rotation,
        scale: req.scale,
        timestamp_ms: now_ms(),
        source_did: String::new(), // no auth yet
    });

    Json(ApiResponse::success(serde_json::json!({ "node_id": node_id })))
}
