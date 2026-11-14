//! Handlers for single-node lifecycle results: import, create, delete.
//! See ../AGENTS.md §db-results.

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;

use super::super::{NodeEntry, PathAssetApplied, PathAssetCache, VerseManager};
use crate::actions::UiManager;
use crate::dialogs::ActiveDialog;
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;

/// `NodeCreated`: add the node to the tree, flush a pending pen auto-create,
/// bind an earthwork-region correlation (T3) or a dimension-callout
/// correlation (HS-P6-CALLOUT), and re-sync the Paths tab.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_node_created(
    id: &str,
    petal_id: &str,
    name: &str,
    has_asset: bool,
    correlation_id: Option<&str>,
    position: [f32; 3],
    verse_mgr: &mut VerseManager,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    path_state: &mut PathEditorState,
    db_sender: &DbCommandSender,
    earthwork_map: &mut crate::actions::terrain_proposal::EarthworkNodeMap,
    callout_map: &mut crate::actions::dimension_callout::DimensionCalloutMap,
) {
    if verse_mgr.petal_id_of(id).is_none() {
        verse_mgr.add_node(
            petal_id,
            NodeEntry {
                transform: Default::default(),
                id: id.to_string(),
                name: name.to_string(),
                has_asset,
                // camera_focus_clip_20260716 FR-1: echoed from `NodeCreated` instead
                // of a hardcoded origin (see spec §Root causes for the stale-focus bug).
                position,
                webpage_url: None,
                asset_path: None,
            },
        );
    }
    if verse_mgr.scene_access_blocked()
        || correlation_id.is_some_and(crate::path_ops::is_host_path_request)
    {
        return;
    }
    let in_active_petal = nav.active_petal_id.as_deref() == Some(petal_id);
    // T3 (D-A8/N-10): an `earthwork:{region_id}` correlation binds the created
    // node to its region and writes the endpoint property bag (node_kind /
    // material / region_id; volumes remain absent until complete samples — the bake report fills volumes
    // async). Mirrors the pen consume idiom below: echoed-id match only, so a
    // concurrent foreign create can never hijack the bind.
    if let Some(region_id) = correlation_id
        .and_then(crate::actions::terrain_proposal::earthwork_region_id_from_correlation)
    {
        let material = earthwork_map
            .take_pending_material(petal_id, region_id)
            .unwrap_or_else(|| "earth".to_string());
        earthwork_map.record(petal_id, region_id, id);
        for (key, value) in
            crate::actions::terrain_proposal::earthwork_node_properties(region_id, &material)
        {
            // Through the scene write door so a bound scene hook sees the follow-up writes too.
            if crate::actions::node::submit_scene_write(
                db_sender,
                fe_runtime::messages::DbCommand::SetNodeProperty {
                    node_id: id.to_string(),
                    key: key.to_string(),
                    value,
                    correlation_id: None,
                },
            ).channel_unavailable()
            {
                bevy::log::warn!(
                    "db_sender channel closed — earthwork node contract keys not written"
                );
                break;
            }
        }
    }
    // HS-P6-CALLOUT: a `callout:{id}` correlation moves the pending
    // measurement geometry into the map and writes its property bag — same
    // echoed-id idiom as the earthwork bind above.
    crate::actions::dimension_callout::bind_created_callout(
        db_sender,
        callout_map,
        id,
        petal_id,
        correlation_id,
    );
    // Pen auto-create flush (`pen_autocreate_track_20260713`, FR-2 +
    // HIGH-1 correlation-id fix): a no-track Pen click stashed
    // `pending_pen_create` (its fe-ui-generated `correlation_id` +
    // first point) and queued a single `PathCreateTrack` carrying
    // that id. The bridge echoes the id back here on `NodeCreated`,
    // so we flush the first point ONLY when the echoed id matches
    // the pending create — NOT on a content heuristic. A concurrent
    // foreign create (GPX import / create-entity dialog) carries a
    // different id (or `None`), so it can never hijack the pen flush.
    if let Some(cid) = correlation_id.filter(|_| in_active_petal) {
        if let Some(pen) = path_state.take_pending_pen_create_in_context(cid, nav, verse_mgr) {
            path_state.start_editing(id.to_string());
            // pen_curve_tool_20260722 (FR-4): a press-drag first anchor keeps
            // its handles through the deferred echo; a plain click stays on
            // the legacy append path.
            if pen.handle_in.is_some() || pen.handle_out.is_some() {
                ui_mgr.push_action(crate::actions::UiAction::PathAppendSmoothPoint {
                    track_node_id: id.to_string(),
                    position: pen.first_point,
                    handle_in: pen.handle_in,
                    handle_out: pen.handle_out,
                    corner: pen.corner,
                    smoothness: pen.smoothness,
                });
            } else {
                ui_mgr.push_action(crate::actions::UiAction::PathAppendPoint {
                    track_node_id: id.to_string(),
                    position: pen.first_point,
                });
            }
            // Road first-commit: the rest of the first segment, applied to the track just opened for editing.
            if !pen.trailing_points.is_empty() {
                ui_mgr.push_action(crate::actions::UiAction::PathAppendShape {
                    points: pen.trailing_points,
                });
            }
        }
    }
    // FR-3: any node create in the active petal may be a track —
    // re-run the Paths tab query rather than relying on the
    // manual Refresh button as the only sync path.
    if in_active_petal {
        crate::actions::path::query_tracks(db_sender, path_state, petal_id.to_string());
    }
}

/// `NodeRenamed`: update the node's display name in the tree (the node mirror
/// of `EntityRenamed`; the spawned entity's `Name` refreshes on next respawn).
pub(super) fn handle_node_renamed(node_id: &str, new_name: &str, verse_mgr: &mut VerseManager) {
    verse_mgr.update_node_name(node_id, new_name);
    bevy::log::info!("Renamed node {node_id} -> '{new_name}'");
}

/// `NodeDescendantCount`: stash the authoritative subtree size where the open
/// cascade-delete confirm reads it (Node Options or the context menu). A result
/// for a since-closed/re-targeted dialog is dropped — stale counts never leak
/// into another node's confirm.
pub(super) fn handle_node_descendant_count(node_id: &str, count: usize, ui_mgr: &mut UiManager) {
    match &mut ui_mgr.active_dialog {
        ActiveDialog::NodeOptions {
            node_id: open_id,
            descendant_count,
            ..
        } if open_id.as_str() == node_id => *descendant_count = Some(count),
        ActiveDialog::ContextMenu {
            target,
            descendant_count,
            ..
        } if target
            .as_ref()
            .and_then(|t| t.node_id.as_deref())
            .is_some_and(|id| id == node_id) =>
        {
            *descendant_count = Some(count)
        }
        _ => {}
    }
}

/// `NodeDeleted`: prune the node, cascade-delete its stamped path-assets (FR-4),
/// drop a stale Paths-tab edit session, and re-sync the Paths tab. (FR-1/FR-3 —
/// fixes the left-panel orphan for all node deletes, not just tracks.) See
/// ../AGENTS.md §path-asset-materialization.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_node_deleted(
    node_id: &str,
    petal_id: &str,
    verse_mgr: &mut VerseManager,
    nav: &NavigationManager,
    path_state: &mut PathEditorState,
    db_sender: &DbCommandSender,
    path_asset_cache: &mut PathAssetCache,
    path_asset_applied: &mut PathAssetApplied,
    callout_map: &mut crate::actions::dimension_callout::DimensionCalloutMap,
) {
    verse_mgr.remove_node(petal_id, node_id);
    // HS-P6-CALLOUT: drop a deleted callout node's bookkeeping; a no-op for
    // any other node (NFR-5 parity with the path-asset cache invalidations).
    callout_map.forget(node_id);
    // FR-4: evict the track's stamp cache + applied gate so the materializer's
    // orphan pass despawns its glTF and it can't resurrect on petal re-entry.
    // No-op for a track that never stamped (NFR-5).
    path_asset_cache.invalidate(node_id);
    path_asset_applied.invalidate(node_id);
    if path_state.editing_track_id.as_deref() == Some(node_id) {
        path_state.stop_editing();
    }
    if nav.active_petal_id.as_deref() == Some(petal_id) {
        crate::actions::path::query_tracks(db_sender, path_state, petal_id.to_string());
    }
    bevy::log::info!("Deleted node {node_id} (petal {petal_id})");
}
