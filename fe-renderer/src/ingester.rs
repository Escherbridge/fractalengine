use fe_network::AssetId;
use crate::addressing::{content_address, validate_glb_magic};

pub const MAX_ASSET_SIZE_BYTES: usize = 256 * 1024 * 1024;

pub trait AssetIngester: Send + Sync {
    fn ingest(&self, bytes: &[u8]) -> anyhow::Result<AssetId>;
}

pub struct GltfIngester;

impl AssetIngester for GltfIngester {
    fn ingest(&self, bytes: &[u8]) -> anyhow::Result<AssetId> {
        if bytes.len() > MAX_ASSET_SIZE_BYTES {
            anyhow::bail!("Asset exceeds 256MB limit");
        }
        if !validate_glb_magic(bytes) {
            anyhow::bail!("Not a valid GLB file");
        }
        let asset_id = content_address(bytes);
        let cache = cache_path(asset_id);
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(cache, bytes)?;
        Ok(asset_id)
    }
}

pub struct SplatIngester;

impl AssetIngester for SplatIngester {
    fn ingest(&self, _bytes: &[u8]) -> anyhow::Result<AssetId> {
        todo!("v2 target: Gaussian splatting ingestion")
    }
}

pub fn cache_path(asset_id: AssetId) -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"))
        .join("fractalengine")
        .join("assets")
        .join(hex::encode(asset_id.0))
}
