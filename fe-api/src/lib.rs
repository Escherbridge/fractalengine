// `Response` is axum's own error carrier for handlers, so `Result<T, Response>` is the idiomatic
// shape throughout this crate and boxing it would add indirection at every signature and call site
// for no benefit. clippy 1.98 began flagging it as `result_large_err` (axum's `Response` is exactly
// the lint's 128-byte threshold). Allowed at crate scope rather than distorting the API -- and
// rather than chasing it module by module, which reddened CI twice: `export.rs` at `c91a63b`, then
// `gis.rs` on the very next run, because clippy stops at the first failing crate.
#![allow(clippy::result_large_err)]

pub mod assets;
pub mod auth;
pub mod bridge_client;
pub mod canonical_ws;
pub mod crs;
pub mod db_reader;
pub mod dev_token;
pub mod device;
pub mod endpoint;
pub mod entity_store_bridge;
pub mod export;
pub mod format;
pub mod gis;
pub mod gpx;
pub mod hexon;
pub mod input;
pub mod iot;
pub mod limits;
pub mod mcp;
pub mod node_history;
pub mod operator_gate;
pub mod portal_bundles;
pub mod property_schema;
pub mod qa_debug;
pub mod qa_invitation;
pub mod qa_portal;
pub mod qa_scene_asset;
pub mod query_guard;
pub mod render_distance;
pub mod rest;
pub mod screenshot;
pub mod server;
pub mod share;
pub mod sim;
pub mod terrain;
mod terrain_credential;
pub mod timeseries_query;
pub mod types;
pub mod upload;
pub mod visibility;
pub mod vision;
pub mod ws;

use std::sync::Arc;

pub use server::ApiState;

/// Configuration for the API server thread.
pub struct ApiConfig {
    pub bind_addr: String,
    pub api_cmd_tx: crossbeam::channel::Sender<fe_runtime::messages::ApiCommand>,
    pub transform_broadcast_tx:
        tokio::sync::broadcast::Sender<fe_runtime::messages::TransformUpdate>,
    pub verifying_key: ed25519_dalek::VerifyingKey,
    /// Receiver for revoked token JTI notifications.
    pub revocation_rx: tokio::sync::broadcast::Receiver<String>,
    /// Content-addressed blob store for asset delivery. `None` if not configured.
    pub blob_store: Option<fe_runtime::blob_store::BlobStoreHandle>,
    /// Allowed CORS origins. Defaults to localhost-only if `None`.
    pub cors_origins: Option<Vec<String>>,
    /// Entity change broadcast for scene graph streaming (CUD deltas).
    pub entity_change_tx: tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>,
    /// Read-only SurrealDB connection for direct queries (bypasses crossbeam channel).
    pub api_db_reader: Option<std::sync::Arc<crate::db_reader::ReadDb>>,
    /// In-memory entity store for DataFusion analytics queries.
    pub entity_store: Option<std::sync::Arc<fe_entity_store::EntityStore>>,
    /// Tileset registry for hexon tile serving and management.
    pub tileset_registry: Option<std::sync::Arc<fe_terrain::tiles::TilesetRegistry>>,
    /// Hexon crate registry for package install/uninstall.
    pub hexon_registry: Option<std::sync::Arc<std::sync::Mutex<fe_hexon::registry::HexonRegistry>>>,
    /// P2P announcement store for peer-discovered crates.
    pub announcement_store:
        Option<std::sync::Arc<std::sync::Mutex<fe_hexon::p2p::AnnouncementStore>>>,
    /// Replication emit seam for the API thread (A11). The DB→sync bridge's
    /// event sender; `None` disables API-side replication emission.
    pub replication_tx: Option<fe_database::ReplicationSender>,
    /// Distributed-query fan-out seam (M2/F7 — A17): the API→sync bridge's
    /// call sender. `None` = no sync-thread seam (the distributed surfaces
    /// return an explicit unavailable error; local reads are unaffected).
    pub distributed_tx: Option<fe_runtime::distributed_query::DistributedQueryCallSender>,
    /// Sim control seam (F9/A20): the fe-sim bridge's call sender. Only a
    /// relay built with `--features sim-control` sets it (§sim-control).
    pub sim_control_tx: Option<fe_runtime::sim_control::SimControlCallSender>,
    /// Ed25519 keypair signing shareable query URLs (A24/DEC-C9): a
    /// dedicated keystore slot ("share_signer"), independent of the node
    /// identity key so a leaked share-signing key cannot be used to
    /// impersonate the node. Both binaries construct it via
    /// `load_or_generate_keypair(&secret_store, "share_signer")` before
    /// calling `spawn_api_thread` — see AGENTS.md §share.
    pub share_signer: Arc<fe_identity::NodeKeypair>,
    /// Operator opt-in for `/api/v1/query/elevated` — binaries pass
    /// [`elevated_query_allowed_from_env`] (AGENTS.md §elevated-query).
    pub elevated_query_allowed: bool,
    /// Per-owner upload quota — binaries pass [`upload_quota_bytes_from_env`]
    /// (AGENTS.md §asset-ingest).
    pub upload_quota_bytes: u64,
    /// Agent screenshot + vision surface — the GUI passes the env opt-in
    /// ([`screenshot::screenshot_allowed_from_env`]), which is now ON unless
    /// the operator sets it to `0` (agent_vision_20261031 DD-1), plus its
    /// capture bridge and (via [`screenshot::ScreenshotSurface::with_vision`])
    /// its dynamic-capture/camera-read-back bridge; headless hosts pass
    /// `ScreenshotSurface::disabled()` (AGENTS.md §screenshot, §camera-state).
    pub screenshot: screenshot::ScreenshotSurface,
    /// Agent input-injection surface (folded-in `agent_input_surface_20261031`
    /// via agent_vision_20261031): operator opt-in (OFF unless
    /// `FE_API_INPUT_ALLOW=1` — mutates state, unlike screenshot/camera's
    /// default-on posture, [`input::input_allowed_from_env`]) + its API→Bevy
    /// synthetic-input bridge (AGENTS.md §agent-input-surface); headless
    /// hosts pass `input::InputSurface::disabled()`.
    pub input: input::InputSurface,
    /// Told the bound address once the listener is up — the GUI's opt-in
    /// dev-token writer waits on it (AGENTS.md §qa-dev-token); `None` elsewhere.
    pub listening_tx: Option<crossbeam::channel::Sender<std::net::SocketAddr>>,
}

/// Read `FE_API_UPLOAD_QUOTA_BYTES` once at startup; unset or unparseable
/// falls back to [`upload::DEFAULT_UPLOAD_QUOTA_BYTES`] (AGENTS.md §asset-ingest).
pub fn upload_quota_bytes_from_env() -> u64 {
    match std::env::var(upload::UPLOAD_QUOTA_ENV) {
        Ok(v) => v.trim().parse().unwrap_or_else(|_| {
            tracing::warn!(
                "{}={v:?} is not a byte count; using the default upload quota",
                upload::UPLOAD_QUOTA_ENV
            );
            upload::DEFAULT_UPLOAD_QUOTA_BYTES
        }),
        Err(_) => upload::DEFAULT_UPLOAD_QUOTA_BYTES,
    }
}

/// Env var that opts a node into the raw-SQL `/api/v1/query/elevated` endpoint.
pub const ELEVATED_ALLOW_ENV: &str = "FE_API_ELEVATED_ALLOW";

/// The elevated-query gate: opt-in → Owner → 5/s per DID (AGENTS.md
/// §elevated-query, §operator-gate).
pub const ELEVATED_QUERY_GATE: operator_gate::OperatorGate = operator_gate::OperatorGate {
    opt_in_env: ELEVATED_ALLOW_ENV,
    endpoint: "elevated query endpoint",
    enabled_warning: "POST /api/v1/query/elevated is ENABLED — raw SurrealQL writes for Owner \
                      tokens. It is NOT scope-confined (an Owner of ANY verse reaches EVERY verse \
                      on this node); its writes are op-logged, replicated and broadcast like typed \
                      edits. Enable only on a single-tenant node",
    disabled_message: "elevated query endpoint is disabled on this node",
    rate_key_prefix: "elevated",
    rate_per_sec: 5,
    rate_label: "5 elevated queries/sec",
    // Pre-existing posture, unchanged by agent_vision_20261031 DD-1: raw SQL
    // writes stay opt-in only, never on by default.
    default_on: false,
};

/// Read the `FE_API_ELEVATED_ALLOW` opt-in once at startup (only `1` enables);
/// warns loudly when enabled — see AGENTS.md §elevated-query.
pub fn elevated_query_allowed_from_env() -> bool {
    ELEVATED_QUERY_GATE.opted_in_from_env()
}

/// Spawn a dedicated OS thread that owns a multi-threaded Tokio runtime and
/// runs the axum API server until the process exits.
pub fn spawn_api_thread(config: ApiConfig) -> std::thread::JoinHandle<()> {
    spawn_api_service_inner(config, false).into_join_handle()
}

/// Spawn an owned API whose mutations use the serial database executor.
pub fn spawn_api_service(config: ApiConfig) -> fe_runtime::service_lifecycle::OwnedService {
    spawn_api_service_inner(config, true)
}

fn spawn_api_service_inner(mut config: ApiConfig, command_writes: bool) -> fe_runtime::service_lifecycle::OwnedService {
    use fe_runtime::service_lifecycle::{OwnedService, StopSignal};
    if command_writes {
        config.api_db_reader = config.api_db_reader.as_ref().map(|db| Arc::new(db.with_command_writes()));
    }
    let stop = StopSignal::default();
    let worker_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build API tokio runtime");

        rt.block_on(run_server(config, worker_stop));
    });
    OwnedService::new("api", stop, thread)
}

async fn run_server(config: ApiConfig, stop: fe_runtime::service_lifecycle::StopSignal) {
    let revoked_jtis = Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()));

    let cors_origins = config.cors_origins.unwrap_or_else(|| {
        vec![
            "http://localhost:8765".to_string(),
            "http://127.0.0.1:8765".to_string(),
        ]
    });

    let state = Arc::new(ApiState {
        api_cmd_tx: config.api_cmd_tx,
        transform_broadcast_tx: config.transform_broadcast_tx,
        entity_change_tx: config.entity_change_tx,
        verifying_key: config.verifying_key,
        revoked_jtis: revoked_jtis.clone(),
        blob_store: config.blob_store,
        cors_origins,
        db_reader: config.api_db_reader,
        terrain_capture: Default::default(),
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: config.entity_store,
        tileset_registry: config.tileset_registry,
        hexon_registry: config.hexon_registry,
        announcement_store: config.announcement_store,
        replication_tx: config.replication_tx,
        distributed_tx: config.distributed_tx,
        sim_control_tx: config.sim_control_tx,
        // A24/DEC-C9: persistent, dedicated share-URL signing key supplied by
        // the caller (both binaries load-or-generate it from their secret
        // store before calling `spawn_api_thread`) — see AGENTS.md §share.
        share_signer: config.share_signer,
        elevated_query_allowed: config.elevated_query_allowed,
        upload_quota_bytes: config.upload_quota_bytes,
        screenshot: config.screenshot,
        input: config.input,
    });

    if auth::validate_auth_startup(&state).await.is_err() {
        tracing::error!("API credential authority is unavailable; listener was not started");
        return;
    }
    tokio::spawn(auth::listen_for_revocations(state.clone(), config.revocation_rx));

    let router = server::build_router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind API server to {}: {}", config.bind_addr, e));

    let local_addr = listener.local_addr().unwrap();
    tracing::info!("API server listening on {local_addr}");
    if let Some(listening_tx) = config.listening_tx {
        if listening_tx.send(local_addr).is_err() {
            tracing::warn!("API listening notification had no receiver");
        }
    }

    tokio::select! {
        _ = stop.cancelled() => tracing::info!("API runtime stopping"),
        result = async { axum::serve(listener, router).await } => {
            if let Err(error) = result { tracing::error!(%error, "API server exited with error"); }
        }
    }
}
