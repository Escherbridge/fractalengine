//! Handlers for single-node lifecycle results: import, create, delete.
//! See ../AGENTS.md §db-results.

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;

use super::super::{NodeEntry, VerseManager};
use crate::actions::UiManager;
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;

/// `GltfImported`: add the node to the tree and spawn it when in the active petal.
pub(super) fn handle_gltf_imported(
    node_id: &str,
    name: &str,
    petal_id: &str,
    asset_path: &str,
    position: [f32; 3],
    verse_mgr: &mut VerseManager,
    nav: &NavigationManager,
    commands: &mut Commands,
    asset_server: &AssetServer,
) {
    verse_mgr.add_node(
        petal_id,
        NodeEntry {
            id: node_id.to_string(),
            name: name.to_string(),
            has_asset: true,
            position,
            webpage_url: None,
            asset_path: Some(asset_path.to_string()),
        },
    );
    if nav.active_petal_id.as_deref() == Some(petal_id) {
        super::super::spawn::spawn_node_entity(
            commands,
            asset_server,
            node_id,
            petal_id,
            name,
            position,
            asset_path,
        );
    }
}

/// `NodeCreated`: add the node to the tree, flush a pending pen auto-create,
/// and re-sync the Paths tab.
pub(super) fn handle_node_created(
    id: &str,
    petal_id: &str,
    name: &str,
    has_asset: bool,
    correlation_id: Option<&str>,
    verse_mgr: &mut VerseManager,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    path_state: &mut PathEditorState,
    db_sender: &DbCommandSender,
) {
    verse_mgr.add_node(
        petal_id,
        NodeEntry {
            id: id.to_string(),
            name: name.to_string(),
            has_asset,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: None,
        },
    );
    let in_active_petal = nav.active_petal_id.as_deref() == Some(petal_id);
    // Pen auto-create flush (`pen_autocreate_track_20260713`, FR-2 +
    // HIGH-1 correlation-id fix): a no-track Pen click stashed
    // `pending_pen_create` (its fe-ui-generated `correlation_id` +
    // first point) and queued a single `PathCreateTrack` carrying
    // that id. The bridge echoes the id back here on `NodeCreated`,
    // so we flush the first point ONLY when the echoed id matches
    // the pending create — NOT on a content heuristic. A concurrent
    // foreign create (GPX import / create-entity dialog) carries a
    // different id (or `None`), so it can never hijack the pen flush.
    if let Some(cid) = correlation_id {
        if let Some(position) = path_state.take_pending_pen_create_if(cid) {
            path_state.start_editing(id.to_string());
            ui_mgr.push_action(crate::actions::UiAction::PathAppendPoint {
                track_node_id: id.to_string(),
                position,
            });
        }
    }
    // FR-3: any node create in the active petal may be a track —
    // re-run the Paths tab query rather than relying on the
    // manual Refresh button as the only sync path.
    if in_active_petal {
        crate::actions::path::query_tracks(db_sender, path_state, petal_id.to_string());
    }
}

/// `NodeDeleted`: prune the node, drop a stale Paths-tab edit session, and
/// re-sync the Paths tab. (FR-1/FR-3 — fixes the left-panel orphan for all
/// node deletes, not just tracks.)
pub(super) fn handle_node_deleted(
    node_id: &str,
    petal_id: &str,
    verse_mgr: &mut VerseManager,
    nav: &NavigationManager,
    path_state: &mut PathEditorState,
    db_sender: &DbCommandSender,
) {
    verse_mgr.remove_node(petal_id, node_id);
    if path_state.editing_track_id.as_deref() == Some(node_id) {
        path_state.stop_editing();
    }
    if nav.active_petal_id.as_deref() == Some(petal_id) {
        crate::actions::path::query_tracks(db_sender, path_state, petal_id.to_string());
    }
    bevy::log::info!("Deleted node {node_id} (petal {petal_id})");
}
