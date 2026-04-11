use crate::theme;
use bevy_egui::egui;

/// Renders the role indicator chip in the bottom-right corner.
/// `role_label` should come from the actual session role (e.g. "owner", "editor", "public").
pub fn role_chip_hud(ctx: &egui::Context, role_label: &str) {
    egui::Area::new("role_chip".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .show(ctx, |ui| {
            ui.add(egui::Button::new(role_label).fill(theme::BG_ROLE_CHIP));
        });
}
