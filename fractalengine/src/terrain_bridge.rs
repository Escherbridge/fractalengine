//! Bridges DB petal-terrain results and UI hexon ops into fe-terrain. See AGENTS.md §petal-map.

use bevy::prelude::*;

use fe_runtime::messages::DbResult;
use fe_terrain::petal_binding::{
    terrain_config_from_petal_json, PetalTerrainAssignment, SharedTilesetRegistry,
    TerrainAssignmentMsg,
};
use fe_terrain::splat::{view_mode_from_terrain_json, TerrainViewModeMsg};
use fe_ui::plugin::{
    ActiveDialog, HexonOp, InstalledTilesetDto, PendingHexonOps, StorageInfoDto, UiManager,
};

/// Forwards `DbResult::PetalTerrainLoaded` to the terrain runtime as an assignment.
pub fn bridge_petal_terrain(
    mut db_results: MessageReader<DbResult>,
    mut assignments: MessageWriter<TerrainAssignmentMsg>,
    mut view_modes: MessageWriter<TerrainViewModeMsg>,
) {
    for result in db_results.read() {
        if let DbResult::PetalTerrainLoaded { petal_id, terrain } = result {
            let config = terrain.as_ref().and_then(terrain_config_from_petal_json);
            assignments.write(TerrainAssignmentMsg(PetalTerrainAssignment {
                petal_id: petal_id.clone(),
                config,
            }));
            // view_mode is an additive JSON field TerrainConfig drops — forward raw.
            view_modes.write(TerrainViewModeMsg(view_mode_from_terrain_json(
                terrain.as_ref(),
            )));
        }
    }
}

/// Drains fe-ui's queued hexon registry ops and refreshes the Hexon Manager dialog.
pub fn drain_hexon_ops(
    mut ops: ResMut<PendingHexonOps>,
    registry: Option<Res<SharedTilesetRegistry>>,
    mut ui_mgr: ResMut<UiManager>,
) {
    if ops.0.is_empty() {
        return;
    }
    let Some(registry) = registry else {
        tracing::warn!("Hexon ops dropped — tileset registry unavailable");
        ops.0.clear();
        return;
    };
    let reg = &registry.0;

    let mut refresh = false;
    for op in ops.0.drain(..) {
        match op {
            HexonOp::Install(path) => match std::fs::read(&path) {
                Ok(bytes) => match reg.install(&bytes) {
                    Ok(installed) => {
                        tracing::info!(hexon_id = %installed.hexon_id, "Installed hexon tileset");
                        refresh = true;
                    }
                    Err(e) => tracing::warn!("Hexon install failed for {}: {e}", path.display()),
                },
                Err(e) => tracing::warn!("Could not read hexon file {}: {e}", path.display()),
            },
            HexonOp::Remove(id) => {
                match reg.remove(&id) {
                    Ok(()) => tracing::info!(hexon_id = %id, "Removed hexon tileset"),
                    Err(e) => tracing::warn!("Hexon remove failed for {id}: {e}"),
                }
                refresh = true;
            }
            HexonOp::SetSeeding(id, enabled) => {
                if let Err(e) = reg.set_seeding(&id, enabled) {
                    tracing::warn!("Hexon set_seeding failed for {id}: {e}");
                }
                refresh = true;
            }
            HexonOp::RefreshList => refresh = true,
        }
    }

    if refresh {
        if let ActiveDialog::HexonManager {
            ref mut installed_tilesets,
            ref mut storage_info,
            ref mut loading,
            ..
        } = ui_mgr.active_dialog
        {
            let store = reg.store();
            *installed_tilesets = store
                .list_installed()
                .into_iter()
                .map(|t| InstalledTilesetDto {
                    hexon_id: t.hexon_id,
                    region_name: t.region_name,
                    bounds: t.bounds,
                    zoom_range: t.zoom_range,
                    tile_count: t.tile_count,
                    size_bytes: t.size_bytes,
                    seeding_enabled: t.seeding_enabled,
                    installed_at: t.installed_at.to_rfc3339(),
                })
                .collect();
            *storage_info = StorageInfoDto {
                base_dir: store.base_dir().to_string_lossy().to_string(),
                total_bytes: store.total_disk_usage(),
                count: store.list_installed().len() as u32,
            };
            *loading = false;
        }
    }
}
