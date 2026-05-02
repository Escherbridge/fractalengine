use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, TransformUpdate};

/// Shared state injected into every axum handler via `State<Arc<ApiState>>`.
pub struct ApiState {
    pub api_cmd_tx: crossbeam::channel::Sender<ApiCommand>,
    pub transform_broadcast_tx: tokio::sync::broadcast::Sender<TransformUpdate>,
    /// Entity change broadcast for scene graph streaming (CUD deltas).
    pub entity_change_tx: tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>,
    pub verifying_key: ed25519_dalek::VerifyingKey,
    /// Cache of revoked token JTIs. Checked by auth middleware.
    pub revoked_jtis: Arc<tokio::sync::RwLock<HashSet<String>>>,
    /// Content-addressed blob store for asset delivery. `None` if not configured.
    pub blob_store: Option<fe_runtime::blob_store::BlobStoreHandle>,
    /// Allowed CORS origins. Use `["*"]` to allow any origin.
    pub cors_origins: Vec<String>,
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
///   Assets:
///   GET   /api/v1/assets/:content_hash
///
///   MCP:
///   POST /mcp
/// ```
pub fn build_router(state: Arc<ApiState>) -> Router {
    let cors = if state.cors_origins.iter().any(|o| o == "*") {
        CorsLayer::new()
            .allow_origin(AllowOrigin::any())
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
            .allow_headers(tower_http::cors::Any)
    } else {
        let origins: Vec<HeaderValue> = state
            .cors_origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
            .allow_headers(tower_http::cors::Any)
    };

    // Public routes — no auth required.
    let public = Router::new()
        .route(
            "/api/v1/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        .route("/ready", get(ready_handler))
        // WebSocket does its own auth after the upgrade handshake.
        .route("/ws", get(crate::ws::ws_handler));

    // Authenticated routes — Bearer JWT required.
    let authenticated = Router::new()
        // Global
        .route("/api/v1/hierarchy", get(crate::rest::get_hierarchy))
        .route("/api/v1/verses", post(crate::rest::create_verse))
        // Verse-scoped
        .route(
            "/api/v1/verses/{verse_id}/fractals",
            post(crate::rest::create_fractal),
        )
        // Fractal-scoped
        .route(
            "/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals",
            post(crate::rest::create_petal),
        )
        // Petal-scoped
        .route(
            "/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals/{petal_id}/nodes",
            post(crate::rest::create_node),
        )
        // Node operations
        .route(
            "/api/v1/nodes/{node_id}/transform",
            patch(crate::rest::update_transform).get(crate::rest::get_transform),
        )
        // Asset delivery (content-addressed)
        .route(
            "/api/v1/assets/{content_hash}",
            get(crate::assets::get_asset),
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

/// Readiness probe: sends a `DbCommand::Ping` through the API command channel
/// and waits up to 2 seconds for a `DbResult::Pong` response.
async fn ready_handler(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::Ping,
            reply_tx,
        })
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "not_ready" })),
        );
    }
    match tokio::time::timeout(std::time::Duration::from_secs(2), reply_rx).await {
        Ok(Ok(DbResult::Pong)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ready" })),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "not_ready" })),
        ),
    }
}

