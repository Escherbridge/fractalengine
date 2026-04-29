use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use fe_runtime::messages::{ApiCommand, TransformUpdate};

/// Shared state injected into every axum handler via `State<Arc<ApiState>>`.
pub struct ApiState {
    pub api_cmd_tx: crossbeam::channel::Sender<ApiCommand>,
    pub transform_broadcast_tx: tokio::sync::broadcast::Sender<TransformUpdate>,
}

/// Build the complete axum [`Router`] for the API server.
pub fn build_router(state: Arc<ApiState>) -> Router {
    Router::new()
        // Health
        .route(
            "/api/v1/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        // Hierarchy
        .route("/api/v1/hierarchy", get(crate::rest::get_hierarchy))
        // Verses
        .route("/api/v1/verses", post(crate::rest::create_verse))
        // Nodes
        .route("/api/v1/nodes", post(crate::rest::create_node))
        .route(
            "/api/v1/nodes/:node_id/transform",
            patch(crate::rest::update_transform),
        )
        .route(
            "/api/v1/nodes/:node_id/transform",
            get(get_transform_placeholder),
        )
        // MCP (JSON-RPC 2.0 over HTTP POST)
        .route("/mcp", post(crate::mcp::mcp_handler))
        // WebSocket
        .route("/ws", get(crate::ws::ws_handler))
        // Middleware
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Placeholder for the read-transform endpoint (501 Not Implemented).
async fn get_transform_placeholder(
    State(_state): State<Arc<ApiState>>,
    Path(_node_id): Path<String>,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}
