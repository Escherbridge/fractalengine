use crate::types::AssetId;
use std::path::PathBuf;

pub async fn register_asset(hash: AssetId, path: PathBuf) -> anyhow::Result<()> {
    // CROSS-CRATE: iroh-blobs provide — wire to iroh endpoint in Sprint 5B
    tracing::info!("Registering asset {:?} at {:?}", hex::encode(hash.0), path);
    Ok(())
}

pub async fn fetch_asset(hash: AssetId) -> anyhow::Result<Vec<u8>> {
    // CROSS-CRATE: iroh-blobs get — wire to iroh endpoint in Sprint 5B
    anyhow::bail!("iroh-blobs fetch not yet wired — deferred to Sprint 5B")
}
