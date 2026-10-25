use crate::projection::Projection;
use crate::terrain_proposal::TerrainProposal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// RP-4b petal-authored publisher pin: tileset id → the `did:key` its
    /// streamed header must be signed by (installed > petal-authored > TOFU).
    /// See `src/AGENTS.md` §petal_binding "Publisher pin".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tileset_publisher_dids: BTreeMap<String, String>,
}

/// Length of an Ed25519 `did:key` (`did:key:z6Mk` + 44 more base58btc chars).
const ED25519_DID_KEY_LEN: usize = 56;

/// Whether `did` has the Ed25519 `did:key` grammar (`did:key:z6Mk…`,
/// base58btc, 56 chars). Grammar only — the DB writer also decodes the key.
pub fn is_ed25519_did_key_grammar(did: &str) -> bool {
    const BASE58_ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    did.len() == ED25519_DID_KEY_LEN
        && did.starts_with("did:key:z6Mk")
        && did["did:key:z".len()..]
            .bytes()
            .all(|b| BASE58_ALPHABET.contains(&b))
}

impl TerrainConfig {
    /// Sanitized, hexon-bound-clamped world scale (FR-5); falls back to
    /// `1.0` on bad JSON, clamps into `scale_bounds` when present.
    pub fn effective_world_scale(&self) -> f64 {
        crate::scale::clamp_world_scale_to_bounds(self.world_scale, self.scale_bounds)
    }

    /// RP-4b strictest reader: every publisher pin names a tileset this
    /// config binds and is an Ed25519 `did:key`; `Err` names the first bad pin.
    pub fn validate_publisher_pins(&self) -> Result<(), String> {
        for (tileset_id, did) in &self.tileset_publisher_dids {
            if !self.tileset_hexon_uris.iter().any(|uri| uri == tileset_id) {
                return Err(format!(
                    "publisher pin for {tileset_id:?} names a tileset the petal does not bind"
                ));
            }
            if !is_ed25519_did_key_grammar(did) {
                return Err(format!(
                    "publisher pin for {tileset_id:?} is not an Ed25519 did:key"
                ));
            }
        }
        Ok(())
    }

    /// The petal-authored publisher pin for `tileset_id`, when one is set.
    pub fn publisher_pin(&self, tileset_id: &str) -> Option<&str> {
        self.tileset_publisher_dids
            .get(tileset_id)
            .map(String::as_str)
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
            tileset_publisher_dids: BTreeMap::new(),
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

    /// RP-4b: a publisher pin must bind a tileset the config names and carry
    /// the Ed25519 did:key grammar; anything else is refused, never ignored.
    #[test]
    fn publisher_pins_are_validated_against_bound_tilesets_and_did_key_grammar() {
        let real_did = fe_format::verifying_key_to_did(
            &ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key(),
        );
        let config = |pins: &[(&str, &str)]| TerrainConfig {
            tileset_hexon_uris: vec!["ts-a".into()],
            tileset_publisher_dids: pins
                .iter()
                .map(|(id, did)| (id.to_string(), did.to_string()))
                .collect(),
            ..TerrainConfig::default()
        };
        let truncated = &real_did[..real_did.len() - 1];
        let non_base58 = format!("{}0", &real_did[..real_did.len() - 1]);
        for (pins, ok) in [
            (vec![], true),
            (vec![("ts-a", real_did.as_str())], true),
            (vec![("ts-b", real_did.as_str())], false),
            (vec![("ts-a", "did:key:not-a-key")], false),
            (vec![("ts-a", truncated)], false),
            (vec![("ts-a", non_base58.as_str())], false),
            (vec![("ts-a", "did:web:example.com")], false),
        ] {
            assert_eq!(
                config(&pins).validate_publisher_pins().is_ok(),
                ok,
                "{pins:?}"
            );
        }

        // The pin round-trips through the petal JSON and is omitted when empty.
        let pinned = config(&[("ts-a", real_did.as_str())]);
        let value = serde_json::to_value(&pinned).unwrap();
        let parsed: TerrainConfig = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.publisher_pin("ts-a"), Some(real_did.as_str()));
        assert!(serde_json::to_value(TerrainConfig::default())
            .unwrap()
            .get("tileset_publisher_dids")
            .is_none());
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
