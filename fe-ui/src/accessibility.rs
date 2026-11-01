//! Accessible names for existing egui responses.

use std::fmt::Display;

use bevy_egui::egui;

/// Name a response while retaining its native widget semantics.
pub trait ResponseExt: Sized {
    fn observed(self, ui: &egui::Ui) -> Self;
    fn named(self, ui: &egui::Ui, name: impl Display) -> Self;
    fn button_named(self, ui: &egui::Ui, name: impl Display) -> Self;
    fn labelled(self, ui: &egui::Ui, label: &egui::Response) -> Self;
}

impl ResponseExt for egui::Response {
    fn observed(self, ui: &egui::Ui) -> Self {
        crate::ui_shell::layout_policy::record_response(ui, &self);
        self
    }

    fn named(self, ui: &egui::Ui, name: impl Display) -> Self {
        ui.ctx()
            .accesskit_node_builder(self.id, |node| node.set_label(name.to_string()));
        self.observed(ui)
    }

    fn button_named(self, ui: &egui::Ui, name: impl Display) -> Self {
        self.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, self.enabled(), &name)
        });
        self.observed(ui)
    }

    fn labelled(self, ui: &egui::Ui, label: &egui::Response) -> Self {
        self.labelled_by(label.id).observed(ui)
    }
}
