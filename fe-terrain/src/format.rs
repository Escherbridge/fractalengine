use anyhow::Result;

/// Terrain configuration for .hexon archives.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerrainConfig {
    pub crs: String,
    pub tile_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_origin: Option<ProjectionOrigin>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectionOrigin {
    pub lat: f64,
    pub lon: f64,
    pub ele: f64,
}

/// Bundle of terrain data extracted from a .hexon archive.
#[derive(Debug, Clone)]
pub struct TerrainBundle {
    pub config: TerrainConfig,
    pub gpx_files: Vec<(String, Vec<u8>)>,
    pub geojson_overlays: Vec<(String, Vec<u8>)>,
}

/// Extension trait for terrain-specific .hexon operations.
///
/// Wraps `fe_format::HexonArchive` export/import with ergonomic terrain methods.
/// The underlying archive already handles the `terrain/` directory layout:
/// - `terrain/config.json`
/// - `terrain/tracks/*.gpx`
/// - `terrain/overlays/*.geojson`
pub trait TerrainFormatExt {
    fn export_terrain_data(
        config: &TerrainConfig,
        gpx_files: &[(String, Vec<u8>)],
        geojson_overlays: &[(String, Vec<u8>)],
        manifest: fe_format::HexonManifest,
    ) -> Result<Vec<u8>>;

    fn import_terrain_data(archive_bytes: &[u8]) -> Result<Option<TerrainBundle>>;
}

impl TerrainFormatExt for fe_format::HexonArchive {
    /// Export terrain data as a .hexon archive.
    ///
    /// Creates an archive with:
    /// - `terrain/config.json` from the config
    /// - `terrain/tracks/*.gpx` from gpx_files
    /// - `terrain/overlays/*.geojson` from geojson_overlays
    fn export_terrain_data(
        config: &TerrainConfig,
        gpx_files: &[(String, Vec<u8>)],
        geojson_overlays: &[(String, Vec<u8>)],
        manifest: fe_format::HexonManifest,
    ) -> Result<Vec<u8>> {
        let config_json = serde_json::to_value(config)?;
        fe_format::HexonArchive::export(
            manifest,
            vec![],
            vec![],
            Some(fe_format::License::default()),
            Some(&config_json),
            Some(gpx_files),
            Some(geojson_overlays),
        )
    }

    /// Import terrain data from a .hexon archive.
    ///
    /// Returns `None` if the archive contains no terrain/ directory.
    fn import_terrain_data(archive_bytes: &[u8]) -> Result<Option<TerrainBundle>> {
        let data = fe_format::HexonArchive::import(archive_bytes)?;

        let config_val = match data.terrain_config {
            Some(v) => v,
            None => return Ok(None),
        };

        let config: TerrainConfig = serde_json::from_value(config_val)?;

        Ok(Some(TerrainBundle {
            config,
            gpx_files: data.gpx_files,
            geojson_overlays: data.geojson_overlays,
        }))
    }
}
