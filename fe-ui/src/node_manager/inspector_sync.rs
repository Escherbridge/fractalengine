//! NodeManager → InspectorFormState display sync (transform strings + URL).

use bevy::prelude::*;

use super::NodeManager;
use crate::plugin::InspectorFormState;

pub(super) fn sync_manager_to_inspector(
    manager: Res<NodeManager>,
    mut inspector: ResMut<InspectorFormState>,
    verse_mgr: Res<crate::verse_manager::VerseManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    // Changed<Transform> avoids 9 format!() allocations per frame while dragging.
    changed_query: Query<&Transform, Changed<Transform>>,
    // Plain query used on initial selection so the inspector populates even when
    // the transform hasn't changed this frame (e.g. freshly selected static node).
    all_query: Query<&Transform>,
    mut last_selected: Local<Option<Entity>>,
) {
    // Early return when nothing is selected.
    let Some(entity) = manager.selected_entity() else {
        *last_selected = None;
        return;
    };

    // On initial selection the transform hasn't Changed<> yet — read it
    // unconditionally so the inspector populates immediately.
    let just_selected = *last_selected != Some(entity);
    *last_selected = Some(entity);

    // Sync per-node URL from VerseManager on selection change so the
    // inspector shows the correct URL for the newly-selected node.
    if just_selected {
        if let Some(ref sel) = manager.selected {
            let url = verse_mgr
                .all_nodes()
                .find(|n| n.id == sel.node_id)
                .and_then(|n| n.webpage_url.clone())
                .unwrap_or_default();
            inspector.external_url = url;
        }
        // Load properties for the newly selected node
        if let Some(ref sel) = manager.selected {
            inspector.node_properties_loading = true;
            inspector.node_properties = serde_json::Value::Object(Default::default());
            // Clear the Annotation card buffers so they don't briefly show the
            // previous node's values while the async property load is in flight
            // (populated by `DbResult::NodePropertiesLoaded` in db_results.rs).
            inspector.annotation_title_buf.clear();
            inspector.annotation_body_buf.clear();
            inspector.annotation_color_buf.clear();
            let _ = db_sender.0.send(fe_runtime::messages::DbCommand::GetNodeProperties {
                node_id: sel.node_id.clone(),
            });
        }
        // Reset API token tab state for the new selection
        inspector.generated_api_token = None;
        inspector.api_tokens.clear();
        inspector.api_tokens_loading = false;
        inspector.api_token_scope_buf.clear();
        inspector.api_tokens_page = 0;
        inspector.api_tokens_total = 0;
    }

    let t = if just_selected {
        let Ok(t) = all_query.get(entity) else { return };
        t
    } else {
        let Ok(t) = changed_query.get(entity) else { return };
        t
    };
    let (rx, ry, rz) = t.rotation.to_euler(EulerRot::XYZ);
    inspector.pos = [
        format!("{:.2}", t.translation.x),
        format!("{:.2}", t.translation.y),
        format!("{:.2}", t.translation.z),
    ];
    inspector.rot = [
        format!("{:.1}", rx.to_degrees()),
        format!("{:.1}", ry.to_degrees()),
        format!("{:.1}", rz.to_degrees()),
    ];
    inspector.scale = [
        format!("{:.2}", t.scale.x),
        format!("{:.2}", t.scale.y),
        format!("{:.2}", t.scale.z),
    ];
}
