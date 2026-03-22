use bevy_egui::{egui, EguiContexts};

pub fn role_chip_hud(mut ctx: EguiContexts) {
    let Ok(ectx) = ctx.ctx_mut() else { return };
    egui::Area::new("role_chip".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .show(ectx, |ui| {
            ui.add(egui::Button::new("admin").fill(egui::Color32::from_rgb(30, 30, 50)));
        });
}
