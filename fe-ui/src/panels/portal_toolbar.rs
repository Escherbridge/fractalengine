//! Portal webview toolbar (replaces the inspector panel while a portal is open).

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::theme;

pub(crate) fn right_portal_toolbar(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let max_w = ctx.viewport_rect().width() * 0.8;
    egui::SidePanel::right("portal_toolbar")
        .resizable(true)
        .default_width(400.0)
        .width_range(260.0..=max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0))
                .stroke(egui::Stroke::new(2.0, theme::BG_BUTTON)),
        )
        .show(ctx, |ui| {
            // Toolbar row: back, URL, close
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);

                // Back button
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("\u{2190}").size(16.0), // ←
                        )
                        .fill(theme::BG_BUTTON)
                        .min_size(egui::vec2(28.0, 24.0)),
                    )
                    .on_hover_text("Go back")
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::PortalGoBack);
                }

                ui.add_space(4.0);

                // URL label (truncated hostname — pre-cached, no per-frame parse)
                let display_url = ui_mgr.portal_hostname().to_string();
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("\u{1F310} {display_url}"))
                            .color(theme::TEXT_DIM)
                            .size(12.0),
                    )
                    .truncate(),
                );

                // Close button (right-aligned)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("\u{2715}").size(14.0), // ✕
                            )
                            .fill(egui::Color32::TRANSPARENT)
                            .small(),
                        )
                        .on_hover_text("Close portal")
                        .clicked()
                    {
                        ui_mgr.push_action(UiAction::ClosePortal);
                    }
                });
            });

            ui.separator();

            // The rest of the panel is empty — the native webview renders over it.
            ui.allocate_space(ui.available_size());
        });
}
