//! Thin binary wrapper around the fe-hexon-registry library.

use std::sync::Arc;

use fe_hexon_registry::{build_router, scan_dir, RegistryConfig, RegistryState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = RegistryConfig::from_env();
    if cfg.token.is_none() {
        tracing::warn!("FE_REGISTRY_TOKEN not set — registry is open access");
    }
    std::fs::create_dir_all(&cfg.dir)?;

    let entries = scan_dir(&cfg.dir);
    tracing::info!(count = entries.len(), dir = %cfg.dir.display(), "indexed hexon packages");

    let state = Arc::new(RegistryState::new(
        cfg.dir,
        cfg.token,
        cfg.readonly,
        entries,
    ));
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(bind = %cfg.bind, "fe-hexon-registry listening");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
