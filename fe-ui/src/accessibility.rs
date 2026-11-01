//! Accessible names for existing egui responses.

use std::fmt::Display;

use bevy_egui::egui;

/// Name a response while retaining its native widget semantics.
pub trait ResponseExt: Sized {
    fn named(self, ui: &egui::Ui, name: impl Display) -> Self;
    fn labelled(self, ui: &egui::Ui, label: &egui::Response) -> Self;
}

impl ResponseExt for egui::Response {
    fn named(self, ui: &egui::Ui, name: impl Display) -> Self {
        ui.ctx()
            .accesskit_node_builder(self.id, |node| node.set_label(name.to_string()));
        crate::ui_shell::layout_policy::record_response(ui, &self);
        self
    }

    fn labelled(self, ui: &egui::Ui, label: &egui::Response) -> Self {
        let response = self.labelled_by(label.id);
        crate::ui_shell::layout_policy::record_response(ui, &response);
        response
    }
}
