use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{delete, get, patch, post},
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
    /// Read-only SurrealDB connection for direct API queries.
    /// When `Some`, read handlers bypass the crossbeam→DB-thread round-trip.
    pub db_reader: Option<std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>>,
    /// Per-token query rate limiter (jti → (count, window_start)).
    pub query_rate_limiter:
        tokio::sync::Mutex<std::collections::HashMap<String, (u32, std::time::Instant)>>,
    /// In-memory entity store for DataFusion analytics queries.
    pub entity_store: Option<std::sync::Arc<fe_entity_store::EntityStore>>,
    /// Tileset registry for hexon tile serving and management.
    pub tileset_registry: Option<std::sync::Arc<fe_terrain::tiles::TilesetRegistry>>,
    /// Hexon crate registry for package install/uninstall.
    pub hexon_registry: Option<std::sync::Arc<std::sync::Mutex<fe_hexon::registry::HexonRegistry>>>,
    /// P2P announcement store for peer-discovered crates.
    pub announcement_store:
        Option<std::sync::Arc<std::sync::Mutex<fe_hexon::p2p::AnnouncementStore>>>,
    /// Ed25519 keypair signing shareable query URLs (see AGENTS.md §share).
    pub share_signer: Arc<fe_identity::NodeKeypair>,
}

/// Build the complete axum [`Router`] (route inventory: fe-api/AGENTS.md §routes).
pub fn build_router(state: Arc<ApiState>) -> Router {
    let cors = if state.cors_origins.iter().any(|o| o == "*") {
        CorsLayer::new()
            .allow_origin(AllowOrigin::any())
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(tower_http::cors::Any)
    } else {
        let origins: Vec<HeaderValue> = state
            .cors_origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
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
        .route("/ws", get(crate::ws::ws_handler))
        // Shared-URL redemption: no session — the ed25519 signature IS the
        // credential and the token's scope ceiling bounds the result set.
        .route(
            "/api/v1/shared/{token}",
            get(crate::share::redeem_share_url),
        );

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
        // Per-endpoint read/write surface (endpoint_api_surface_20260725, T5)
        // Read an object's full payload (FR-2); sync-safe tombstone delete (FR-3).
        .route(
            "/api/v1/nodes/{node_id}",
            get(crate::endpoint::get_node).delete(crate::endpoint::delete_node),
        )
        // Resolve a node id → its stable fe:// endpoint URI (FR-1).
        .route(
            "/api/v1/nodes/{node_id}/address",
            get(crate::endpoint::get_node_address),
        )
        // Parse a fe:// URI back into its components (FR-1 inverse).
        .route("/api/v1/address", get(crate::endpoint::resolve_address))
        // List live nodes of a type tag through the generic abstraction (FR-6).
        .route(
            "/api/v1/petals/{petal_id}/nodes",
            get(crate::endpoint::list_nodes_by_kind),
        )
        // Aggregate real-unit cut/fill across earthwork regions (FR-6).
        .route(
            "/api/v1/petals/{petal_id}/earthwork/summary",
            get(crate::endpoint::earthwork_summary),
        )
        // Lazily promote a stamp instance to a full addressable node (FR-3/FR-6).
        .route(
            "/api/v1/petals/{petal_id}/paths/{path_id}/instances/{instance_index}/promote",
            post(crate::endpoint::promote_instance),
        )
        // Node operations
        .route(
            "/api/v1/nodes/{node_id}/transform",
            patch(crate::rest::update_transform).get(crate::rest::get_transform),
        )
        // Node properties
        .route(
            "/api/v1/nodes/{node_id}/properties",
            patch(crate::rest::set_node_property).get(crate::rest::get_node_properties),
        )
        .route(
            "/api/v1/nodes/{node_id}/properties/{key}",
            delete(crate::rest::delete_node_property),
        )
        // Asset delivery (content-addressed)
        .route(
            "/api/v1/assets/{content_hash}",
            get(crate::assets::get_asset),
        )
        // Asset delivery (by asset_id -- real content-type/name/length headers)
        .route(
            "/api/v1/assets/by-id/{asset_id}",
            get(crate::assets::get_asset_by_id),
        )
        // Node asset download (node -> asset -> blob, real headers)
        .route(
            "/api/v1/nodes/{node_id}/asset",
            get(crate::assets::get_node_asset),
        )
        // Petal export/import (.hexon archive)
        .route(
            "/api/v1/petals/{petal_id}/export",
            get(crate::format::export_petal),
        )
        .route(
            "/api/v1/petals/{petal_id}/import",
            post(crate::format::import_petal),
        )
        // GPX import/export
        .route(
            "/api/v1/petals/{petal_id}/import/gpx",
            post(crate::gpx::import_gpx),
        )
        .route(
            "/api/v1/petals/{petal_id}/export/gpx",
            get(crate::gpx::export_gpx),
        )
        // Petal terrain config
        .route(
            "/api/v1/petals/{petal_id}/terrain",
            get(crate::terrain::get_terrain_config)
                .put(crate::terrain::set_terrain_config)
                .delete(crate::terrain::delete_terrain_config),
        )
        // Petal-scoped tile data plane
        .route(
            "/api/v1/tiles/elevation/{tileset_id}/{z}/{x}/{y_png}",
            get(crate::terrain::get_elevation_tile),
        )
        .route(
            "/api/v1/tiles/satellite/{tileset_id}/{z}/{x}/{y_jpg}",
            get(crate::terrain::get_satellite_tile),
        )
        .route(
            "/api/v1/tilesets",
            get(crate::terrain::list_available_tilesets),
        )
        .route(
            "/api/v1/tilesets/{tileset_id}/meta",
            get(crate::terrain::get_tileset_meta),
        )
        // Petal GIS reads (geo nodes + GPX tracks)
        .route(
            "/api/v1/petals/{petal_id}/gis/nodes",
            get(crate::gis::list_gis_nodes),
        )
        .route(
            "/api/v1/petals/{petal_id}/gis/tracks",
            get(crate::gis::list_gis_tracks),
        )
        // Waypoint CRUD
        .route(
            "/api/v1/petals/{petal_id}/waypoints",
            post(crate::rest::create_waypoint),
        )
        .route(
            "/api/v1/nodes/{waypoint_id}/move",
            patch(crate::rest::move_waypoint),
        )
        // Track profiles and stats
        .route(
            "/api/v1/nodes/{track_id}/elevation-profile",
            get(crate::rest::get_elevation_profile),
        )
        .route(
            "/api/v1/nodes/{track_id}/stats",
            get(crate::rest::get_track_stats),
        )
        // Field definition (property schema) management
        .route("/api/v1/field-defs", post(crate::rest::create_field_def))
        .route(
            "/api/v1/field-defs/{scope}",
            get(crate::rest::list_field_defs),
        )
        .route(
            "/api/v1/field-defs/by-id/{field_def_id}",
            patch(crate::rest::update_field_def).delete(crate::rest::delete_field_def),
        )
        // Legacy flat create endpoints (for MCP and existing integrations)
        .route("/api/v1/nodes", post(crate::rest::create_node_legacy))
        // Query endpoints (scope-guarded SurrealQL)
        .route("/api/v1/query", post(crate::rest::execute_query))
        .route(
            "/api/v1/query/elevated",
            post(crate::rest::execute_elevated_query),
        )
        // BI egress: parquet/CSV downloads (same guard pipeline as /query)
        .route(
            "/api/v1/petals/{petal_id}/export.parquet",
            get(crate::export::export_parquet),
        )
        .route(
            "/api/v1/petals/{petal_id}/export.csv",
            get(crate::export::export_csv),
        )
        // IoT reading ingestion (batch, petal-scoped)
        .route(
            "/api/v1/petals/{petal_id}/iot/readings",
            post(crate::iot::ingest_readings),
        )
        // Shareable signed query URLs (mint; redemption is public)
        .route("/api/v1/query/share", post(crate::share::issue_share_url))
        // Analytics (DataFusion columnar queries over EntityStore)
        .route(
            "/api/v1/analytics/query",
            post(crate::rest::execute_analytics_query),
        )
        // Hexon tileset management
        .route(
            "/api/v1/hexons/tilesets/install",
            post(crate::terrain::install_hexon_tileset),
        )
        .route(
            "/api/v1/hexons/tilesets/{hexon_id}",
            delete(crate::terrain::remove_hexon_tileset),
        )
        .route(
            "/api/v1/hexons/tilesets/{hexon_id}/seeding",
            patch(crate::terrain::toggle_seeding),
        )
        .route(
            "/api/v1/hexons/tilesets",
            get(crate::terrain::list_installed_hexons),
        )
        .route(
            "/api/v1/hexons/storage",
            get(crate::terrain::get_storage_info),
        )
        // Hexon crate registry (Phase 8)
        .route("/api/v1/crates/publish", post(crate::hexon::publish_crate))
        .route(
            "/api/v1/crates/{hexon_uri}/install",
            post(crate::hexon::install_crate),
        )
        .route(
            "/api/v1/crates/{hexon_uri}/uninstall",
            delete(crate::hexon::uninstall_crate),
        )
        .route("/api/v1/crates/search", get(crate::hexon::search_crates))
        .route(
            "/api/v1/crates/installed",
            get(crate::hexon::list_installed),
        )
        .route(
            "/api/v1/crates/{hexon_uri}",
            get(crate::hexon::get_crate_manifest),
        )
        .route(
            "/api/v1/crates/{hexon_uri}/entries",
            get(crate::hexon::get_crate_entries),
        )
        .route(
            "/api/v1/crates/{hexon_uri}/entries/{entry_id}/asset",
            get(crate::hexon::get_crate_asset),
        )
        .route(
            "/api/v1/crates/available",
            get(crate::hexon::available_crates),
        )
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

/// Readiness probe: pings the database and returns 200 if responsive.
///
/// When a direct `db_reader` is available, bypasses the crossbeam channel and
/// queries SurrealDB directly. Falls back to the channel-based
/// `DbCommand::Ping` → `DbResult::Pong` round-trip otherwise.
async fn ready_handler(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    if let Some(ref db) = state.db_reader {
        // Direct DB ping — bypass the crossbeam channel
        match tokio::time::timeout(std::time::Duration::from_secs(2), db.query("RETURN true")).await
        {
            Ok(Ok(_)) => (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "ready" })),
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "not_ready" })),
            ),
        }
    } else {
        // Fallback: channel-based ping
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
}
