// `Response` is axum's own error carrier for handlers, so `Result<T, Response>` is the idiomatic
// shape throughout this crate and boxing it would add indirection at every signature and call site
// for no benefit. clippy 1.98 began flagging it as `result_large_err` (axum's `Response` is exactly
// the lint's 128-byte threshold). Allowed at crate scope rather than distorting the API -- and
// rather than chasing it module by module, which reddened CI twice: `export.rs` at `ca338bc`, then
// `gis.rs` on the very next run, because clippy stops at the first failing crate.
#![allow(clippy::result_large_err)]

pub mod assets;
pub mod auth;
pub mod canonical_ws;
pub mod crs;
pub mod endpoint;
pub mod export;
pub mod format;
pub mod gis;
pub mod gpx;
pub mod hexon;
pub mod iot;
pub mod limits;
pub mod mcp;
pub mod query_guard;
pub mod rest;
pub mod server;
pub mod share;
pub mod terrain;
pub mod types;
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
    pub api_db_reader: Option<std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>>,
    /// In-memory entity store for DataFusion analytics queries.
    pub entity_store: Option<std::sync::Arc<fe_entity_store::EntityStore>>,
    /// Tileset registry for hexon tile serving and management.
    pub tileset_registry: Option<std::sync::Arc<fe_terrain::tiles::TilesetRegistry>>,
    /// Hexon crate registry for package install/uninstall.
    pub hexon_registry: Option<std::sync::Arc<std::sync::Mutex<fe_hexon::registry::HexonRegistry>>>,
    /// P2P announcement store for peer-discovered crates.
    pub announcement_store:
        Option<std::sync::Arc<std::sync::Mutex<fe_hexon::p2p::AnnouncementStore>>>,
}

/// Spawn a dedicated OS thread that owns a multi-threaded Tokio runtime and
/// runs the axum API server until the process exits.
pub fn spawn_api_thread(config: ApiConfig) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build API tokio runtime");

        rt.block_on(run_server(config));
    })
}

async fn run_server(config: ApiConfig) {
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
        query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        entity_store: config.entity_store,
        tileset_registry: config.tileset_registry,
        hexon_registry: config.hexon_registry,
        announcement_store: config.announcement_store,
        // Ephemeral per-process share-URL signing key: restart invalidates
        // outstanding links (TTL ≤ 24h anyway) — see AGENTS.md §share.
        share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
    });

    // Background task: listen for revocation notifications from Bevy thread
    let revoked_cache = revoked_jtis.clone();
    let mut revocation_rx = config.revocation_rx;
    tokio::spawn(async move {
        loop {
            match revocation_rx.recv().await {
                Ok(jti) => {
                    revoked_cache.write().await.insert(jti);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Revocation listener lagged by {n} messages");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        tracing::info!("Revocation listener shut down");
    });

    let router = server::build_router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind API server to {}: {}", config.bind_addr, e));

    tracing::info!("API server listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, router)
        .await
        .expect("API server exited with error");
}
