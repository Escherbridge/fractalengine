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
        grant: fe_runtime::terrain_grant::TerrainWriteGrant::Local,
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
        grant: fe_runtime::terrain_grant::TerrainWriteGrant::Local,
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

/// Clamp a world scale into `[min, max]` bounds (FR-6); delegates to `fe_format::scale`.
fn clamp_to_bounds(scale: f64, bounds: Option<[f64; 2]>) -> f64 {
    fe_format::scale::clamp_world_scale_to_bounds(scale, bounds)
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

pub(crate) fn manifest_open(ui_mgr: &mut UiManager, petal_id: String, petal_name: String, scope: Option<String>) {
    bevy::log::info!("PetalManifest: open dialog for petal {petal_id} ({petal_name})");
    crate::native_accessibility::surface::transition();
    ui_mgr.active_dialog = ActiveDialog::PetalManifest {
        petal_id,
        petal_name,
        manifest: PetalManifest::default(),
        available_hexon_ids: Vec::new(),
        add_hexon_id_buf: String::new(),
        add_hexon_type_buf: String::new(),
        requirements_state: crate::dialogs::requirements_state::RequirementsState::new(scope),
        dirty: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_save_requires_fenced_load_and_exact_readback() {
        use crate::dialogs::requirements_state as state;
        use crate::navigation_manager::NavigationManager;
        use fe_runtime::petal_requirements::PetalRequirementsSnapshot;
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "V");
        nav.navigate_to_fractal("f1", "F");
        nav.navigate_to_petal("p1");
        let mut ui = UiManager::default();
        let (tx, rx) = crossbeam::channel::unbounded();
        manifest_open(&mut ui, "p1".into(), "P".into(), nav.active_scope());
        state::request_if_needed(&mut ui, &nav, &tx, 0.0);
        let DbCommand::GetPetalRequirements {
            correlation_id: load,
            ..
        } = rx.recv().unwrap()
        else {
            panic!("fenced read required")
        };
        let initial = PetalRequirementsSnapshot {
            petal_id: "p1".into(),
            scope: nav.active_scope().unwrap(),
            requirements: PetalManifest::default(),
            document: None,
            can_manage: true,
        };
        state::apply(&mut ui, &nav, "unrelated", Ok(&initial));
        assert!(state::save(&mut ui, &nav, &tx, "p1", PetalManifest::default(), 0.1).is_err());
        state::apply(&mut ui, &nav, &load, Ok(&initial));
        let mut draft = PetalManifest::default();
        draft.hexons.push(crate::terrain_map::ManifestHexonEntry {
            hexon_id: "model-v1".into(),
            hexon_type: "Model".into(),
            required: true,
        });
        if let ActiveDialog::PetalManifest {
            manifest, dirty, ..
        } = &mut ui.active_dialog
        {
            *manifest = draft.clone();
            *dirty = true;
        }
        state::save(&mut ui, &nav, &tx, "p1", draft.clone(), 1.0).unwrap();
        let DbCommand::SetPetalRequirements {
            expected,
            correlation_id: save,
            ..
        } = rx.recv().unwrap()
        else {
            panic!("only one checked save")
        };
        assert_eq!(expected, initial);
        let saved = PetalRequirementsSnapshot {
            requirements: draft.clone(),
            document: Some(draft.update_document(None).unwrap()),
            ..initial.clone()
        };
        state::apply(&mut ui, &nav, &save, Ok(&saved));
        let ActiveDialog::PetalManifest {
            dirty,
            requirements_state,
            add_hexon_id_buf,
            ..
        } = &mut ui.active_dialog
        else {
            panic!()
        };
        assert!(!*dirty);
        assert_eq!(requirements_state.status(*dirty), "Requirements saved.");
        *add_hexon_id_buf = "unapplied-model".into();
        *dirty = true;
        assert_eq!(
            requirements_state.status(*dirty || !add_hexon_id_buf.is_empty()),
            "Last submitted declaration is confirmed saved. Newer draft changes remain unsaved."
        );
        add_hexon_id_buf.clear();
        let mut newer = draft.clone();
        newer.hexons[0].required = false;
        if let ActiveDialog::PetalManifest {
            manifest, dirty, ..
        } = &mut ui.active_dialog
        {
            *manifest = newer.clone();
            *dirty = true;
        }
        state::save(&mut ui, &nav, &tx, "p1", newer.clone(), 1.2).unwrap();
        let DbCommand::SetPetalRequirements {
            correlation_id: save_newer,
            ..
        } = rx.recv().unwrap()
        else {
            panic!()
        };
        let mut latest = saved.clone();
        latest.requirements = newer.clone();
        latest.document = Some(newer.update_document(saved.document.as_ref()).unwrap());
        // Another edit arrives while the preceding declaration is saving.
        newer.hexons[0].hexon_id = "model-v2".into();
        if let ActiveDialog::PetalManifest { manifest, .. } = &mut ui.active_dialog {
            *manifest = newer.clone();
        }
        state::apply(&mut ui, &nav, &save_newer, Ok(&latest));
        let ActiveDialog::PetalManifest {
            manifest,
            dirty,
            requirements_state,
            ..
        } = &ui.active_dialog
        else {
            panic!()
        };
        assert_eq!(manifest, &newer, "late save keeps newer draft");
        assert!(*dirty);
        assert_eq!(
            requirements_state.status(*dirty),
            "Last submitted declaration is confirmed saved. Newer draft changes remain unsaved."
        );
        state::save(&mut ui, &nav, &tx, "p1", newer.clone(), 2.0).unwrap();
        let DbCommand::SetPetalRequirements {
            correlation_id: timed_out,
            ..
        } = rx.recv().unwrap()
        else {
            panic!()
        };
        state::request_if_needed(&mut ui, &nav, &tx, 33.0);
        assert!(
            state::save(&mut ui, &nav, &tx, "p1", newer.clone(), 33.1).is_err(),
            "timeout never blindly retries write"
        );
        state::apply(&mut ui, &nav, &timed_out, Ok(&saved));
        assert!(rx.try_recv().is_err());
        if let ActiveDialog::PetalManifest {
            requirements_state, ..
        } = &mut ui.active_dialog
        {
            requirements_state.refresh();
        }
        state::request_if_needed(&mut ui, &nav, &tx, 34.0);
        let DbCommand::GetPetalRequirements {
            correlation_id: reload,
            ..
        } = rx.recv().unwrap()
        else {
            panic!("recovery is a read")
        };
        state::apply(&mut ui, &nav, &reload, Ok(&saved));
        assert!(
            state::save(&mut ui, &nav, &tx, "p1", newer.clone(), 34.1).is_err(),
            "loaded differing data requires explicit review"
        );
        state::review(&mut ui, false);
        state::save(&mut ui, &nav, &tx, "p1", newer.clone(), 34.2).unwrap();
        let DbCommand::SetPetalRequirements {
            correlation_id: foreign_reply,
            ..
        } = rx.recv().unwrap()
        else {
            panic!()
        };
        let wrong = PetalRequirementsSnapshot {
            petal_id: "other".into(),
            ..saved.clone()
        };
        state::apply(&mut ui, &nav, &foreign_reply, Ok(&wrong));
        assert!(state::save(&mut ui, &nav, &tx, "p1", newer.clone(), 34.3).is_err());
        manifest_open(&mut ui, "p1".into(), "P".into(), nav.active_scope());
        state::request_if_needed(&mut ui, &nav, &tx, 35.0);
        let DbCommand::GetPetalRequirements {
            correlation_id: fresh,
            ..
        } = rx.recv().unwrap()
        else {
            panic!()
        };
        assert_ne!(
            fresh, load,
            "reopened draft has restart-unique request identity"
        );
        state::apply(&mut ui, &nav, &load, Ok(&saved));
        state::apply(&mut ui, &nav, &fresh, Ok(&initial));
        if let ActiveDialog::PetalManifest {
            manifest, dirty, ..
        } = &mut ui.active_dialog
        {
            *manifest = draft.clone();
            *dirty = true;
        }
        nav.navigate_to_petal("p2");
        assert!(
            state::save(&mut ui, &nav, &tx, "p1", draft, 36.0).is_err(),
            "navigation is rechecked at action use"
        );
        assert!(rx.try_recv().is_err());
    }


}
