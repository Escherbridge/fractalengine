use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use fe_runtime::messages::{ApiCommand, TransformUpdate};

/// Shared state injected into every axum handler via `State<Arc<ApiState>>`.
pub struct ApiState {
    pub api_cmd_tx: crossbeam::channel::Sender<ApiCommand>,
    pub transform_broadcast_tx: tokio::sync::broadcast::Sender<TransformUpdate>,
    pub verifying_key: ed25519_dalek::VerifyingKey,
    /// Cache of revoked token JTIs. Checked by auth middleware.
    pub revoked_jtis: Arc<tokio::sync::RwLock<HashSet<String>>>,
}

/// Build the complete axum [`Router`] for the API server.
///
/// # Route hierarchy
///
/// ```text
/// PUBLIC (no auth):
///   GET  /api/v1/health
///   GET  /ws
///
/// AUTHENTICATED (Bearer JWT):
///   GET  /api/v1/hierarchy
///   POST /api/v1/verses
///
///   Verse-scoped:
///   POST /api/v1/verses/:verse_id/fractals
///
///   Fractal-scoped:
///   POST /api/v1/verses/:verse_id/fractals/:fractal_id/petals
///
///   Petal-scoped:
///   POST /api/v1/verses/:verse_id/fractals/:fractal_id/petals/:petal_id/nodes
///
///   Node operations (scope resolved from node's parent chain):
///   PATCH /api/v1/nodes/:node_id/transform
///   GET   /api/v1/nodes/:node_id/transform
///
///   MCP:
///   POST /mcp
/// ```
pub fn build_router(state: Arc<ApiState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            "http://localhost:8765".parse::<HeaderValue>().unwrap(),
            "http://127.0.0.1:8765".parse::<HeaderValue>().unwrap(),
        ]))
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
        .allow_headers(tower_http::cors::Any);

    // Public routes — no auth required.
    let public = Router::new()
        .route(
            "/api/v1/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        // WebSocket does its own auth after the upgrade handshake.
        .route("/ws", get(crate::ws::ws_handler));

    // Authenticated routes — Bearer JWT required.
    let authenticated = Router::new()
        // Global
        .route("/api/v1/hierarchy", get(crate::rest::get_hierarchy))
        .route("/api/v1/verses", post(crate::rest::create_verse))
        // Verse-scoped
        .route(
            "/api/v1/verses/:verse_id/fractals",
            post(crate::rest::create_fractal),
        )
        // Fractal-scoped
        .route(
            "/api/v1/verses/:verse_id/fractals/:fractal_id/petals",
            post(crate::rest::create_petal),
        )
        // Petal-scoped
        .route(
            "/api/v1/verses/:verse_id/fractals/:fractal_id/petals/:petal_id/nodes",
            post(crate::rest::create_node),
        )
        // Node operations (legacy flat path kept for backwards compat)
        .route(
            "/api/v1/nodes/:node_id/transform",
            patch(crate::rest::update_transform),
        )
        .route(
            "/api/v1/nodes/:node_id/transform",
            get(get_transform_placeholder),
        )
        // Legacy flat create endpoints (for MCP and existing integrations)
        .route("/api/v1/nodes", post(crate::rest::create_node_legacy))
        // MCP
        .route("/mcp", post(crate::mcp::mcp_handler))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::auth_middleware,
        ));

    public
        .merge(authenticated)
        .layer(cors)
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
