//! Terrain map — which tileset(s) the active petal uses as its map, plus the
//! hexon registry op queue and tileset event draining. See `fe-ui/src/AGENTS.md`
//! §terrain-map.

pub mod dto;
pub mod events;
pub mod manifest;

pub use dto::{
    AvailableTilesetDto, DownloadProgress, DownloadStatus, HexonManagerTab, InstalledTilesetDto,
    StorageInfoDto,
};
pub(crate) use events::drain_tileset_events;
pub use manifest::{ManifestHexonEntry, PetalManifest};

use bevy::prelude::*;

/// Which tileset(s) the active petal uses as its map, plus its world scale.
#[derive(Debug, Clone, Resource)]
pub struct PetalMapState {
    pub petal_id: Option<String>,
    pub tileset_ids: Vec<String>,
    pub loaded: bool,
    /// World units per real meter for the active map (`1.0` = human scale).
    pub world_scale: f64,
    /// Hexon-declared `[min, max]` clamp bounds for `world_scale` (FR-6),
    /// read back from `terrain_json["scale_bounds"]`. `None` = unbounded.
    pub scale_bounds: Option<[f64; 2]>,
    /// The raw, last-loaded petal terrain JSON (fe-terrain's `TerrainConfig`
    /// serde shape). Kept verbatim (not just the derived fields above) so the
    /// GIS Layer Manager (`crate::gis`) can mutate a single field (e.g. one
    /// layer's `visible`/`opacity`) and round-trip via `SetPetalTerrain`
    /// without clobbering unrelated fields. `None` when no map is assigned.
    pub terrain_json: Option<serde_json::Value>,
}

impl Default for PetalMapState {
    fn default() -> Self {
        Self {
            petal_id: None,
            tileset_ids: Vec::new(),
            loaded: false,
            world_scale: 1.0,
            scale_bounds: None,
            terrain_json: None,
        }
    }
}

/// Registry operation requested by the UI; drained by the main binary.
#[derive(Debug, Clone)]
pub enum HexonOp {
    Install(std::path::PathBuf),
    Remove(String),
    SetSeeding(String, bool),
    RefreshList,
}

/// Queue of pending registry ops (fe-ui has no TilesetRegistry access).
#[derive(Debug, Default, Resource)]
pub struct PendingHexonOps(pub Vec<HexonOp>);

/// Requests the petal's terrain config when the active petal changes.
pub(crate) fn load_petal_terrain_on_nav_change(
    nav: Res<crate::navigation_manager::NavigationManager>,
    mut petal_map: ResMut<PetalMapState>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
) {
    if petal_map.petal_id == nav.active_petal_id {
        return;
    }
    petal_map.petal_id = nav.active_petal_id.clone();
    petal_map.tileset_ids.clear();
    petal_map.loaded = false;
    // Reset to human scale pending the load; PetalTerrainLoaded restores the stored value.
    petal_map.world_scale = 1.0;
    petal_map.scale_bounds = None;
    petal_map.terrain_json = None;
    if let Some(petal_id) = nav.active_petal_id.clone() {
        if db_sender
            .0
            .send(fe_runtime::messages::DbCommand::GetPetalTerrain { petal_id })
            .is_err()
        {
            bevy::log::warn!("db_sender channel closed — GetPetalTerrain not dispatched");
        }
    }
}

/// Mirrors the active petal's world scale into the renderer's [`CameraScaleSettings`]
/// so the orbit camera + far plane adapt live (and on restart). Defensive: no-op
/// when the renderer resource is absent (headless / tests). See AGENTS.md §terrain-map.
pub(crate) fn sync_camera_scale_from_petal_map(
    petal_map: Res<PetalMapState>,
    settings: Option<ResMut<fe_renderer::camera::CameraScaleSettings>>,
) {
    let Some(mut settings) = settings else { return };
    let target = petal_map.world_scale as f32;
    if (settings.world_scale - target).abs() > 1e-9 {
        settings.world_scale = target;
    }
}

/// Builds petal terrain JSON matching fe-terrain's `TerrainConfig` serde shape.
/// Bounds are `[min_lat, min_lon, max_lat, max_lon]`; origin = bounds center.
/// `world_scale` is world units per real meter (`1.0` = human scale). fe-ui builds
/// this via `serde_json` only — it must not depend on fe-terrain (boundary rule).
///
/// `scale_bounds` (FR-6) is the hexon-declared `[min, max]` clamp range, carried
/// through verbatim as the `scale_bounds` JSON key so `TerrainConfig` (fe-terrain)
/// can clamp; `None` omits the key (unbounded).
pub(crate) fn tileset_to_terrain_json(
    ts: &InstalledTilesetDto,
    world_scale: f64,
    scale_bounds: Option<[f64; 2]>,
) -> serde_json::Value {
    if ts.bounds == [0.0, 0.0, 0.0, 0.0] {
        bevy::log::warn!(
            "tileset {} has unpopulated bounds; origin will default to (0,0) (Gulf of Guinea)",
            ts.hexon_id
        );
    }
    let world_scale = if world_scale.is_finite() && world_scale > 0.0 {
        world_scale
    } else {
        1.0
    };
    let mut json = serde_json::json!({
        "enabled": true,
        "origin": {
            "origin_lat": (ts.bounds[0] + ts.bounds[2]) / 2.0,
            "origin_lon": (ts.bounds[1] + ts.bounds[3]) / 2.0,
            "origin_ele": 0.0,
        },
        "tile_source_url": "",
        "elevation_source": "terrain_rgb",
        "max_zoom": ts.zoom_range.1,
        "min_zoom": ts.zoom_range.0,
        "cache_dir": "terrain_cache",
        "layers": [
            {"name": "satellite", "visible": true},
            {"name": "terrain", "visible": true},
        ],
        "tileset_hexon_uris": [ts.hexon_id],
        "tile_source_mode": "hybrid",
        "world_scale": world_scale,
    });
    if let Some(bounds) = scale_bounds {
        json["scale_bounds"] = serde_json::json!(bounds);
    }
    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_map::dto::InstalledTilesetDto;

    fn dto() -> InstalledTilesetDto {
        InstalledTilesetDto {
            hexon_id: "ts-1".into(),
            region_name: "PNW".into(),
            bounds: [45.0, -122.0, 46.0, -121.0],
            zoom_range: (8, 12),
            tile_count: 4,
            size_bytes: 0,
            seeding_enabled: false,
            installed_at: String::new(),
        }
    }

    #[test]
    fn terrain_json_carries_world_scale() {
        let v = tileset_to_terrain_json(&dto(), 0.001, None);
        assert_eq!(v["world_scale"], serde_json::json!(0.001));
        assert_eq!(v["tileset_hexon_uris"], serde_json::json!(["ts-1"]));
    }

    #[test]
    fn terrain_json_sanitizes_bad_scale() {
        assert_eq!(tileset_to_terrain_json(&dto(), 0.0, None)["world_scale"], serde_json::json!(1.0));
        assert_eq!(tileset_to_terrain_json(&dto(), -3.0, None)["world_scale"], serde_json::json!(1.0));
        assert_eq!(
            tileset_to_terrain_json(&dto(), f64::NAN, None)["world_scale"],
            serde_json::json!(1.0)
        );
    }

    #[test]
    fn terrain_json_carries_scale_bounds_when_present() {
        let v = tileset_to_terrain_json(&dto(), 0.001, Some([0.0001, 0.01]));
        assert_eq!(v["scale_bounds"], serde_json::json!([0.0001, 0.01]));
    }

    #[test]
    fn terrain_json_omits_scale_bounds_when_absent() {
        let v = tileset_to_terrain_json(&dto(), 0.001, None);
        assert!(v.get("scale_bounds").is_none());
    }

    #[test]
    fn petal_map_state_defaults_to_human_scale() {
        let state = PetalMapState::default();
        assert_eq!(state.world_scale, 1.0);
        assert!(state.scale_bounds.is_none());
    }
}
