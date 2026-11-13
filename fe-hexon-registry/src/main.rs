//! Thin binary wrapper around the fe-hexon-registry library.

use std::sync::Arc;

use fe_hexon_registry::{
    build_router, FsPackageStore, PackageStore, RegistryConfig, RegistryState,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = RegistryConfig::from_env();
    cfg.validate()?;
    if cfg.token.is_none() {
        tracing::warn!("FE_REGISTRY_ALLOW_OPEN set and no token: /api/v1/hexons is open access");
    }
    std::fs::create_dir_all(&cfg.dir)?;

    let store: Arc<dyn PackageStore> = Arc::new(FsPackageStore::new(cfg.dir.clone()));
    let entries = store.list().await?;
    tracing::info!(count = entries.len(), dir = %cfg.dir.display(), "indexed hexon packages");

    let state = Arc::new(
        RegistryState::with_store(store, cfg.token, cfg.readonly, entries)
            .with_public_blobs(cfg.public_blobs),
    );
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(bind = %cfg.bind, "fe-hexon-registry listening");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
