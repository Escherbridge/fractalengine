//! Terrain view mode (mesh / splats / hybrid); see `src/splat/AGENTS.md` §view_mode.

use serde::{Deserialize, Serialize};

/// Which representation of the terrain to render for the active petal.
///
/// Parsed from the petal terrain JSON's additive `"view_mode"` field. Doubles as a
/// Bevy resource under the `render` feature (default [`TerrainViewMode::Mesh`], so
/// pre-feature petals and unset configs keep the classic mesh view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "render", derive(bevy::prelude::Resource))]
#[serde(rename_all = "snake_case")]
pub enum TerrainViewMode {
    /// Classic triangle mesh only (default).
    #[default]
    Mesh,
    /// Synthesized splats only.
    Splats,
    /// Mesh near the camera, splats far (scale-aware distance threshold).
    Hybrid,
}

impl TerrainViewMode {
    /// Map a case-insensitive string to a mode; unknown strings fall back to `Mesh`.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "splats" => Self::Splats,
            "hybrid" => Self::Hybrid,
            "mesh" => Self::Mesh,
            _ => Self::default(),
        }
    }
}

/// Read the additive `"view_mode"` string from a petal terrain JSON value.
///
/// Returns [`TerrainViewMode::Mesh`] for `None`/missing/non-string/unknown — the
/// same graceful default serde applies, so no config round-trip ever fails.
pub fn view_mode_from_terrain_json(terrain: Option<&serde_json::Value>) -> TerrainViewMode {
    terrain
        .and_then(|v| v.get("view_mode"))
        .and_then(|v| v.as_str())
        .map(TerrainViewMode::from_str_or_default)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_mesh() {
        assert_eq!(TerrainViewMode::default(), TerrainViewMode::Mesh);
    }

    #[test]
    fn parses_known_modes_case_insensitively() {
        assert_eq!(
            TerrainViewMode::from_str_or_default("mesh"),
            TerrainViewMode::Mesh
        );
        assert_eq!(
            TerrainViewMode::from_str_or_default("Splats"),
            TerrainViewMode::Splats
        );
        assert_eq!(
            TerrainViewMode::from_str_or_default(" HYBRID "),
            TerrainViewMode::Hybrid
        );
    }

    #[test]
    fn unknown_string_falls_back_to_mesh() {
        assert_eq!(
            TerrainViewMode::from_str_or_default("wireframe"),
            TerrainViewMode::Mesh
        );
    }

    #[test]
    fn json_field_extracted() {
        let v = serde_json::json!({ "enabled": true, "view_mode": "hybrid" });
        assert_eq!(
            view_mode_from_terrain_json(Some(&v)),
            TerrainViewMode::Hybrid
        );
    }

    #[test]
    fn json_missing_or_null_defaults_to_mesh() {
        assert_eq!(view_mode_from_terrain_json(None), TerrainViewMode::Mesh);
        assert_eq!(
            view_mode_from_terrain_json(Some(&serde_json::Value::Null)),
            TerrainViewMode::Mesh
        );
        let v = serde_json::json!({ "enabled": true });
        assert_eq!(view_mode_from_terrain_json(Some(&v)), TerrainViewMode::Mesh);
    }

    #[test]
    fn serde_roundtrips_snake_case() {
        assert_eq!(
            serde_json::to_value(TerrainViewMode::Splats).unwrap(),
            serde_json::json!("splats")
        );
        let parsed: TerrainViewMode = serde_json::from_value(serde_json::json!("hybrid")).unwrap();
        assert_eq!(parsed, TerrainViewMode::Hybrid);
    }
}
