use super::InstallResult;
use crate::manifest::EntryKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;
use tracing::info;

/// A GPX file extracted from a hexon package, ready for import.
///
/// Contains raw GPX bytes that can be fed to
/// `fe_terrain::gpx::parser::parse_gpx_bytes()` and then
/// `fe_terrain::gpx::convert::gpx_to_scene_commands()`.
#[derive(Debug, Clone)]
pub struct GpxFileData {
    pub entry_id: String,
    pub asset_hash: String,
    pub label: String,
    pub gpx_bytes: Vec<u8>,
}

/// Handle installation of GpxCollection entries from a hexon package.
///
/// Stores all GPX file blobs in the blob store and returns a summary
/// of how many files were registered.
pub fn handle_gpx_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    let mut result = InstallResult::default();
    let mut gpx_count = 0u32;

    for entry in &package.entries {
        if let Some(data) = package.assets.get(&entry.asset_hash) {
            blob_store.store(&entry.asset_hash, data)?;
            result.registered_assets.push(entry.asset_hash.clone());
        }

        if entry.kind == EntryKind::GpxFile {
            gpx_count += 1;
            info!(
                "Registered GPX file {} ({}) for petal {}",
                entry.entry_id, entry.label, petal_id
            );
        }
    }

    result.summary = format!(
        "Installed GPX collection with {} file(s) for petal {}",
        gpx_count, petal_id
    );
    Ok(result)
}

/// Extract GPX file data from a package for fe-terrain integration.
///
/// Returns raw GPX bytes that can be fed to `fe_terrain::gpx::parser::parse_gpx_bytes()`
/// and then `fe_terrain::gpx::convert::gpx_to_scene_commands()`.
pub fn extract_gpx_files(package: &HexonPackageData) -> Vec<GpxFileData> {
    package
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::GpxFile)
        .filter_map(|entry| {
            let bytes = package.assets.get(&entry.asset_hash)?;
            Some(GpxFileData {
                entry_id: entry.entry_id.clone(),
                asset_hash: entry.asset_hash.clone(),
                label: entry.label.clone(),
                gpx_bytes: bytes.clone(),
            })
        })
        .collect()
}
