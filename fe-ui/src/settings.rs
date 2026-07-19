//! Application settings surface (D-78): the single home for the residency
//! ledger's tunable knobs — render distance, mesh/entity/stamp budget ceilings,
//! tile source mode, and camera prefs. In-memory now; RON persistence under the
//! platform config dir is a seam (see the `// TODO(ultrapilot)` below). The
//! hardcoded `const` caps (`MAX_MESH_INSTANCES`, `MAX_PETAL_NODES`,
//! `MAX_STAMPS_PER_PETAL`) are routed through this resource with the current
//! constants as defaults. See `fe-ui/src/AGENTS.md` §app-settings.

use bevy::prelude::*;

/// Default configurable entity ceiling. Mirrors `verse_manager::spawn::MAX_PETAL_NODES`
/// (that const is the module-private HARD backstop; this is the soft default).
const DEFAULT_ENTITY_CAP: usize = 10_000;

/// Default configurable per-petal stamp ceiling. Mirrors
/// `verse_manager::path_asset_materialize::MAX_STAMPS_PER_PETAL` (the hard backstop).
const DEFAULT_STAMP_CEILING: usize = 65_536;

/// Where terrain tiles are sourced from (D-78). `Offline` deliberately skips the
/// online-origin disk cache; `Hybrid` prefers local then falls back to network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileSourceMode {
    Offline,
    Online,
    #[default]
    Hybrid,
}

/// User-tunable application settings (D-78). The global defaults for the
/// residency ledger; `PetalManifest.render_distance` is the per-petal override
/// (not yet wired — see the residency-ledger seam in the spawners).
#[derive(Resource, Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Global render distance (world units); soft residency horizon. `0` halts
    /// distance-ranked spawning entirely. Per-petal override: `PetalManifest.render_distance`.
    pub render_distance: f32,
    /// Live mirror of `MeshInstanceBudget.ceiling` (synced by
    /// `sync_app_settings_to_mesh_budget`). Seeded from `MAX_MESH_INSTANCES`.
    pub mesh_budget_ceiling: usize,
    /// Configurable soft cap on scene entities per petal (hard-backstopped by
    /// `MAX_PETAL_NODES` at the spawn sites).
    pub entity_cap: usize,
    /// Configurable soft cap on path-asset stamps per petal pass (hard-backstopped
    /// by `MAX_STAMPS_PER_PETAL`).
    pub stamp_ceiling: usize,
    /// Terrain tile source mode.
    pub tile_mode: TileSourceMode,
    /// Camera look/orbit sensitivity multiplier (consumed by the camera stack; seam).
    pub camera_sensitivity: f32,
    /// Camera zoom speed multiplier.
    pub camera_zoom_speed: f32,
    /// Camera focus easing factor (0 = snap, 1 = full easing).
    pub camera_easing: f32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            render_distance: 500.0,
            mesh_budget_ceiling: crate::plugin::MAX_MESH_INSTANCES,
            entity_cap: DEFAULT_ENTITY_CAP,
            stamp_ceiling: DEFAULT_STAMP_CEILING,
            tile_mode: TileSourceMode::Hybrid,
            camera_sensitivity: 1.0,
            camera_zoom_speed: 1.0,
            camera_easing: 1.0,
        }
    }
}

// TODO(ultrapilot): load/save `AppSettings` as RON under the platform config dir
// (`~/.config/fractalengine/settings.ron` per D-78 / the render_distance_lod
// design). Debounced save on change, defaults on missing/corrupt. In-memory only
// for now — the serde derives above make this a drop-in when the loader lands.

/// Mirrors `AppSettings.mesh_budget_ceiling` into the live `MeshInstanceBudget`
/// gate so a settings change re-ceilings the watchdog without a restart. Cheap
/// (single compare) — runs every frame. See AGENTS.md §app-settings.
pub fn sync_app_settings_to_mesh_budget(
    settings: Res<AppSettings>,
    mut budget: ResMut<crate::plugin::MeshInstanceBudget>,
) {
    if budget.ceiling != settings.mesh_budget_ceiling {
        budget.ceiling = settings.mesh_budget_ceiling;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_seeds_ceilings_from_the_hard_caps() {
        let s = AppSettings::default();
        assert_eq!(s.render_distance, 500.0);
        assert_eq!(s.mesh_budget_ceiling, crate::plugin::MAX_MESH_INSTANCES);
        assert_eq!(s.entity_cap, DEFAULT_ENTITY_CAP);
        assert_eq!(s.stamp_ceiling, DEFAULT_STAMP_CEILING);
        assert_eq!(s.tile_mode, TileSourceMode::Hybrid);
    }

    #[test]
    fn serde_roundtrips_through_json() {
        let mut s = AppSettings::default();
        s.render_distance = 750.0;
        s.entity_cap = 2_000;
        s.tile_mode = TileSourceMode::Offline;
        let json = serde_json::to_value(&s).expect("serialize");
        assert_eq!(json["tile_mode"], serde_json::json!("offline"));
        let back: AppSettings = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.render_distance, 750.0);
        assert_eq!(back.entity_cap, 2_000);
        assert_eq!(back.tile_mode, TileSourceMode::Offline);
    }

    #[test]
    fn serde_default_fills_missing_fields() {
        // `#[serde(default)]` means a partial doc (forward/backward compat) loads.
        let partial = serde_json::json!({ "render_distance": 300.0 });
        let s: AppSettings = serde_json::from_value(partial).expect("partial deserialize");
        assert_eq!(s.render_distance, 300.0);
        assert_eq!(s.entity_cap, DEFAULT_ENTITY_CAP);
        assert_eq!(s.mesh_budget_ceiling, crate::plugin::MAX_MESH_INSTANCES);
    }
}
