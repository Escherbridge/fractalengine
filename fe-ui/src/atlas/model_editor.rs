/// A key-value pair row in the model metadata editor.
#[derive(Debug, Clone, Default)]
pub struct KvRow {
    pub key: String,
    pub value_raw: String,
}

impl KvRow {
    /// Returns `true` if `value_raw` parses as valid JSON.
    pub fn value_is_valid_json(&self) -> bool {
        serde_json::from_str::<serde_json::Value>(&self.value_raw).is_ok()
    }
}

/// State resource for the model metadata editor panel.
#[derive(Debug, Clone, Default)]
pub struct ModelEditorState {
    pub model_id: String,
    pub display_name: String,
    pub description: String,
    pub external_url: String,
    pub config_url: String,
    pub tags: Vec<String>,
    pub tag_input: String,
    pub kv_rows: Vec<KvRow>,
    pub dirty: bool,
}

impl ModelEditorState {
    /// Append a blank key-value row.
    pub fn add_kv_row(&mut self) {
        self.kv_rows.push(KvRow::default());
    }

    /// Remove the row at `index`.
    pub fn delete_kv_row(&mut self, index: usize) {
        if index < self.kv_rows.len() {
            self.kv_rows.remove(index);
        }
    }

    /// Returns `true` if the form is valid and can be saved.
    /// Invalid if any kv row's value is non-empty and not valid JSON.
    pub fn can_save(&self) -> bool {
        self.kv_rows
            .iter()
            .all(|row| row.value_raw.is_empty() || row.value_is_valid_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_kv_row_appends_blank_row() {
        let mut state = ModelEditorState::default();
        state.add_kv_row();
        assert_eq!(state.kv_rows.len(), 1);
        assert!(state.kv_rows[0].key.is_empty());
    }

    #[test]
    fn delete_kv_row_removes_correct_index() {
        let mut state = ModelEditorState::default();
        state.add_kv_row();
        state.add_kv_row();
        state.kv_rows[0].key = "k1".into();
        state.kv_rows[1].key = "k2".into();
        state.delete_kv_row(0);
        assert_eq!(state.kv_rows.len(), 1);
        assert_eq!(state.kv_rows[0].key, "k2");
    }

    #[test]
    fn kv_row_invalid_json_value_detected() {
        let row = KvRow {
            key: "x".into(),
            value_raw: "{not json}".into(),
        };
        assert!(!row.value_is_valid_json());
    }

    #[test]
    fn kv_row_valid_json_value_accepted() {
        let row = KvRow {
            key: "x".into(),
            value_raw: "\"hello\"".into(),
        };
        assert!(row.value_is_valid_json());
    }

    #[test]
    fn can_save_false_when_any_kv_row_has_invalid_json() {
        let mut state = ModelEditorState::default();
        state.add_kv_row();
        state.kv_rows[0].key = "k".into();
        state.kv_rows[0].value_raw = "not_json".into();
        assert!(!state.can_save());
    }
}
