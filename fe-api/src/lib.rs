pub mod assets;
pub mod auth;
pub mod mcp;
pub mod rest;
pub mod server;
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
