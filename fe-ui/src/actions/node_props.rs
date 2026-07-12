//! Node custom-property action handling (load / set / delete).

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

/// Reserved node-property keys backing the inspector's Annotation card. Flat
/// dotted-string keys (not nested paths) — see `set_entity_property_handler`'s
/// `properties[$key]` dynamic-key setter in fe-database.
pub const ANNOTATION_TITLE_KEY: &str = "gis.annotation.title";
pub const ANNOTATION_BODY_KEY: &str = "gis.annotation.body";
pub const ANNOTATION_COLOR_KEY: &str = "gis.annotation.color";

/// Extract the Annotation card's (title, body, color) text buffers from a
/// node's loaded `properties` object. Missing/non-string values become "".
pub(crate) fn annotation_fields_from_properties(props: &serde_json::Value) -> (String, String, String) {
    let get = |k: &str| props.get(k).and_then(|v| v.as_str()).map(str::to_string).unwrap_or_default();
    (get(ANNOTATION_TITLE_KEY), get(ANNOTATION_BODY_KEY), get(ANNOTATION_COLOR_KEY))
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
        .send(DbCommand::GetNodeProperties { node_id })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — GetNodeProperties not dispatched");
    }
}

pub(crate) fn set(db_sender: &DbCommandSender, node_id: String, key: String, value: serde_json::Value) {
    if db_sender
        .0
        .send(DbCommand::SetNodeProperty { node_id, key, value })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — SetNodeProperty not dispatched");
    }
}

pub(crate) fn delete(db_sender: &DbCommandSender, node_id: String, key: String) {
    if db_sender
        .0
        .send(DbCommand::DeleteNodeProperty { node_id, key })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — DeleteNodeProperty not dispatched");
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
        assert_eq!(annotation_field_for_key(ANNOTATION_TITLE_KEY), Some(AnnotationField::Title));
        assert_eq!(annotation_field_for_key(ANNOTATION_BODY_KEY), Some(AnnotationField::Body));
        assert_eq!(annotation_field_for_key(ANNOTATION_COLOR_KEY), Some(AnnotationField::Color));
    }

    #[test]
    fn annotation_field_for_key_none_for_unrelated_key() {
        assert_eq!(annotation_field_for_key("custom.other"), None);
    }
}
