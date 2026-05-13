use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;
use tracing::info;

/// Installed skybox asset reference with format and resolution metadata.
#[derive(Debug, Clone)]
pub struct SkyboxAsset {
    pub entry_id: String,
    pub asset_hash: String,
    pub format: String,
    pub resolution: Option<[u32; 2]>,
}

impl SkyboxAsset {
    /// Extract resolution from entry metadata if present.
    ///
    /// Expects metadata of the form `{"resolution": [width, height]}`.
    pub fn from_entry(entry: &crate::manifest::AssetEntry) -> Self {
        let resolution = entry.metadata.as_ref().and_then(|m| {
            let arr = m.get("resolution")?;
            let w = arr.get(0)?.as_u64()? as u32;
            let h = arr.get(1)?.as_u64()? as u32;
            Some([w, h])
        });
        Self {
            entry_id: entry.entry_id.clone(),
            asset_hash: entry.asset_hash.clone(),
            format: entry.format.clone(),
            resolution,
        }
    }
}

/// Handle installation of Skybox entries from a hexon package.
///
/// Registers each skybox blob (HDR/EXR equirectangular) in the blob store
/// and extracts resolution metadata from entry metadata.
pub fn handle_skybox_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut skybox_count = 0u32;

    for entry in &package.entries {
        if entry.kind != EntryKind::Skybox {
            continue;
        }

        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
            skybox_count += 1;

            let asset = SkyboxAsset::from_entry(entry);
            info!(
                "Registered skybox {} (format: {}, resolution: {:?}) for petal {}",
                asset.entry_id, asset.format, asset.resolution, petal_id
            );
        }
    }

    result.summary = format!(
        "Installed {} skybox(es) for petal {}",
        skybox_count, petal_id
    );
    Ok(result)
}
