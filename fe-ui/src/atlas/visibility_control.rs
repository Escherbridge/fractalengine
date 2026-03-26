use bevy_egui::egui;
use fe_database::atlas::Visibility;

/// Extension trait that adds a human-readable display name to `Visibility`.
pub trait VisibilityExt {
    fn display_name(&self) -> &'static str;
}

impl VisibilityExt for Visibility {
    fn display_name(&self) -> &'static str {
        match self {
            Visibility::Public => "Public",
            Visibility::Private => "Private",
            Visibility::Unlisted => "Unlisted",
        }
    }
}

/// Render a radio-button group for selecting visibility.
pub fn visibility_radio_ui(ui: &mut egui::Ui, current: &mut Visibility) {
    ui.horizontal(|ui| {
        ui.selectable_value(current, Visibility::Private, "Private");
        ui.selectable_value(current, Visibility::Public, "Public");
        ui.selectable_value(current, Visibility::Unlisted, "Unlisted");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_display_names_are_human_readable() {
        assert_eq!(Visibility::Public.display_name(), "Public");
        assert_eq!(Visibility::Private.display_name(), "Private");
        assert_eq!(Visibility::Unlisted.display_name(), "Unlisted");
    }
}
