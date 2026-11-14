use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Extension, Json, Router,
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, TransformUpdate};

/// Shared state injected into every axum handler via `State<Arc<ApiState>>`.
pub struct ApiState {
    pub terrain_capture: fe_runtime::terrain_grant::TerrainCaptureLimiter,
    pub api_cmd_tx: crossbeam::channel::Sender<ApiCommand>,
    pub transform_broadcast_tx: tokio::sync::broadcast::Sender<TransformUpdate>,
    /// Entity change broadcast for scene graph streaming (CUD deltas).
    pub entity_change_tx: tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>,
    pub verifying_key: ed25519_dalek::VerifyingKey,
    /// Revocation hints; configured stores are also checked durably at every use.
    pub revoked_jtis: Arc<tokio::sync::RwLock<HashSet<String>>>,
    /// Content-addressed blob store for asset delivery. `None` if not configured.
    pub blob_store: Option<fe_runtime::blob_store::BlobStoreHandle>,
    /// Allowed CORS origins. Use `["*"]` to allow any origin.
    pub cors_origins: Vec<String>,
    /// Read-only SurrealDB connection for direct API queries (guarded facade,
    /// see `fe-api/AGENTS.md` §db-reader). When `Some`, read handlers bypass
    /// the crossbeam→DB-thread round-trip.
    pub db_reader: Option<std::sync::Arc<crate::db_reader::ReadDb>>,
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
    /// Replication emit seam: the DB→sync bridge's event channel (A11).
    ///
    /// The API ingestion path (IoT readings) writes directly through
    /// `db_reader`'s `writable()` escape hatch (the §iot-readings append-only
    /// exception), so it needs its own way to publish rows to peers.
    /// Handlers that own a `ReplicationEvent` push it
    /// here; `None` means the API thread is running without a replication
    /// bridge (harness/tests) and emission is skipped.
    pub replication_tx: Option<fe_database::ReplicationSender>,
    /// Distributed-query fan-out seam (M2/F7 — A17): the API→sync bridge's
    /// call sender (`fe_sync::distributed_query::bridge_distributed_queries`
    /// bridges it into `SyncCommand::SubmitComputeTask`). `None` = no
    /// sync-thread seam (harness/tests without a sync side) — the
    /// distributed surfaces return an explicit unavailable error.
    pub distributed_tx: Option<fe_runtime::distributed_query::DistributedQueryCallSender>,
    /// Sim control seam (F9/A20): the fe-sim bridge's call sender. `None` =
    /// not a lab host — every `/api/v1/sim/*` call fails closed (§sim-control).
    pub sim_control_tx: Option<fe_runtime::sim_control::SimControlCallSender>,
    /// Ed25519 keypair signing shareable query URLs (see AGENTS.md §share).
    pub share_signer: Arc<fe_identity::NodeKeypair>,
    /// Operator opt-in for `/api/v1/query/elevated` (env `FE_API_ELEVATED_ALLOW=1`,
    /// read once at startup); `false` = the endpoint refuses every call (§elevated-query).
    pub elevated_query_allowed: bool,
    /// Per-owner cap on registered asset bytes (env `FE_API_UPLOAD_QUOTA_BYTES`,
    /// read once at startup) — AGENTS.md §asset-ingest.
    pub upload_quota_bytes: u64,
    /// Agent screenshot + vision surface: operator opt-in (ON unless `=0`) +
    /// API→Bevy capture bridge + dynamic-capture/camera-read-back bridge
    /// (AGENTS.md §screenshot, §camera-state). `ScreenshotSurface::disabled()`
    /// on headless hosts.
    pub screenshot: crate::screenshot::ScreenshotSurface,
    /// Agent input-injection surface (folded-in `agent_input_surface_20261031`
    /// via agent_vision_20261031): operator opt-in (OFF unless
    /// `FE_API_INPUT_ALLOW=1` — this surface MUTATES state, unlike
    /// screenshot/camera's default-on posture) + its API→Bevy synthetic-input
    /// bridge (AGENTS.md §agent-input-surface). `InputSurface::disabled()`
    /// on headless hosts and every test `ApiState` that does not exercise
    /// this surface.
    pub input: crate::input::InputSurface,
}

/// Optional host seams the binary provides (fe-api/AGENTS.md §extensions); kept out of [`ApiState`] so no
/// state constructor changes. Reaches `/mcp` and `/ws` as an axum `Extension`.
#[derive(Clone, Default)]
pub struct ApiHosts {
    pub extension: Option<Arc<dyn crate::extensions::ExtensionHost>>,
    pub binding: Option<Arc<dyn crate::bindings::BindingHost>>,
}

/// Build the complete axum [`Router`] with no extension or binding host (route inventory: fe-api/AGENTS.md §routes).
pub fn build_router(state: Arc<ApiState>) -> Router {
    build_router_with_hosts(state, ApiHosts::default())
}

/// [`build_router`] plus the plugin-host routes when the hosts are provided.
pub fn build_router_with_hosts(state: Arc<ApiState>, hosts: ApiHosts) -> Router {
    let cors = if state.cors_origins.iter().any(|o| o == "*") {
        CorsLayer::new()
            .allow_origin(AllowOrigin::any())
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::PUT,
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
            // Configured origins, plus static portal origins on the share route only (portal_bundles).
            .allow_origin(AllowOrigin::predicate(move |origin, parts| {
                origins.contains(origin) || crate::portal_bundles::share_route_allows(origin, parts)
            }))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::PUT,
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
            "/api/v1/petals/{petal_id}/objects",
            post(crate::node_history::create),
        )
        .route(
            "/api/v1/nodes/{node_id}/history",
            get(crate::node_history::history),
        )
        .route(
            "/api/v1/nodes/{node_id}/state",
            get(crate::node_history::state_at),
        )
        .route(
            "/api/v1/nodes/{node_id}/mutations",
            post(crate::node_history::mutate),
        )
        .route(
            "/api/v1/nodes/{node_id}/transform",
            patch(crate::rest::update_transform).get(crate::rest::get_transform),
        )
        // Node properties
        .route(
            "/api/v1/field-defs/by-id/{field_def_id}/schema",
            get(crate::property_schema::get).put(crate::property_schema::put),
        )
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
        // GLB upload (FR-1; petal = authz anchor; per-route body limit — §asset-ingest)
        .route(
            "/api/v1/petals/{petal_id}/assets",
            post(crate::upload::upload_petal_asset)
                .layer(DefaultBodyLimit::max(crate::upload::ASSET_ROUTE_BODY_LIMIT)),
        )
        // Portal bundle upload (multipart; cap + framing) and summary read
        .route(
            "/api/v1/petals/{petal_id}/portal-bundles",
            post(crate::portal_bundles::upload_portal_bundle)
                .layer(DefaultBodyLimit::max(crate::portal_bundles::route_body_limit())),
        )
        .route(
            "/api/v1/portal-bundles/{asset_id}",
            get(crate::portal_bundles::get_portal_bundle),
        )
        // Static-directory export of a repository node's portal bundle (Viewer+, reach-checked)
        .route(
            "/api/v1/nodes/{node_id}/portal/export.zip",
            get(crate::portal_bundles::export_portal_bundle),
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
        // Petal render distance (p2p_asset_streaming_20260718 S6)
        .route(
            "/api/v1/petals/{petal_id}/render-distance",
            get(crate::render_distance::get_render_distance)
                .put(crate::render_distance::set_render_distance)
                .delete(crate::render_distance::delete_render_distance),
        )
        // Petal visibility-Group registry (VIS-PH6, AGENTS.md §visibility-groups)
        .route(
            "/api/v1/petals/{petal_id}/visibility-groups",
            get(crate::visibility::list_visibility_groups)
                .put(crate::visibility::replace_visibility_groups),
        )
        .route(
            "/api/v1/petals/{petal_id}/visibility-groups/{group_id}",
            patch(crate::visibility::patch_visibility_group)
                .delete(crate::visibility::delete_visibility_group),
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
        // Agent visual verification: PNG of the live window (opt-in + Owner —
        // §screenshot), plus a dynamic posed capture (§vision-bridge) and a
        // camera read-back (§camera-state).
        .route(
            "/api/v1/debug/screenshot",
            get(crate::screenshot::get_screenshot).post(crate::vision::post_screenshot),
        )
        .route("/api/v1/debug/camera", get(crate::vision::get_camera))
        // Agent input injection (folded-in agent_input_surface_20261031):
        // opt-in OFF unless FE_API_INPUT_ALLOW=1 — mutates state, unlike the
        // two capture routes above (AGENTS.md §agent-input-surface).
        .route("/api/v1/debug/input", post(crate::input::post_input))
        .route("/api/v1/debug/ui-tree", get(crate::qa_debug::ui_tree))
        .route("/api/v1/debug/ui-state", get(crate::qa_debug::ui_state))
        .route(
            "/api/v1/debug/nodes/{node_id}/scene-asset-publication",
            get(crate::qa_scene_asset::get).layer(axum::extract::DefaultBodyLimit::max(0)),
        )
        .route("/api/v1/debug/invitation", get(crate::qa_invitation::observe).post(crate::qa_invitation::consume).layer(axum::extract::DefaultBodyLimit::max(2048)))
        .route("/api/v1/debug/portal/observe", get(crate::qa_portal::portal_observe))
        .route("/api/v1/debug/portal/ipc-stats", get(crate::qa_portal::portal_ipc_stats))
        .route("/api/v1/debug/portal/action", post(crate::qa_portal::portal_action).layer(axum::extract::DefaultBodyLimit::max(1024)))
        // Local kill switch for the QA lane (`{"engage": bool}` only), behind the same portal-fixture Owner+input gate
        // as `action` (steer 2026-11-09 02:15; handler `qa_portal::portal_killswitch`).
        .route("/api/v1/debug/portal/killswitch", post(crate::qa_portal::portal_killswitch).layer(axum::extract::DefaultBodyLimit::max(64)))
        .route("/api/v1/debug/portal/ipc-post", post(crate::qa_portal::portal_ipc_post).layer(axum::extract::DefaultBodyLimit::max(256)))
        .route("/api/v1/debug/portal-fixture/observe", get(crate::qa_portal::observe))
        .route("/api/v1/debug/portal-fixture/action", post(crate::qa_portal::action).layer(axum::extract::DefaultBodyLimit::max(1024)))
        .route("/api/v1/debug/portal-fixture/capture", post(crate::qa_portal::capture).layer(axum::extract::DefaultBodyLimit::max(1024)))
        .route("/api/v1/debug/logs", get(crate::qa_debug::logs))
        .route(
            "/api/v1/debug/dialog-answers",
            post(crate::qa_debug::dialog_answers)
                .layer(axum::extract::DefaultBodyLimit::max(65536)),
        )
        .route(
            "/api/v1/debug/window",
            post(crate::qa_debug::window).layer(axum::extract::DefaultBodyLimit::max(65536)),
        )
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
        // R5 device formats + device payloads (CDL-SPEC10 A4, §device-ingest)
        .route(
            "/api/v1/verses/{verse_id}/device-formats",
            post(crate::device::register_device_format).get(crate::device::list_device_formats),
        )
        .route(
            "/api/v1/petals/{petal_id}/iot/payloads",
            post(crate::device::ingest_device_payload)
                .get(crate::device::list_device_payloads)
                .layer(DefaultBodyLimit::max(
                    crate::upload::DEVICE_PAYLOAD_ROUTE_BODY_LIMIT,
                )),
        )
        // Shareable signed query URLs (mint; redemption is public)
        .route("/api/v1/query/share", post(crate::share::issue_share_url))
        // Analytics (DataFusion columnar queries over EntityStore)
        .route(
            "/api/v1/analytics/query",
            post(crate::rest::execute_analytics_query),
        )
        // Hexon tileset management (per-route body limit — §asset-ingest)
        .route(
            "/api/v1/hexons/tilesets/install",
            post(crate::terrain::install_hexon_tileset).layer(DefaultBodyLimit::max(
                crate::terrain::TILESET_ROUTE_BODY_LIMIT,
            )),
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
        // Sim control (F9/A20, Owner-only; fails closed without a sim bridge)
        .route("/api/v1/sim/start", post(crate::sim::post_start))
        .route("/api/v1/sim/stop", post(crate::sim::post_stop))
        .route("/api/v1/sim/status", get(crate::sim::get_status))
        .route("/api/v1/sim/step", post(crate::sim::post_step))
        .route(
            "/api/v1/sim/inject-fault",
            post(crate::sim::post_inject_fault),
        )
        // Relay advertisement (Manager+ on the verse; refused without a configured signer)
        // 32 KiB: two ≤2 KiB URLs (even fully JSON-escaped), a ≤256-byte note and one dial-back signature.
        .route("/api/v1/verses/{verse_id}/relays", post(crate::relay_advertise::post_relay).layer(DefaultBodyLimit::max(32 * 1024)))
        .route("/api/v1/verses/{verse_id}/relays/{relay_id}", delete(crate::relay_advertise::delete_relay))
        // MCP: 16 MiB body cap for every method + bounded concurrency
        // (DEC-C21; large GLBs use REST multipart — §asset-ingest).
        .route(
            "/mcp",
            post(crate::mcp::mcp_handler)
                .layer(DefaultBodyLimit::max(crate::upload::MCP_ROUTE_BODY_LIMIT))
                .layer(axum::middleware::from_fn_with_state(
                    Arc::new(tokio::sync::Semaphore::new(
                        crate::upload::MCP_MAX_CONCURRENT_CALLS,
                    )),
                    mcp_concurrency_limit,
                )),
        )
        // First-party extension routes mount inside the auth layer below.
        .merge(mount_extension_handles(&builtin_extension_handles()))
        // Plugin-host routes always mount inside the auth layer; an absent host answers the uniform
        // not-found through the same handlers (AGENTS.md §extensions "Mounting").
        .merge(plugin_host_routes(&hosts))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::auth_middleware,
        ));

    public
        .merge(authenticated)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(Extension(hosts))
        .with_state(state)
}

/// The extension, binding and bound-alias routers, body-capped at the mount; an unwired host is replaced by a
/// not-found host so every path answers exactly what a wired host answers for an unknown id (AGENTS.md §extensions).
fn plugin_host_routes(hosts: &ApiHosts) -> Router<Arc<ApiState>> {
    use crate::bindings::BindingHost;
    use crate::extensions::ExtensionHost;
    let extension: Arc<dyn ExtensionHost> = match &hosts.extension {
        Some(host) => host.clone(),
        None => Arc::new(absent_hosts::AbsentExtensionHost),
    };
    let binding: Arc<dyn BindingHost> = match &hosts.binding {
        Some(host) => host.clone(),
        None => Arc::new(absent_hosts::AbsentBindingHost),
    };
    let ingress_cap = || DefaultBodyLimit::max(crate::extensions::MAX_INGRESS_BODY_BYTES);
    Router::new()
        .merge(crate::extensions::routes(extension.clone()).layer(ingress_cap()))
        .merge(
            crate::bindings::routes(binding.clone())
                .layer(DefaultBodyLimit::max(crate::bindings::MAX_BINDING_BODY_BYTES)),
        )
        .merge(crate::bound_routes::router(binding, extension).layer(ingress_cap()))
}

/// Hosts that stand in for an unwired plugin host or binding plane: every lookup is the uniform not-found.
mod absent_hosts {
    use std::sync::Arc;

    use fe_runtime::extension_binding::BindingRecord;
    use fe_runtime::messages::BindingRefusal;
    use serde_json::Value;
    use tokio::sync::{oneshot, Notify};

    use crate::bindings::{BindingHost, BindingListReply, BindingWriteReply, EffectivePoint};
    use crate::extensions::{
        AdminOp, AdminRefusal, AdminReply, ApiCaller, CommandReply, ExtensionHost, HostRefusal,
        IngressAnswer, IngressCall, IngressRefusal,
    };

    /// No DID equals this, so no token is the local identity here.
    const NO_LOCAL_DID: &str = "";

    /// A reply channel that already holds the uniform not-found refusal.
    fn not_found<T, E>(refusal: E) -> oneshot::Receiver<Result<T, E>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Err(refusal));
        rx
    }

    fn admin_not_found() -> AdminReply {
        not_found(AdminRefusal { code: "not-found".into(), message: crate::rest::NOT_FOUND_OR_DENIED.into() })
    }

    fn binding_not_found() -> BindingRefusal {
        BindingRefusal { code: "not-found".into(), message: crate::rest::NOT_FOUND_OR_DENIED.into() }
    }

    /// Stands in for `ApiConfig.extension_host: None` (relay, harness).
    pub(super) struct AbsentExtensionHost;

    impl ExtensionHost for AbsentExtensionHost {
        fn local_did(&self) -> String {
            NO_LOCAL_DID.to_owned()
        }
        fn admit_remote(&self, _did: &str) -> bool {
            true
        }
        fn list(&self) -> Vec<Value> {
            Vec::new()
        }
        fn status(&self, _installation: &str) -> Option<Value> {
            None
        }
        fn audit(&self, _installation: &str, _limit: usize) -> Option<Vec<Value>> {
            None
        }
        fn administer(&self, _actor_did: &str, _op: AdminOp) -> Result<Value, AdminRefusal> {
            Err(AdminRefusal { code: "not-found".into(), message: crate::rest::NOT_FOUND_OR_DENIED.into() })
        }
        fn record_denied_admin(&self, _actor_did: &str, _target: &str) {}
        fn call_command(
            &self,
            _caller: &ApiCaller,
            _petal_id: &str,
            _package_id: &str,
            _command: &str,
            _args: &Value,
        ) -> Result<CommandReply, HostRefusal> {
            Err(HostRefusal::NotFound)
        }
        fn ingress(&self, _call: IngressCall) -> Result<oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal> {
            Err(IngressRefusal::NotFound)
        }
        fn open_client(&self, _caller: &ApiCaller) -> (u64, Arc<Notify>) {
            (0, Arc::new(Notify::new()))
        }
        fn client_subscribe(
            &self,
            _client: u64,
            _petal_id: &str,
            _source: &str,
            _params: &Value,
        ) -> Result<u64, HostRefusal> {
            Err(HostRefusal::NotFound)
        }
        fn client_unsubscribe(&self, _client: u64, _subscription: u64) {}
        fn take_client_events(&self, _client: u64, _max: usize) -> Vec<String> {
            Vec::new()
        }
        fn close_client(&self, _client: u64) {}
        // The trait defaults answer `not-available` (503); an absent host has nothing to answer about.
        fn verse_policy(&self, _verse_id: &str) -> AdminReply {
            admin_not_found()
        }
        fn set_verse_policy(&self, _actor_did: &str, _verse_id: &str, _policy: Value) -> AdminReply {
            admin_not_found()
        }
    }

    /// Stands in for `ApiConfig.binding_host: None`.
    pub(super) struct AbsentBindingHost;

    impl BindingHost for AbsentBindingHost {
        fn local_did(&self) -> String {
            NO_LOCAL_DID.to_owned()
        }
        fn put_binding(&self, _caller_did: &str, _template: BindingRecord) -> BindingWriteReply {
            not_found(binding_not_found())
        }
        fn list_bindings(&self, _caller_did: &str, _scope: &str) -> BindingListReply {
            not_found(binding_not_found())
        }
        fn effective(&self, _petal_scope: &str, _caller_did: &str) -> Option<Vec<EffectivePoint>> {
            None
        }
    }
}

/// First-party extension handles whose routes the engine mounts (fe-sdk/src/AGENTS.md §live-consumers).
fn builtin_extension_handles() -> Vec<fe_sdk::ApiExtensionHandle> {
    vec![fe_terrain::api_ext::terrain_api_routes()]
}

/// Resolve an extension route's `handler_id` to its handler; `None` = unknown (route not mounted).
fn extension_handler(handler_id: &str) -> Option<axum::routing::MethodRouter<Arc<ApiState>>> {
    match handler_id {
        "get_elevation_profile" => Some(get(crate::rest::get_elevation_profile)),
        "get_track_stats" => Some(get(crate::rest::get_track_stats)),
        _ => None,
    }
}

/// Rewrite a declared `:param` path segment to axum 0.8 `{param}` syntax.
fn extension_axum_path(declared: &str) -> String {
    declared
        .split('/')
        .map(|segment| match segment.strip_prefix(':') {
            Some(name) => format!("{{{name}}}"),
            None => segment.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Mount each handle's GET routes under `/api/v1/ext/{base_path}`; unknown handlers or methods are skipped with a warning (fail closed).
fn mount_extension_handles(handles: &[fe_sdk::ApiExtensionHandle]) -> Router<Arc<ApiState>> {
    let mut router = Router::new();
    for handle in handles {
        for route in &handle.routes {
            let handler = match (route.method, extension_handler(&route.handler_id)) {
                (fe_sdk::HttpMethod::Get, Some(handler)) => handler,
                _ => {
                    tracing::warn!(
                        extension = %handle.base_path,
                        handler = %route.handler_id,
                        "extension route not mounted: no handler registered for this method/handler_id"
                    );
                    continue;
                }
            };
            let path = format!(
                "/api/v1/ext/{}{}",
                handle.base_path.trim_matches('/'),
                extension_axum_path(&route.path)
            );
            router = router.route(&path, handler);
        }
    }
    router
}

/// Concurrency limit for `/mcp` (tower `ConcurrencyLimit` semantics: excess
/// requests wait for a permit BEFORE their body is buffered). A semaphore
/// middleware because `tower`'s `limit` feature is not a fe-api dependency.
async fn mcp_concurrency_limit(
    State(permits): State<Arc<tokio::sync::Semaphore>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    match permits.acquire_owned().await {
        Ok(_permit) => next.run(req).await,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
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

#[cfg(test)]
mod extension_mount_tests {
    use super::*;

    #[test]
    fn declared_params_become_axum_params() {
        assert_eq!(
            extension_axum_path("/elevation-profile/:track_id"),
            "/elevation-profile/{track_id}"
        );
    }

    #[test]
    fn every_builtin_extension_route_has_a_handler() {
        for handle in builtin_extension_handles() {
            assert!(!handle.routes.is_empty());
            for route in &handle.routes {
                assert!(
                    extension_handler(&route.handler_id).is_some(),
                    "unmounted extension route {} {}",
                    handle.base_path,
                    route.handler_id
                );
            }
        }
    }
}
