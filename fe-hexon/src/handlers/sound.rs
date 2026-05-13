use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;
use tracing::info;

/// Installed sound asset reference with spatial audio metadata.
#[derive(Debug, Clone)]
pub struct SoundAsset {
    pub entry_id: String,
    pub asset_hash: String,
    pub format: String,
    pub is_spatial: bool,
}

impl SoundAsset {
    /// Build a SoundAsset from an AssetEntry.
    ///
    /// Reads `spatial: true/false` from entry metadata if present.
    pub fn from_entry(entry: &crate::manifest::AssetEntry) -> Self {
        let is_spatial = entry
            .metadata
            .as_ref()
            .and_then(|m| m.get("spatial"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Self {
            entry_id: entry.entry_id.clone(),
            asset_hash: entry.asset_hash.clone(),
            format: entry.format.clone(),
            is_spatial,
        }
    }
}

/// Handle installation of Sound entries from a hexon package.
///
/// Registers each sound blob (OGG/WAV) in the blob store and extracts
/// spatial audio metadata from the entry.
pub fn handle_sound_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut sound_count = 0u32;

    for entry in &package.entries {
        if entry.kind != EntryKind::Sound {
            continue;
        }

        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
            sound_count += 1;

            let asset = SoundAsset::from_entry(entry);
            info!(
                "Registered sound {} (format: {}, spatial: {}) for petal {}",
                asset.entry_id, asset.format, asset.is_spatial, petal_id
            );
        }
    }

    result.summary = format!("Installed {} sound(s) for petal {}", sound_count, petal_id);
    Ok(result)
}
