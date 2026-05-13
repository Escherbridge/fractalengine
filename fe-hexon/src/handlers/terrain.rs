use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;
use std::collections::HashMap;
use tracing::info;

/// Extracted terrain data ready for integration with fe-terrain.
///
/// fe-hexon does NOT depend on fe-terrain directly. This struct provides
/// terrain data in a structured form so the caller (fe-runtime or an
/// integration layer) can feed it to `DiskTileCache` or `TerrainConfig`.
#[derive(Debug, Clone)]
pub struct TerrainInstallData {
    /// Tileset entry IDs that were installed.
    pub tileset_entries: Vec<String>,
    /// Tile data keyed by cache key (z/x/y), ready for DiskTileCache.put().
    /// Populated by the caller from the blob store after installation.
    pub tile_cache_entries: HashMap<String, Vec<u8>>,
    /// Terrain config metadata from the package entries.
    pub terrain_metadata: Option<serde_json::Value>,
}

/// Handle installation of Terrain entries from a hexon package.
///
/// Stores asset blobs and extracts tileset metadata for the caller
/// to wire into fe-terrain's DiskTileCache and TerrainConfig.
pub fn handle_terrain_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut tileset_count = 0u32;

    for entry in &package.entries {
        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
        }

        if entry.kind == EntryKind::TerrainTileset {
            tileset_count += 1;
            info!(
                "Registered terrain tileset {} (format: {}) for petal {}",
                entry.entry_id, entry.format, petal_id
            );
        }
    }

    // Also count any GPX files bundled with the terrain crate
    let gpx_count = package
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::GpxFile)
        .count();

    result.summary = format!(
        "Installed terrain crate with {} tileset(s) and {} GPX file(s) for petal {}",
        tileset_count, gpx_count, petal_id
    );
    Ok(result)
}

/// Extract terrain install data from a package for fe-terrain integration.
///
/// The returned `TerrainInstallData` can be used by the integration layer
/// to populate `DiskTileCache` and configure `TerrainConfig`.
pub fn extract_terrain_data(package: &HexonPackageData) -> TerrainInstallData {
    let mut tileset_entries = Vec::new();
    let mut terrain_metadata = None;

    for entry in &package.entries {
        if entry.kind == EntryKind::TerrainTileset {
            tileset_entries.push(entry.entry_id.clone());
            if terrain_metadata.is_none() {
                terrain_metadata = entry.metadata.clone();
            }
        }
    }

    TerrainInstallData {
        tileset_entries,
        tile_cache_entries: HashMap::new(), // populated by caller from blob store
        terrain_metadata,
    }
}
