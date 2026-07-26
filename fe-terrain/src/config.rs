use crate::projection::Projection;
use crate::terrain_proposal::TerrainProposal;
use serde::{Deserialize, Serialize};

/// Elevation data source kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ElevationSourceKind {
    /// Mapbox Terrain-RGB tiles.
    TerrainRgb,
    /// Mapzen Terrarium tiles.
    Terrarium,
    /// No elevation data — flat terrain.
    #[default]
    None,
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
    /// World units per real meter (`0.001` → 1 unit per km); see `src/AGENTS.md` §scale.
    #[serde(default = "default_world_scale")]
    pub world_scale: f64,
    /// Hexon-declared `[min, max]` world-scale bounds (C1); user nudges of
    /// `world_scale` are clamped into this range. `None` = unbounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_bounds: Option<[f64; 2]>,
    /// Non-destructive analytics terrain proposals (overlays only; NEVER written
    /// to the heightfield/tileset — NFR-1). Additive: pre-feature petal JSON
    /// omits the key. See `src/AGENTS.md` §terrain-proposals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<TerrainProposal>,
}

impl TerrainConfig {
    /// Sanitized, hexon-bound-clamped world scale (FR-5); falls back to
    /// `1.0` on bad JSON, clamps into `scale_bounds` when present.
    pub fn effective_world_scale(&self) -> f64 {
        crate::scale::clamp_world_scale_to_bounds(self.world_scale, self.scale_bounds)
    }
}

fn default_world_scale() -> f64 {
    1.0
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
            world_scale: default_world_scale(),
            scale_bounds: None,
            proposals: vec![],
        }
    }
}

/// Binding between a petal and its terrain configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetalTerrainBinding {
    pub petal_id: String,
    pub config: TerrainConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_scale_defaults_when_absent() {
        // Existing stored petal JSON (pre-feature) has no `world_scale` key.
        let json = serde_json::json!({
            "enabled": true,
            "origin": { "origin_lat": 45.0, "origin_lon": -122.0, "origin_ele": 0.0 },
            "tile_source_url": "",
            "elevation_source": "terrain_rgb",
        });
        let cfg: TerrainConfig = serde_json::from_value(json).expect("parses without world_scale");
        assert_eq!(cfg.world_scale, 1.0);
        assert_eq!(cfg.effective_world_scale(), 1.0);
    }

    #[test]
    fn world_scale_roundtrips_and_sanitizes() {
        let mut cfg = TerrainConfig {
            world_scale: 0.001,
            ..TerrainConfig::default()
        };
        let value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(value["world_scale"], serde_json::json!(0.001));
        let parsed: TerrainConfig = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.world_scale, 0.001);

        // Bad values fall back to 1.0 via effective_world_scale.
        cfg.world_scale = 0.0;
        assert_eq!(cfg.effective_world_scale(), 1.0);
        cfg.world_scale = f64::NAN;
        assert_eq!(cfg.effective_world_scale(), 1.0);
    }

    #[test]
    fn proposals_default_empty_and_roundtrip_additively() {
        use crate::terrain_proposal::{TerrainOp, TerrainProposal};

        // Pre-feature stored JSON (no `proposals` key) parses to an empty set.
        let json = serde_json::json!({
            "enabled": true,
            "origin": { "origin_lat": 0.0, "origin_lon": 0.0, "origin_ele": 0.0 },
            "tile_source_url": "",
            "elevation_source": "none",
        });
        let cfg: TerrainConfig = serde_json::from_value(json).expect("parses without proposals");
        assert!(cfg.proposals.is_empty());

        // With proposals: they roundtrip and the empty case stays omitted.
        let mut cfg = TerrainConfig::default();
        assert!(serde_json::to_value(&cfg)
            .unwrap()
            .get("proposals")
            .is_none());
        cfg.proposals.push(TerrainProposal {
            id: "p1".into(),
            op: TerrainOp::Flatten,
            footprint: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            target_height: Some(5.0),
            delta: None,
            material: None,
        });
        let value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(value["proposals"][0]["op"], serde_json::json!("flatten"));
        let parsed: TerrainConfig = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.proposals, cfg.proposals);
    }

    #[test]
    fn effective_world_scale_clamps_to_hexon_scale_bounds() {
        let cfg = TerrainConfig {
            world_scale: 5.0,
            scale_bounds: Some([0.001, 0.1]),
            ..TerrainConfig::default()
        };
        assert_eq!(cfg.effective_world_scale(), 0.1);
    }
}
