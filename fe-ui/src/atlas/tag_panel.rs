use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::petal_wizard::TagError;

/// Validate a tag string, returning Ok or a TagError.
pub fn validate_tag(tags: &[String], tag: &str) -> Result<(), TagError> {
    if tag.trim().is_empty() {
        return Err(TagError::Empty);
    }
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
pub fn tag_input_ui(
    ui: &mut egui::Ui,
    tags: &mut Vec<String>,
    input: &mut String,
) -> Option<String> {
    ui.horizontal_wrapped(|ui| {
        let mut to_remove: Option<usize> = None;
        for (i, tag) in tags.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(tag.as_str());
                if crate::ui_shell::layout_policy::small_button(ui, "×")
                    .named(ui, format_args!("Remove tag {tag}"))
                    .clicked()
                {
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
        ui.add(crate::ui_shell::layout_policy::text_input(ui, input)).named(ui, "New tag");
        if ui.button("Add").named(ui, "Add tag").clicked() && !input.is_empty() {
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
