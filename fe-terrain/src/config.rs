use crate::projection::Projection;
use serde::{Deserialize, Serialize};

/// Elevation data source kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ElevationSourceKind {
    /// Mapbox Terrain-RGB tiles.
    TerrainRgb,
    /// Mapzen Terrarium tiles.
    Terrarium,
    /// No elevation data — flat terrain.
    None,
}

impl Default for ElevationSourceKind {
    fn default() -> Self {
        Self::None
    }
}

/// Layer configuration within a terrain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerConfig {
    pub name: String,
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// How the terrain system resolves tiles at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TileSourceMode {
    /// Only serve tiles from installed hexon archives (no network).
    Offline,
    /// Only fetch tiles from online URL sources (ignore hexons).
    Online,
    /// Hexon first, then disk cache, then online fallback (default).
    #[default]
    Hybrid,
}

/// Full terrain configuration for a petal.
///
/// Stored as petal properties JSON under the `"terrain"` key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerrainConfig {
    pub enabled: bool,
    pub origin: Projection,
    pub tile_source_url: String,
    pub elevation_source: ElevationSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_api_key_env: Option<String>,
    #[serde(default = "default_max_zoom")]
    pub max_zoom: u8,
    #[serde(default = "default_min_zoom")]
    pub min_zoom: u8,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerConfig>,
    /// URIs or file paths to `.hexon` tileset archives for offline tile serving.
    /// Loaded at terrain init and added to the `CompositeTileSource` fallback chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tileset_hexon_uris: Vec<String>,
    /// Tile resolution strategy used at runtime.
    #[serde(default)]
    pub tile_source_mode: TileSourceMode,
}

fn default_max_zoom() -> u8 {
    15
}

fn default_min_zoom() -> u8 {
    10
}

fn default_cache_dir() -> String {
    "terrain_cache".into()
}

impl Default for TerrainConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            origin: Projection::new(0.0, 0.0, 0.0),
            tile_source_url: String::new(),
            elevation_source: ElevationSourceKind::None,
            elevation_api_key_env: None,
            max_zoom: default_max_zoom(),
            min_zoom: default_min_zoom(),
            cache_dir: default_cache_dir(),
            layers: vec![],
            tileset_hexon_uris: vec![],
            tile_source_mode: TileSourceMode::default(),
        }
    }
}

/// Binding between a petal and its terrain configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetalTerrainBinding {
    pub petal_id: String,
    pub config: TerrainConfig,
}
