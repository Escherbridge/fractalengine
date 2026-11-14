pub mod gpx_collection;
pub mod material;
pub mod model;
pub mod skybox;
pub mod sound;
pub mod terrain;

use crate::manifest::HexonType;
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

/// Refuse types with their own install path (Script, PetalSnapshot, Plugin); call before any blob or row write.
pub fn check_content_installable(hexon_type: &HexonType) -> Result<(), anyhow::Error> {
    match hexon_type {
        HexonType::Script | HexonType::PetalSnapshot | HexonType::Plugin => Err(anyhow::anyhow!(
            "hexon type {hexon_type} is not installed as content"
        )),
        _ => Ok(()),
    }
}

/// Dispatch handler based on the hexon type.
pub fn handle_install(
    package: &HexonPackageData,
    petal_id: &str,
    blob_store: &FsBlobStore,
) -> Result<InstallResult, anyhow::Error> {
    check_content_installable(&package.manifest.hexon_type)?;
    match &package.manifest.hexon_type {
        HexonType::Model => model::handle_model_install(package, petal_id, blob_store),
        HexonType::Material => material::handle_material_install(package, petal_id, blob_store),
        HexonType::Skybox => skybox::handle_skybox_install(package, petal_id, blob_store),
        HexonType::Terrain => terrain::handle_terrain_install(package, petal_id, blob_store),
        HexonType::GpxCollection => {
            gpx_collection::handle_gpx_install(package, petal_id, blob_store)
        }
        HexonType::Sound => sound::handle_sound_install(package, petal_id, blob_store),
        _ => {
            // Theme, Scene, Bundle, ... — no special handling yet
            Ok(InstallResult {
                registered_assets: vec![],
                summary: format!(
                    "Installed {} hexon (no special handler)",
                    package.manifest.hexon_type
                ),
                extras: InstallExtras::default(),
            })
        }
    }
}
