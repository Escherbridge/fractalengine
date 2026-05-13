use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;
use tracing::info;

/// Registered model asset, suitable for Bevy asset pipeline loading.
#[derive(Debug, Clone)]
pub struct ModelAsset {
    pub entry_id: String,
    pub asset_hash: String,
    pub format: String,
    pub label: String,
    pub is_placeable: bool,
}

/// Handle installation of Model entries from a hexon package.
///
/// Registers each model entry's asset blob in the blob store, along with
/// any bundled texture entries. Returns an `InstallResult` with the list
/// of registered asset hashes and a summary.
pub fn handle_model_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut model_count = 0u32;

    for entry in &package.entries {
        if entry.kind != EntryKind::Model {
            continue;
        }

        // Verify blob exists in package and store it
        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
            model_count += 1;
            info!(
                "Registered model asset {} ({}) for petal {}",
                entry.entry_id, entry.format, petal_id
            );
        } else {
            tracing::warn!(
                "Model entry {} references missing asset blob {}",
                entry.entry_id,
                entry.asset_hash
            );
        }
    }

    // Also register any texture entries bundled with the model crate
    for entry in &package.entries {
        if entry.kind == EntryKind::Texture {
            if let Some(data) = package.assets.get(&entry.asset_hash) {
                blob_store.store(&entry.asset_hash, data)?;
                result.registered_assets.push(entry.asset_hash.clone());
            }
        }
    }

    result.summary = format!(
        "Installed {} model(s) with {} total assets for petal {}",
        model_count,
        result.registered_assets.len(),
        petal_id
    );
    Ok(result)
}
