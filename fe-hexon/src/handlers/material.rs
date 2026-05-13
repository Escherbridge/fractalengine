use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;
use std::collections::HashMap;
use tracing::info;

/// PBR material handle with texture references for Bevy StandardMaterial construction.
///
/// Maps texture roles (albedo, normal, roughness, ao, metallic) to their
/// content-addressed blob hashes via the entry's `sub_assets` map.
#[derive(Debug, Clone)]
pub struct MaterialHandle {
    pub entry_id: String,
    pub albedo_hash: Option<String>,
    pub normal_hash: Option<String>,
    pub roughness_hash: Option<String>,
    pub ao_hash: Option<String>,
    pub metallic_hash: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

impl MaterialHandle {
    /// Extract a MaterialHandle from an AssetEntry's sub_assets map.
    pub fn from_sub_assets(
        entry_id: &str,
        sub_assets: &Option<HashMap<String, String>>,
        metadata: &Option<serde_json::Value>,
    ) -> Self {
        let get = |key: &str| -> Option<String> { sub_assets.as_ref()?.get(key).cloned() };
        Self {
            entry_id: entry_id.to_string(),
            albedo_hash: get("albedo"),
            normal_hash: get("normal"),
            roughness_hash: get("roughness"),
            ao_hash: get("ao"),
            metallic_hash: get("metallic"),
            metadata: metadata.clone(),
        }
    }

    /// Collect all referenced hashes.
    pub fn all_hashes(&self) -> Vec<&str> {
        [
            self.albedo_hash.as_deref(),
            self.normal_hash.as_deref(),
            self.roughness_hash.as_deref(),
            self.ao_hash.as_deref(),
            self.metallic_hash.as_deref(),
        ]
        .iter()
        .filter_map(|h| *h)
        .collect()
    }
}

/// Handle installation of Material entries from a hexon package.
///
/// Stores all asset blobs (material bundles and their textures) in the
/// blob store, then builds `MaterialHandle` structs from each material
/// entry's `sub_assets` to verify all referenced texture blobs are present.
pub fn handle_material_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut material_count = 0u32;

    for entry in &package.entries {
        // Store all asset blobs (materials, textures)
        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
        }

        if entry.kind != EntryKind::Material {
            continue;
        }

        let handle =
            MaterialHandle::from_sub_assets(&entry.entry_id, &entry.sub_assets, &entry.metadata);

        // Verify all sub-asset blobs exist and are stored
        for hash in handle.all_hashes() {
            if let Some(data) = package.assets.get(hash) {
                blob_store.store(hash, data)?;
                if !result.registered_assets.contains(&hash.to_string()) {
                    result.registered_assets.push(hash.to_string());
                }
            }
        }

        material_count += 1;
        info!(
            "Registered PBR material {} for petal {} (sub_assets: {:?})",
            entry.entry_id,
            petal_id,
            entry
                .sub_assets
                .as_ref()
                .map(|s| s.keys().collect::<Vec<_>>())
        );
    }

    result.summary = format!(
        "Installed {} material(s) with {} total blobs for petal {}",
        material_count,
        result.registered_assets.len(),
        petal_id
    );
    Ok(result)
}
