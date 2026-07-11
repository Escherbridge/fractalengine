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
}
