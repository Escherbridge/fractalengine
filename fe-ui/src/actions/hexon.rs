//! Hexon/tileset/petal-map action handling. See `fe-ui/src/AGENTS.md`
//! §terrain-map.

use std::path::PathBuf;

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::dialogs::ActiveDialog;
use crate::terrain_map::dto::{DownloadProgress, DownloadStatus, InstalledTilesetDto};
use crate::terrain_map::manifest::PetalManifest;
use crate::terrain_map::{tileset_to_terrain_json, HexonOp, PendingHexonOps, PetalMapState};

use super::UiManager;

pub(crate) fn install_from_file(hexon_ops: &mut PendingHexonOps, path: PathBuf) {
    bevy::log::info!("Hexon: install from file {:?}", path);
    hexon_ops.0.push(HexonOp::Install(path));
}

pub(crate) fn remove_tileset(hexon_ops: &mut PendingHexonOps, id: String) {
    bevy::log::info!("Hexon: remove tileset {}", id);
    hexon_ops.0.push(HexonOp::Remove(id));
}

pub(crate) fn toggle_seeding(
    hexon_ops: &mut PendingHexonOps,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
    active_verse_id: Option<&str>,
    id: String,
    enabled: bool,
) {
    bevy::log::info!("Hexon: toggle seeding {}={}", id, enabled);
    hexon_ops.0.push(HexonOp::SetSeeding(id, enabled));
    // After toggling, re-advertise to peers
    if let Some(sender) = sync_sender {
        sender
            .0
            .send(fe_sync::SyncCommand::AdvertiseTilesets {
                verse_id: active_verse_id.unwrap_or_default().to_string(),
                advertisements_json: String::new(), // refreshed by terrain layer
            })
            .ok();
    }
}

pub(crate) fn start_download(
    ui_mgr: &mut UiManager,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
    id: String,
) {
    bevy::log::info!("Hexon: start P2P download for tileset {}", id);
    // Initialize a download tracker in the dialog state
    if let ActiveDialog::HexonManager {
        ref mut download_progress,
        ..
    } = ui_mgr.active_dialog
    {
        download_progress.insert(
            id.clone(),
            DownloadProgress {
                tileset_id: id.clone(),
                chunks_received: 0,
                total_chunks: 0,
                bytes_received: 0,
                total_bytes_estimate: 0,
                status: DownloadStatus::Queued,
            },
        );
    }
    // Request metadata from any peer that has it
    if let Some(sender) = sync_sender {
        sender
            .0
            .send(fe_sync::SyncCommand::RequestTilesetMeta {
                peer_id: String::new(), // sync thread picks best peer
                tileset_id: id,
            })
            .ok();
    }
}

pub(crate) fn cancel_download(
    ui_mgr: &mut UiManager,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
    id: String,
) {
    bevy::log::info!("Hexon: cancel download {}", id);
    if let Some(sender) = sync_sender {
        sender
            .0
            .send(fe_sync::SyncCommand::CancelTilesetDownload {
                tileset_id: id.clone(),
            })
            .ok();
    }
    // Update status in dialog
    if let ActiveDialog::HexonManager {
        ref mut download_progress,
        ..
    } = ui_mgr.active_dialog
    {
        download_progress.remove(&id);
    }
}

pub(crate) fn refresh_list(
    hexon_ops: &mut PendingHexonOps,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
    active_verse_id: Option<&str>,
) {
    bevy::log::info!("Hexon: refresh tileset list");
    hexon_ops.0.push(HexonOp::RefreshList);
    // Re-advertise our tilesets to trigger peer exchange
    if let Some(sender) = sync_sender {
        sender
            .0
            .send(fe_sync::SyncCommand::AdvertiseTilesets {
                verse_id: active_verse_id.unwrap_or_default().to_string(),
                advertisements_json: String::new(),
            })
            .ok();
    }
}

pub(crate) fn open_storage_dir(ui_mgr: &UiManager) {
    if let ActiveDialog::HexonManager { ref storage_info, .. } = ui_mgr.active_dialog {
        let dir = &storage_info.base_dir;
        if !dir.is_empty() {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(dir).spawn();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
            }
        }
    }
}

pub(crate) fn set_petal_map(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    petal_id: String,
    tileset: Option<InstalledTilesetDto>,
) {
    // Carry over the petal's current world scale when (re)assigning a map.
    let world_scale = petal_map.world_scale;
    let scale_bounds = petal_map.scale_bounds;
    let terrain = tileset
        .as_ref()
        .map(|ts| tileset_to_terrain_json(ts, world_scale, scale_bounds));
    let tileset_ids = tileset.as_ref().map(|ts| vec![ts.hexon_id.clone()]).unwrap_or_default();
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain,
    }) {
        Ok(()) => {
            // Optimistic local update only after the command is queued; DbResult::PetalTerrainLoaded confirms.
            petal_map.petal_id = Some(petal_id);
            petal_map.tileset_ids = tileset_ids;
        }
        Err(_) => {
            bevy::log::warn!(
                "db_sender channel closed — SetPetalTerrain not dispatched; local map state unchanged"
            );
        }
    }
}

/// Persist a new world scale on the active petal's map (rebuilds terrain JSON).
/// Live-apply is free: SetPetalTerrain → PetalTerrainLoaded → apply_terrain_assignments
/// respawns chunks; the camera adapts via the CameraScaleSettings sync system.
///
/// FR-6: `world_scale` is clamped into `petal_map.scale_bounds` (the hexon's
/// declared bounds, surfaced via `terrain_json["scale_bounds"]`) before being
/// persisted or applied locally — the clamped value is what gets surfaced
/// back to the caller via `petal_map.world_scale` (clamp + feedback, OQ-2).
pub(crate) fn set_petal_map_scale(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    petal_id: String,
    tileset: InstalledTilesetDto,
    world_scale: f64,
) {
    let clamped_scale = clamp_to_bounds(world_scale, petal_map.scale_bounds);
    let terrain = tileset_to_terrain_json(&tileset, clamped_scale, petal_map.scale_bounds);
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain: Some(terrain),
    }) {
        Ok(()) => {
            petal_map.petal_id = Some(petal_id);
            petal_map.tileset_ids = vec![tileset.hexon_id];
            petal_map.world_scale = clamped_scale;
        }
        Err(_) => {
            bevy::log::warn!(
                "db_sender channel closed — SetPetalTerrain (scale) not dispatched; local map state unchanged"
            );
        }
    }
}

/// Clamp a world scale into `[min, max]` bounds (FR-6); no fe-terrain dep —
/// fe-ui owns this trivial clamp locally per the layering boundary (C6).
fn clamp_to_bounds(scale: f64, bounds: Option<[f64; 2]>) -> f64 {
    let sanitized = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    match bounds {
        Some([min, max]) if min.is_finite() && max.is_finite() && min > 0.0 && max >= min => {
            sanitized.clamp(min, max)
        }
        _ => sanitized,
    }
}

pub(crate) fn manifest_save(petal_id: String, manifest: PetalManifest) {
    bevy::log::info!(
        "PetalManifest: save requested for petal {petal_id} ({} hexons)",
        manifest.hexons.len()
    );
    // TODO: PATCH /api/v1/petals/{petal_id}/manifest
}

pub(crate) fn manifest_open(ui_mgr: &mut UiManager, petal_id: String, petal_name: String) {
    bevy::log::info!("PetalManifest: open dialog for petal {petal_id} ({petal_name})");
    ui_mgr.active_dialog = ActiveDialog::PetalManifest {
        petal_id,
        petal_name,
        manifest: PetalManifest::default(),
        available_hexon_ids: Vec::new(),
        add_hexon_id_buf: String::new(),
        add_hexon_type_buf: String::new(),
        render_distance_buf: "500".to_string(),
        dirty: false,
    };
}
