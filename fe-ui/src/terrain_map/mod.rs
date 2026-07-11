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

/// Which tileset(s) the active petal uses as its map.
#[derive(Debug, Clone, Default, Resource)]
pub struct PetalMapState {
    pub petal_id: Option<String>,
    pub tileset_ids: Vec<String>,
    pub loaded: bool,
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

/// Builds petal terrain JSON matching fe-terrain's `TerrainConfig` serde shape.
/// Bounds are `[min_lat, min_lon, max_lat, max_lon]`; origin = bounds center.
pub(crate) fn tileset_to_terrain_json(ts: &InstalledTilesetDto) -> serde_json::Value {
    if ts.bounds == [0.0, 0.0, 0.0, 0.0] {
        bevy::log::warn!(
            "tileset {} has unpopulated bounds; origin will default to (0,0) (Gulf of Guinea)",
            ts.hexon_id
        );
    }
    serde_json::json!({
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
    })
}
