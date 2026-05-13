pub mod gpx_collection;
pub mod material;
pub mod model;
pub mod skybox;
pub mod sound;
pub mod terrain;

use crate::manifest::HexonKind;
use crate::package::HexonPackageData;
use crate::registry::FsBlobStore;

use crate::handlers::terrain::TerrainInstallData;

/// Type-specific data produced by an install handler, available for
/// the caller to wire into domain-specific configuration (e.g. petal
/// TerrainConfig for terrain crates).
#[derive(Debug, Clone, Default)]
pub enum InstallExtras {
    /// Terrain crate install data — tilesets, metadata, blob references.
    Terrain(TerrainInstallData),
    /// No type-specific data (model, sound, skybox, etc.).
    #[default]
    None,
}

/// Result of installing entries from a hexon package.
#[derive(Debug, Default)]
pub struct InstallResult {
    /// Asset hashes that were registered in the blob store.
    pub registered_assets: Vec<String>,
    /// Human-readable summary of what was installed.
    pub summary: String,
    /// Type-specific data for post-install configuration by the caller.
    pub extras: InstallExtras,
}

/// Dispatch handler based on the hexon type.
pub fn handle_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    match package.manifest.hexon_type {
        HexonKind::Model => model::handle_model_install(package, petal_id, blob_store),
        HexonKind::Material => material::handle_material_install(package, petal_id, blob_store),
        HexonKind::Skybox => skybox::handle_skybox_install(package, petal_id, blob_store),
        HexonKind::Terrain => terrain::handle_terrain_install(package, petal_id, blob_store),
        HexonKind::GpxCollection => {
            gpx_collection::handle_gpx_install(package, petal_id, blob_store)
        }
        HexonKind::Sound => sound::handle_sound_install(package, petal_id, blob_store),
        _ => {
            // Theme, Script — no special handling yet
            Ok(InstallResult {
                registered_assets: vec![],
                summary: format!(
                    "Installed {:?} crate (no special handler)",
                    package.manifest.hexon_type
                ),
                extras: InstallExtras::default(),
            })
        }
    }
}
