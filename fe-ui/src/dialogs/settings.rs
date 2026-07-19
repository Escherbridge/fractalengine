//! D-78 application settings window (p2p_asset_streaming_20260718 FR-7).
//! First live knobs: `AppSettings.mesh_budget_ceiling` (the cheapest first
//! knob per the decision record — `MeshInstanceBudget.ceiling` is already a
//! runtime field) and `AppSettings.render_distance`. Reads/writes
//! `AppSettings` directly (no `UiAction` round-trip — mirrors how
//! `petal_manifest.rs` edits its buffer in place before an explicit Save).
//! See `dialogs/AGENTS.md` §settings.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::settings::AppSettings;
use crate::theme;

pub fn settings_window(ctx: &egui::Context, ui_mgr: &mut UiManager, app_settings: &mut AppSettings) {
    if !matches!(ui_mgr.active_dialog, ActiveDialog::Settings) {
        return;
    }

    let mut close = false;
    egui::Window::new("Settings")
        .collapsible(true)
        .resizable(true)
        .default_width(320.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Rendering")
                    .strong()
                    .color(theme::TEXT_SECTION),
            );
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Render distance")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(
                    egui::DragValue::new(&mut app_settings.render_distance)
                        .speed(1.0)
                        .range(1.0..=f32::MAX)
                        .suffix(" wu"),
                )
                .on_hover_text(
                    "Global default render distance; PetalManifest.render_distance overrides per-petal",
                );
            });

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Mesh budget ceiling")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                // usize has no native egui DragValue support; round-trip via u64.
                let mut ceiling = app_settings.mesh_budget_ceiling as u64;
                if ui
                    .add(egui::DragValue::new(&mut ceiling).range(1..=u32::MAX as u64))
                    .on_hover_text("MeshInstanceBudget.ceiling — the mesh-instance watchdog gate")
                    .changed()
                {
                    app_settings.mesh_budget_ceiling = ceiling as usize;
                }
            });

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Additional knobs (stamp caps, tile source mode, camera, P2P relay/peer config) land as AppSettings grows further fields.",
                )
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
            );

            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                close = true;
            }
        });

    if close {
        ui_mgr.close_dialog();
    }
}
