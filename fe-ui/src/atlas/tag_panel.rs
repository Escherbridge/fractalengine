use bevy_egui::egui;

use super::petal_wizard::TagError;

/// Validate a tag string, returning Ok or a TagError.
pub fn validate_tag(tags: &[String], tag: &str) -> Result<(), TagError> {
    if tags.len() >= 20 {
        return Err(TagError::LimitReached);
    }
    if tag.len() > 32 {
        return Err(TagError::TooLong);
    }
    let lower = tag.to_lowercase();
    if tags.iter().any(|t| t.to_lowercase() == lower) {
        return Err(TagError::Duplicate);
    }
    Ok(())
}

/// Remove a tag at the given index.
pub fn remove_tag(tags: &mut Vec<String>, index: usize) {
    if index < tags.len() {
        tags.remove(index);
    }
}

/// Render a tag chip list with remove buttons and an "Add" input.
/// Returns `Some(tag)` if a valid tag was submitted, `None` otherwise.
pub fn tag_input_ui(ui: &mut egui::Ui, tags: &mut Vec<String>, input: &mut String) -> Option<String> {
    ui.horizontal_wrapped(|ui| {
        let mut to_remove: Option<usize> = None;
        for (i, tag) in tags.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(tag.as_str());
                if ui.small_button("×").clicked() {
                    to_remove = Some(i);
                }
            });
        }
        if let Some(idx) = to_remove {
            remove_tag(tags, idx);
        }
    });

    let mut submitted: Option<String> = None;
    ui.horizontal(|ui| {
        ui.text_edit_singleline(input);
        if ui.button("Add").clicked() && !input.is_empty() {
            submitted = Some(input.clone());
        }
    });
    submitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_tag_removes_correct_index() {
        let mut tags = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        remove_tag(&mut tags, 1);
        assert_eq!(tags, vec!["a", "c"]);
    }
}
