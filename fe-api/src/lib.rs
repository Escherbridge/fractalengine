pub mod assets;
pub mod auth;
pub mod format;
pub mod gis;
pub mod gpx;
pub mod hexon;
pub mod mcp;
pub mod rest;
pub mod server;
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
    pub announcement_store: Option<std::sync::Arc<std::sync::Mutex<fe_hexon::p2p::AnnouncementStore>>>,
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
