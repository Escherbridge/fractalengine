use bevy_egui::{egui, EguiContexts};

pub fn role_chip_hud(mut ctx: EguiContexts) {
    egui::Area::new("role_chip".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .show(ctx.ctx_mut(), |ui| {
            ui.add(
                egui::Button::new("admin") // TODO: read from NodeIdentity resource
                    .fill(egui::Color32::from_rgb(30, 30, 50)),
            );
        });
}
