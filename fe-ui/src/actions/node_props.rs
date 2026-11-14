//! Node custom-property action handling (load / set / delete).

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

/// Reserved node-property keys backing the inspector's Annotation card. Flat
/// dotted-string keys (not nested paths) — see `set_entity_property_handler`'s
/// `properties[$key]` dynamic-key setter in fe-database.
pub const ANNOTATION_TITLE_KEY: &str = "gis.annotation.title";
pub const ANNOTATION_BODY_KEY: &str = "gis.annotation.body";
pub const ANNOTATION_COLOR_KEY: &str = "gis.annotation.color";

/// Marker property set on track nodes (name of the track) — lets the Paths
/// tab list track nodes with the same "nodes with a reserved property"
/// query shape as the Annotations tab, without a `fe-terrain` dependency.
/// The bridge (`fractalengine/src/gpx_bridge.rs`) sets this alongside
/// `gpx_points` on `CreateTrack`/import. See `fe-ui/src/AGENTS.md`
/// §path-editor. Not referenced in-crate — `gis::query` keeps its own copy
/// (import-free by design, see that module); this is the canonical doc of
/// the contract for cross-crate readers.
#[allow(dead_code)]
pub const TRACK_NAME_KEY: &str = "gis.track.name";

/// Extract the Annotation card's (title, body, color) text buffers from a
/// node's loaded `properties` object. Missing/non-string values become "".
pub(crate) fn annotation_fields_from_properties(
    props: &serde_json::Value,
) -> (String, String, String) {
    let get = |k: &str| {
        props
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default()
    };
    (
        get(ANNOTATION_TITLE_KEY),
        get(ANNOTATION_BODY_KEY),
        get(ANNOTATION_COLOR_KEY),
    )
}

/// Which Annotation-card buffer a property key corresponds to, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnnotationField {
    Title,
    Body,
    Color,
}

/// Maps a deleted/set property key to its Annotation-card buffer. Used by
/// `DbResult::NodePropertyDeleted` handling to clear only the buffer for the
/// touched key instead of re-deriving all three from the (possibly stale,
/// mid-Save-batch) properties cache — see `fe-ui/src/AGENTS.md`
/// §gis-query-ui annotation-save-fix for the race this avoids.
pub(crate) fn annotation_field_for_key(key: &str) -> Option<AnnotationField> {
    match key {
        ANNOTATION_TITLE_KEY => Some(AnnotationField::Title),
        ANNOTATION_BODY_KEY => Some(AnnotationField::Body),
        ANNOTATION_COLOR_KEY => Some(AnnotationField::Color),
        _ => None,
    }
}

pub(crate) fn load(db_sender: &DbCommandSender, node_id: String) {
    if db_sender
        .0
        .send(DbCommand::GetNodeProperties {
            node_id,
            correlation_id: None,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — GetNodeProperties not dispatched");
    }
}

pub(crate) fn set(
    db_sender: &DbCommandSender,
    node_id: String,
    key: String,
    value: serde_json::Value,
) {
    let command = DbCommand::SetNodeProperty {
        node_id,
        key,
        value,
        correlation_id: None,
    };
    if super::node::submit_scene_write(db_sender, command).channel_unavailable() {
        bevy::log::warn!("db_sender channel closed — SetNodeProperty not dispatched");
    }
}

pub(crate) fn delete(db_sender: &DbCommandSender, node_id: String, key: String) {
    let command = DbCommand::DeleteNodeProperty {
        node_id,
        key,
        correlation_id: None,
    };
    if super::node::submit_scene_write(db_sender, command).channel_unavailable() {
        bevy::log::warn!("db_sender channel closed — DeleteNodeProperty not dispatched");
    }
}

/// Engine-reserved flat keys: the node's identity, parent edges and kind
/// definition. Clear Properties keeps them (QA-0102); see `actions/AGENTS.md`
/// §clear-properties.
const STRUCTURAL_PROPERTY_KEYS: [&str; 15] = [
    "parent_id",
    "gpx_track_id",
    "owning_path_id",
    "instance_index",
    "node_kind",
    "gpx_type",
    "gpx_points",
    "path_id",
    "route_points",
    "material",
    "placed",
    "region_id",
    "cut_volume_m3",
    "fill_volume_m3",
    "_fe_scene_asset",
];

/// Engine-reserved key namespaces (definition bags of engine object kinds).
const STRUCTURAL_PROPERTY_PREFIXES: [&str; 6] =
    ["view.", "stamp.", "gis.track.", "dimension.", "path_asset", "primitive"];

/// Whether Clear Properties removes `key`: everything a user can author,
/// never an engine-reserved structural key.
pub(crate) fn is_clearable_property(key: &str) -> bool {
    !STRUCTURAL_PROPERTY_KEYS.contains(&key)
        && !STRUCTURAL_PROPERTY_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

/// T4 FR-2 — clear a node's custom properties WITHOUT deleting the node
/// (`node::handle_delete` tombstones). Step 1 of 2: read the current bag; the
/// `NodePropertiesLoaded` arm finishes via [`finish_clear`].
pub(crate) fn handle_clear(
    db_sender: &DbCommandSender,
    ui_mgr: &mut super::UiManager,
    node_id: String,
) {
    if db_sender
        .0
        .send(DbCommand::GetNodeProperties {
            node_id: node_id.clone(),
            correlation_id: None,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — ClearNodeProperties not dispatched");
        return;
    }
    ui_mgr.pending_property_clears.insert(node_id);
}

/// Step 2 of Clear Properties: delete every clearable key of the loaded bag.
/// No-op unless `node_id` has a pending clear.
pub(crate) fn finish_clear(
    db_sender: &DbCommandSender,
    ui_mgr: &mut super::UiManager,
    node_id: &str,
    properties: &serde_json::Value,
) {
    if !ui_mgr.pending_property_clears.remove(node_id) {
        return;
    }
    let Some(object) = properties.as_object() else {
        return;
    };
    let keys: Vec<String> = object
        .keys()
        .filter(|key| is_clearable_property(key))
        .cloned()
        .collect();
    if keys.is_empty() {
        return;
    }
    // One command = one commit and one replicated row version (QA-0129).
    for chunk in keys.chunks(fe_runtime::messages::MAX_CLEAR_PROPERTY_KEYS) {
        let command = DbCommand::ClearNodeProperties {
            node_id: node_id.to_string(),
            keys: chunk.to_vec(),
        };
        if super::node::submit_scene_write(db_sender, command).channel_unavailable() {
            bevy::log::warn!("db_sender channel closed — ClearNodeProperties not dispatched");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_fields_extracts_all_three() {
        let props = serde_json::json!({
            (ANNOTATION_TITLE_KEY): "Trailhead",
            (ANNOTATION_BODY_KEY): "Start of the loop trail.",
            (ANNOTATION_COLOR_KEY): "#ff8800",
            "other.prop": "ignored",
        });
        let (title, body, color) = annotation_fields_from_properties(&props);
        assert_eq!(title, "Trailhead");
        assert_eq!(body, "Start of the loop trail.");
        assert_eq!(color, "#ff8800");
    }

    #[test]
    fn annotation_fields_default_empty_on_missing_keys() {
        let props = serde_json::json!({});
        let (title, body, color) = annotation_fields_from_properties(&props);
        assert_eq!(title, "");
        assert_eq!(body, "");
        assert_eq!(color, "");
    }

    #[test]
    fn annotation_fields_ignore_non_string_values() {
        let props = serde_json::json!({ (ANNOTATION_TITLE_KEY): 42 });
        let (title, _, _) = annotation_fields_from_properties(&props);
        assert_eq!(title, "");
    }

    #[test]
    fn annotation_field_for_key_maps_all_three() {
        assert_eq!(
            annotation_field_for_key(ANNOTATION_TITLE_KEY),
            Some(AnnotationField::Title)
        );
        assert_eq!(
            annotation_field_for_key(ANNOTATION_BODY_KEY),
            Some(AnnotationField::Body)
        );
        assert_eq!(
            annotation_field_for_key(ANNOTATION_COLOR_KEY),
            Some(AnnotationField::Color)
        );
    }

    #[test]
    fn annotation_field_for_key_none_for_unrelated_key() {
        assert_eq!(annotation_field_for_key("custom.other"), None);
    }

    #[test]
    fn clear_properties_reads_the_bag_then_deletes_only_user_authored_keys() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut ui_mgr = super::super::UiManager::default();
        handle_clear(&sender, &mut ui_mgr, "n1".into());
        assert!(matches!(rx.try_recv(), Ok(DbCommand::GetNodeProperties { node_id, .. }) if node_id == "n1"));
        let bag = serde_json::json!({
            (ANNOTATION_TITLE_KEY): "t", "qa_value": 10, "p_count": 3,
            "parent_id": "root", "gpx_track_id": "trk", "view.groups": ["G"],
            "dimension.mode": "tape", "path_asset": {}, "node_kind": "earthwork_region",
        });
        finish_clear(&sender, &mut ui_mgr, "other", &bag);
        assert!(rx.try_recv().is_err(), "a bag for a node without a pending clear deletes nothing");
        finish_clear(&sender, &mut ui_mgr, "n1", &bag);
        let commands: Vec<DbCommand> = rx.try_iter().collect();
        let [DbCommand::ClearNodeProperties { node_id, keys }] = commands.as_slice() else {
            panic!("QA-0129: Clear Properties must send ONE atomic command, got {commands:?}");
        };
        assert_eq!(node_id, "n1");
        let mut deleted = keys.clone();
        deleted.sort();
        assert_eq!(deleted, [ANNOTATION_TITLE_KEY, "p_count", "qa_value"]);
        finish_clear(&sender, &mut ui_mgr, "n1", &bag);
        assert!(rx.try_recv().is_err(), "the pending clear is consumed once");
    }
}
