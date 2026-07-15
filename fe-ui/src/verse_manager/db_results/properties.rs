//! Handlers for per-node property results. All three gate on `node_id` still
//! matching the current selection — a stale async response must not stomp a
//! different node's buffers (see `fe-ui/src/AGENTS.md` §gis-query-ui
//! annotation-save-fix). Each returns `false` when the dispatcher must
//! `continue` (skipping `pending_api.try_deliver`), preserving the original
//! mega-match's control flow exactly.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::plugin::InspectorFormState;

/// `true` when `node_id` is still the currently-selected node — guards
/// async property-load/set/delete results against stomping a different
/// node's buffers after the user has switched selection mid-round-trip.
pub(super) fn is_for_selected_node(node_mgr: &NodeManager, node_id: &str) -> bool {
    node_mgr.selected.as_ref().is_some_and(|s| s.node_id == node_id)
}

/// `NodePropertiesLoaded`: feed the Paths-tab read-back, then the inspector.
pub(super) fn handle_node_properties_loaded(
    node_id: &str,
    properties: &serde_json::Value,
    path_state: &mut PathEditorState,
    node_mgr: &NodeManager,
    inspector: &mut InspectorFormState,
) -> bool {
    // Path editor's `gpx_points` read-back (PathSelectTrack): a
    // DIFFERENT claimant from the inspector's `is_for_selected_node`
    // guard below — `NodePropertiesLoaded` has no correlation id
    // and is broadcast to every reader, so the inspector's own
    // GetNodeProperties reply must not stomp this buffer and vice
    // versa. Gated on `editing_track_id` still matching (guards a
    // stale reply after the user picked a different track or left
    // the editor) + `points_pending` (only the read-back we asked
    // for, not e.g. a later unrelated load). See AGENTS.md §path-editor.
    if path_state.editing_track_id.as_deref() == Some(node_id) && path_state.points_pending {
        path_state.points = properties
            .get("gpx_points")
            .map(crate::gis::decode_gpx_points)
            .unwrap_or_default();
        // track_styling_20260713: seed the Paths-tab style controls
        // from the same property bag (default when absent/invalid).
        path_state.edited_track_style =
            crate::gis::TrackStyleFields::from_properties(properties);
        // path-asset-stamp: seed the stamp descriptor from the same
        // bag so `reconcile_path_asset` (keyed on `editing_track_id`)
        // can spawn the models. `None` when absent/invalid.
        path_state.edited_track_path_asset = properties
            .get(fe_sdk::path_asset::PATH_ASSET_PROPERTY_KEY)
            .and_then(|raw| fe_sdk::path_asset::PathAssetDescriptor::from_json(raw).ok());
        path_state.points_pending = false;
    }
    if !is_for_selected_node(node_mgr, node_id) {
        return false;
    }
    inspector.node_properties = properties.clone();
    inspector.node_properties_loading = false;
    // Annotation card buffers derive from the same properties object.
    let (title, body, color) =
        crate::actions::node_props::annotation_fields_from_properties(properties);
    inspector.annotation_title_buf = title;
    inspector.annotation_body_buf = body;
    inspector.annotation_color_buf = color;
    true
}

/// `NodePropertySet`: re-sync the Paths tab / inspector by re-issuing the
/// property read; returns whether the write targeted the selected node.
pub(super) fn handle_node_property_set(
    node_id: &str,
    key: &str,
    nav: &NavigationManager,
    db_sender: &DbCommandSender,
    path_state: &mut PathEditorState,
    node_mgr: &NodeManager,
    inspector: &mut InspectorFormState,
) -> bool {
    // FR-3: a track-identity property landing anywhere (not just
    // the currently-selected node) means the Paths tab's track
    // list may now be stale — re-run the query unconditionally
    // for that one key, independent of the selection guard below.
    if key == "gis.track.name" {
        if let Some(petal_id) = nav.active_petal_id.clone() {
            crate::actions::path::query_tracks(db_sender, path_state, petal_id);
        }
    }
    // path-asset-stamp: a property write on the track currently open
    // in the Paths tab (e.g. a `PathAssetApply` writing `path_asset`)
    // may have changed the points/style/stamp descriptor the Paths
    // tab keys off — refresh them independent of the viewport/tree
    // selection guard below. Setting `points_pending` re-arms the
    // `NodePropertiesLoaded` arm to repopulate `points` +
    // `edited_track_style` + `edited_track_path_asset`. Mirrors
    // `select_track`'s round-trip.
    let refresh_editing_track = path_state.editing_track_id.as_deref() == Some(node_id);
    if refresh_editing_track {
        path_state.points_pending = true;
    }
    let for_selected = is_for_selected_node(node_mgr, node_id);
    if for_selected {
        // Re-fetch properties for the node to refresh the inspector UI.
        inspector.node_properties_loading = true;
    }
    // Issue the read once when either consumer needs it (idempotent).
    if refresh_editing_track || for_selected {
        let _ = db_sender.0.send(DbCommand::GetNodeProperties { node_id: node_id.to_string() });
    }
    for_selected
}

/// `NodePropertyDeleted`: drop the key locally and clear only its buffer.
pub(super) fn handle_node_property_deleted(
    node_id: &str,
    key: &str,
    node_mgr: &NodeManager,
    inspector: &mut InspectorFormState,
) -> bool {
    if !is_for_selected_node(node_mgr, node_id) {
        return false;
    }
    // Remove the key locally for immediate UI feedback
    if let Some(obj) = inspector.node_properties.as_object_mut() {
        obj.remove(key);
    }
    // Update only the buffer for the deleted key rather than
    // re-deriving all three from `inspector.node_properties` —
    // a single "Save Annotation" click emits one action per
    // field (Set for non-empty, Delete for empty), and that
    // cache may still be missing a sibling field's just-Set
    // value (its own NodePropertySet→refetch round-trip hasn't
    // resolved yet) when this Delete result lands. Re-deriving
    // from the stale cache would visibly revert the sibling
    // field to blank even though its write already landed in
    // the DB — this was the annotation-save bug.
    match crate::actions::node_props::annotation_field_for_key(key) {
        Some(crate::actions::node_props::AnnotationField::Title) => {
            inspector.annotation_title_buf.clear();
        }
        Some(crate::actions::node_props::AnnotationField::Body) => {
            inspector.annotation_body_buf.clear();
        }
        Some(crate::actions::node_props::AnnotationField::Color) => {
            inspector.annotation_color_buf.clear();
        }
        None => {}
    }
    true
}
