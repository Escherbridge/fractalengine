//! Petal hexon manifest — declares which hexons a petal requires. See
//! `fe-ui/src/AGENTS.md` §terrain-map.

/// A single hexon requirement in a petal's manifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestHexonEntry {
    pub hexon_id: String,
    pub hexon_type: String,
    pub required: bool,
}

/// Parsed petal manifest — mirrors the JSON stored in `petal.hexon_manifest`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PetalManifest {
    #[serde(default)]
    pub hexons: Vec<ManifestHexonEntry>,
    #[serde(default = "default_render_distance")]
    pub render_distance: f32,
    #[serde(default = "default_fallback")]
    pub fallback: String,
}

fn default_render_distance() -> f32 {
    500.0
}

fn default_fallback() -> String {
    "sign".to_string()
}
