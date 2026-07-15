//! Handler for per-petal terrain docs feeding the map picker. See ../AGENTS.md §db-results.

use crate::navigation_manager::NavigationManager;
use crate::terrain_map::PetalMapState;

/// `PetalTerrainLoaded`: only the active petal's terrain drives the map picker state.
pub(super) fn handle_petal_terrain_loaded(
    petal_id: &str,
    terrain: &Option<serde_json::Value>,
    nav: &NavigationManager,
    petal_map: &mut PetalMapState,
) {
    if nav.active_petal_id.as_deref() != Some(petal_id) {
        return;
    }
    petal_map.petal_id = Some(petal_id.to_string());
    petal_map.tileset_ids = terrain
        .as_ref()
        .and_then(|t| t.get("tileset_hexon_uris"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Restore the stored world scale (drives the settings slider + camera).
    petal_map.world_scale = terrain
        .as_ref()
        .and_then(|t| t.get("world_scale"))
        .and_then(|v| v.as_f64())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    // Hexon-authoritative clamp bounds (scale orchestration track); see fe-ui/src/verse_manager/AGENTS.md.
    petal_map.scale_bounds = terrain
        .as_ref()
        .and_then(|t| t.get("scale_bounds"))
        .and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok());
    // Keep the raw doc for the GIS Layer Manager's mutate-and-round-trip flow.
    petal_map.terrain_json = terrain.clone();
    petal_map.loaded = true;
}
