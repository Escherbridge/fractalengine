//! GLTF model import dialog.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::navigation_manager::NavigationManager;
use crate::theme;
use fe_runtime::messages::DbCommand;

pub fn render_gltf_import_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::GltfImport {
        ref mut file_path_buf,
        ref mut name_buf,
        ref mut position,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let mut close = false;

    let mut still_open = true;
    egui::Window::new("Import GLTF Model")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(340.0)
        .max_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("File Path:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(file_path_buf)
                        .hint_text("path/to/model.glb")
                        .desired_width(ui.available_width() - 70.0),
                );
                if ui
                    .add(egui::Button::new("Browse...").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("GLTF Models", &["glb", "gltf"])
                        .add_filter("All Files", &["*"])
                        .pick_file()
                    {
                        *file_path_buf = path.display().to_string();
                        if name_buf.is_empty() {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                *name_buf = stem.to_string();
                            }
                        }
                    }
                }
            });

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Display Name:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(name_buf)
                    .hint_text("My Model")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Position:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                for (axis, val) in ["X", "Y", "Z"].iter().zip(position.iter()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.label(
                        egui::RichText::new(format!("{:.2}", val))
                            .monospace()
                            .small(),
                    );
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Import").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let file_path = file_path_buf.trim().to_string();
                    let name = name_buf.trim().to_string();
                    let petal_id = nav.active_petal_id.clone().unwrap_or_default();
                    if !file_path.is_empty() && !petal_id.is_empty() {
                        db_tx
                            .send(DbCommand::ImportGltf {
                                petal_id,
                                name: if name.is_empty() {
                                    "Imported Model".to_string()
                                } else {
                                    name
                                },
                                file_path,
                                position: *position,
                            })
                            .ok();
                    }
                    close = true;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    close = true;
                }
            });
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}
