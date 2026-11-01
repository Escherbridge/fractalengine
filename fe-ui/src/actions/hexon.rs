//! Hexon/tileset/petal-map action handling. See `fe-ui/src/AGENTS.md`
//! §terrain-map.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::dialogs::ActiveDialog;
use crate::terrain_map::dto::{DownloadProgress, DownloadStatus, InstalledTilesetDto};
use crate::terrain_map::manifest::PetalManifest;
use crate::terrain_map::{
    tileset_to_terrain_json, HexonOp, PendingHexonOps, PendingMapSet, PetalMapState,
    TilesetDownloads,
};

use super::UiManager;

pub(crate) fn install_from_file(
    hexon_ops: &mut PendingHexonOps,
    path: crate::file_dialogs::DialogPath,
) {
    bevy::log::info!("Hexon: install from file {}", path.display());
    hexon_ops.0.push(HexonOp::Install(path));
}

/// Queues a hosted-registry install by URI (PAS-P5 S7b). The caller (the
/// Hexon Manager dialog) only enables this control when
/// `AppSettings.hexon_registry_url` is configured — a blank URI here would
/// otherwise queue an op `drain_hexon_ops` can only fail.
pub(crate) fn install_from_registry(hexon_ops: &mut PendingHexonOps, uri: String) {
    bevy::log::info!("Hexon: install from registry {}", uri);
    hexon_ops.0.push(HexonOp::InstallFromRegistry { uri });
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
        if let Err(e) = sender.0.send(fe_sync::SyncCommand::AdvertiseTilesets {
            verse_id: active_verse_id.unwrap_or_default().to_string(),
            advertisements_json: String::new(), // ignored on the wire — built from the catalog
        }) {
            bevy::log::warn!("sync_sender channel closed — AdvertiseTilesets not dispatched: {e}");
        }
    }
}

/// Starts a P2P tileset download: seeds a [`DownloadProgress`] tracker and
/// sends `DownloadTileset`, which drives meta resolution + every bundle
/// fetch on the sync thread (S3) — no more `RequestChunk` ping-pong from
/// here. The tracker lives in [`TilesetDownloads`], not the dialog, so it
/// survives the Hexon Manager closing (S4 regression fix).
pub(crate) fn start_download(
    downloads: &mut TilesetDownloads,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
    id: String,
) {
    bevy::log::info!("Hexon: start P2P download for tileset {}", id);
    downloads.0.insert(
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
    if let Some(sender) = sync_sender {
        if let Err(e) = sender.0.send(fe_sync::SyncCommand::DownloadTileset {
            tileset_id: id,
            provider: None, // every provider this node has seen advertise it
        }) {
            bevy::log::warn!("sync_sender channel closed — DownloadTileset not dispatched: {e}");
        }
    } else {
        downloads.0.entry(id).and_modify(|dl| {
            dl.status = DownloadStatus::Failed("P2P offline — no sync thread".to_string());
        });
    }
}

pub(crate) fn cancel_download(
    downloads: &mut TilesetDownloads,
    sync_sender: Option<&fe_sync::SyncCommandSenderRes>,
    id: String,
) {
    bevy::log::info!("Hexon: cancel download {}", id);
    if let Some(sender) = sync_sender {
        if let Err(e) = sender.0.send(fe_sync::SyncCommand::CancelTilesetDownload {
            tileset_id: id.clone(),
        }) {
            bevy::log::warn!(
                "sync_sender channel closed — CancelTilesetDownload not dispatched: {e}"
            );
        }
    }
    downloads.0.remove(&id);
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
        if let Err(e) = sender.0.send(fe_sync::SyncCommand::AdvertiseTilesets {
            verse_id: active_verse_id.unwrap_or_default().to_string(),
            advertisements_json: String::new(),
        }) {
            bevy::log::warn!("sync_sender channel closed — AdvertiseTilesets not dispatched: {e}");
        }
    }
}

pub(crate) fn open_storage_dir(ui_mgr: &UiManager) {
    if let ActiveDialog::HexonManager {
        ref storage_info, ..
    } = ui_mgr.active_dialog
    {
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

/// M-03 (petal-map truth): mints its own correlation id so a denial always
/// comes back as `DbResult::PetalTerrainSetFailed` (never the generic,
/// un-routable `Error`), and snapshots the pre-send state into
/// `petal_map.pending_set` so `db_results::terrain::handle_petal_terrain_set_failed`
/// can roll back exactly what this call optimistically changed. The
/// optimistic update and the snapshot are taken from the SAME pre-send
/// values, so a denial restores precisely the row this click changed, not
/// whatever `petal_map` holds by the time the reply arrives.
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
    let tileset_ids = tileset
        .as_ref()
        .map(|ts| vec![ts.hexon_id.clone()])
        .unwrap_or_default();
    let correlation_id = crate::actions::query::next_ui_query_id("petalmap");
    let (previous_tileset_ids, previous_world_scale) = rollback_baseline(petal_map);
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain,
        auth: fe_runtime::messages::CallerAuth::Local,
        correlation_id: Some(correlation_id.clone()),
    }) {
        Ok(()) => {
            // Optimistic local update only after the command is queued; DbResult::PetalTerrainLoaded confirms.
            petal_map.petal_id = Some(petal_id);
            petal_map.tileset_ids = tileset_ids;
            petal_map.pending_set = Some(PendingMapSet {
                correlation_id,
                previous_tileset_ids,
                previous_world_scale,
            });
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
    let correlation_id = crate::actions::query::next_ui_query_id("petalmap");
    let (previous_tileset_ids, previous_world_scale) = rollback_baseline(petal_map);
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain: Some(terrain),
        auth: fe_runtime::messages::CallerAuth::Local,
        correlation_id: Some(correlation_id.clone()),
    }) {
        Ok(()) => {
            petal_map.petal_id = Some(petal_id);
            petal_map.tileset_ids = vec![tileset.hexon_id];
            petal_map.world_scale = clamped_scale;
            petal_map.pending_set = Some(PendingMapSet {
                correlation_id,
                previous_tileset_ids,
                previous_world_scale,
            });
        }
        Err(_) => {
            bevy::log::warn!(
                "db_sender channel closed — SetPetalTerrain (scale) not dispatched; local map state unchanged"
            );
        }
    }
}

/// What a new set rolls back to if denied: the last CONFIRMED state. While
/// an earlier set is still in flight, `petal_map` holds that set's optimistic
/// values, so the earlier snapshot is carried forward instead — otherwise a
/// denied second set "restored" a first set that was never persisted.
fn rollback_baseline(petal_map: &PetalMapState) -> (Vec<String>, f64) {
    match &petal_map.pending_set {
        Some(pending) => (
            pending.previous_tileset_ids.clone(),
            pending.previous_world_scale,
        ),
        None => (petal_map.tileset_ids.clone(), petal_map.world_scale),
    }
}

/// Clamp a world scale into `[min, max]` bounds (FR-6); no fe-terrain dep —
/// fe-ui owns this trivial clamp locally per the layering boundary (C6).
fn clamp_to_bounds(scale: f64, bounds: Option<[f64; 2]>) -> f64 {
    let sanitized = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    match bounds {
        Some([min, max]) if min.is_finite() && max.is_finite() && min > 0.0 && max >= min => {
            sanitized.clamp(min, max)
        }
        _ => sanitized,
    }
}

/// Applies a manifest save: the manifest's `render_distance` is persisted to
/// the petal's durable `petal.render_distance` column (p2p_asset_streaming
/// S6) via a correlated-free local write (`CallerAuth::Local` — the DB thread
/// resolves the caller's own role). This REPLACES the prior session-override
/// write: a manifest save is a durable, shared setting, not a local knob —
/// see `fe-ui/src/AGENTS.md` §app-settings. The session-only override
/// (`PetalRenderOverride`, for a viewer on a weak machine) is a separate,
/// explicit action (`UiAction::PetalSetRenderOverride`).
pub(crate) fn manifest_save(
    db_sender: &DbCommandSender,
    petal_id: String,
    manifest: PetalManifest,
) {
    bevy::log::info!(
        "PetalManifest: save requested for petal {petal_id} ({} hexons)",
        manifest.hexons.len()
    );
    if let Err(e) = db_sender.0.send(DbCommand::SetPetalRenderDistance {
        petal_id,
        render_distance: Some(manifest.render_distance),
        auth: fe_runtime::messages::CallerAuth::Local,
        correlation_id: None,
    }) {
        bevy::log::warn!("db_sender channel closed — SetPetalRenderDistance not dispatched: {e}");
    }
}

/// Sets (or clears) the session-only residency override for `petal_id` — a
/// local-only knob for a viewer on a weak machine, never persisted, cleared
/// on petal change (`clear_petal_render_override_on_nav_change`). Beats both
/// the petal's durable column and the global setting
/// (`settings::effective_render_distance`).
pub(crate) fn set_render_override(
    render_override: &mut crate::settings::PetalRenderOverride,
    petal_id: String,
    render_distance: Option<f32>,
) {
    *render_override = crate::settings::PetalRenderOverride {
        petal_id: Some(petal_id),
        render_distance,
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// S6 flow test: `manifest_save` persists to the petal column — the
    /// `DbCommand` on the channel is the observable outcome at the thread
    /// seam (campaign plan §3 S6 "fe-ui" flow tests).
    #[test]
    fn manifest_save_persists_to_the_petal_column() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let db_sender = DbCommandSender(tx);
        let manifest = PetalManifest {
            render_distance: 1234.0,
            ..Default::default()
        };

        manifest_save(&db_sender, "petal-1".to_string(), manifest);

        match rx.try_recv().expect("a DbCommand was sent") {
            DbCommand::SetPetalRenderDistance {
                petal_id,
                render_distance,
                auth,
                correlation_id,
            } => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(render_distance, Some(1234.0));
                assert!(matches!(auth, fe_runtime::messages::CallerAuth::Local));
                assert_eq!(correlation_id, None);
            }
            other => panic!("expected SetPetalRenderDistance, got {other:?}"),
        }
        assert!(
            rx.try_recv().is_err(),
            "exactly one command, no session-override side channel"
        );
    }
}
