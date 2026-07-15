//! NodeManager → InspectorFormState display sync (transform strings + URL).

use bevy::prelude::*;

use super::NodeManager;
use crate::plugin::InspectorFormState;
use crate::verse_manager::VerseManager;

/// Stored portal URL for `node_id` as an inspector buffer string ("" when none).
pub(super) fn stored_node_url(verse_mgr: &VerseManager, node_id: &str) -> String {
    verse_mgr
        .all_nodes()
        .find(|n| n.id == node_id)
        .and_then(|n| n.webpage_url.clone())
        .unwrap_or_default()
}

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
            inspector.external_url = stored_node_url(&verse_mgr, &sel.node_id);
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
            let _ = db_sender
                .0
                .send(fe_runtime::messages::DbCommand::GetNodeProperties {
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
        let Ok(t) = changed_query.get(entity) else {
            return;
        };
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

// FR-1 restart round-trip: save decision → DB-hydrated VerseManager → reload → still-validated.
#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;
    use crate::actions::portal::{compute_save_url, SaveUrlOutcome};
    use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
    use bevy::prelude::Entity;

    /// A VerseManager as `DbResult::HierarchyLoaded` would hydrate it after a
    /// restart: one node whose `webpage_url` came back from SurrealDB.
    fn hydrated_manager(node_id: &str, stored_url: Option<String>) -> VerseManager {
        let mut mgr = VerseManager {
            verses: vec![VerseEntry {
                id: "verse-1".to_string(),
                name: "Verse 1".to_string(),
                namespace_id: None,
                expanded: true,
                fractals: vec![FractalEntry {
                    id: "fractal-1".to_string(),
                    name: "Fractal 1".to_string(),
                    expanded: true,
                    petals: vec![PetalEntry {
                        id: "petal-1".to_string(),
                        name: "Petal 1".to_string(),
                        expanded: true,
                        nodes: vec![NodeEntry {
                            id: node_id.to_string(),
                            name: node_id.to_string(),
                            has_asset: false,
                            position: [0.0; 3],
                            webpage_url: stored_url,
                            asset_path: None,
                        }],
                    }],
                }],
            }],
            ..Default::default()
        };
        mgr.rebuild_node_index();
        mgr
    }

    fn selected(node_id: &str, url: &str) -> (NodeManager, InspectorFormState) {
        let mut node_mgr = NodeManager::default();
        node_mgr.select(Entity::from_bits(1), node_id);
        let mut inspector = InspectorFormState::default();
        inspector.external_url = url.to_string();
        (node_mgr, inspector)
    }

    #[test]
    fn stored_node_url_empty_when_node_has_no_url() {
        let mgr = hydrated_manager("node-1", None);
        assert_eq!(stored_node_url(&mgr, "node-1"), "");
        assert_eq!(stored_node_url(&mgr, "missing"), "");
    }

    #[test]
    fn save_url_restart_round_trip_still_validated() {
        // 1. Save through the real SaveUrl decision path.
        let (node_mgr, inspector) = selected("node-1", "https://example.com/twin");
        let SaveUrlOutcome::Persist { node_id, url } = compute_save_url(&node_mgr, &inspector)
        else {
            panic!("allowed URL must persist")
        };
        assert_eq!(url.as_deref(), Some("https://example.com/twin"));

        // 2. "Restart": a fresh VerseManager hydrated from the persisted row.
        let verse_mgr = hydrated_manager(&node_id, url);

        // 3. Reload path (what sync_manager_to_inspector runs on selection).
        let reloaded = stored_node_url(&verse_mgr, &node_id);
        assert_eq!(reloaded, "https://example.com/twin");

        // 4. Still-validated: the reloaded URL passes the same allowlist gate.
        let (node_mgr, mut inspector) = selected(&node_id, "");
        inspector.external_url = reloaded;
        assert!(matches!(
            compute_save_url(&node_mgr, &inspector),
            SaveUrlOutcome::Persist { url: Some(_), .. }
        ));
    }

    #[test]
    fn blocked_url_never_reaches_persistence() {
        let (node_mgr, inspector) = selected("node-1", "http://192.168.1.1/router");
        assert!(matches!(
            compute_save_url(&node_mgr, &inspector),
            SaveUrlOutcome::Blocked { .. }
        ));
        // Nothing persisted → a restart hydrates with the prior (empty) value.
        let verse_mgr = hydrated_manager("node-1", None);
        assert_eq!(stored_node_url(&verse_mgr, "node-1"), "");
    }
}
