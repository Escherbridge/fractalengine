//! Petal hexon manifest editor dialog.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::{UiAction, UiManager};
use crate::terrain_map::manifest::ManifestHexonEntry;
use crate::theme;

pub fn render_petal_manifest(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let ActiveDialog::PetalManifest {
        ref petal_id,
        ref petal_name,
        ref mut manifest,
        ref available_hexon_ids,
        ref mut add_hexon_id_buf,
        ref mut add_hexon_type_buf,
        ref mut render_distance_buf,
        ref mut dirty,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let current_petal_id = petal_id.clone();
    let mut close = false;
    let mut still_open = true;
    let mut actions: Vec<UiAction> = Vec::new();

    egui::Window::new(format!("Petal Manifest — {}", petal_name))
        .open(&mut still_open)
        .collapsible(false)
        .resizable(true)
        .default_width(600.0)
        .min_width(450.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Hexon requirements for this petal. Nodes referencing missing hexons will show a fallback sign.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(8.0);

            // --- Render distance ---
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Render distance:").color(theme::TEXT_SECTION));
                let resp = ui.add(egui::TextEdit::singleline(render_distance_buf).desired_width(80.0));
                if resp.changed() {
                    if let Ok(v) = render_distance_buf.parse::<f32>() {
                        manifest.render_distance = v;
                        *dirty = true;
                    }
                }
                ui.label(egui::RichText::new("units").small().color(theme::TEXT_DIM));
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // --- Required hexons table ---
            ui.label(egui::RichText::new("Required Hexons").color(theme::TEXT_SECTION).strong());
            ui.add_space(4.0);

            if manifest.hexons.is_empty() {
                ui.label(
                    egui::RichText::new("No hexons assigned. Add one below.")
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
            } else {
                let mut remove_idx: Option<usize> = None;

                egui::Grid::new("manifest_hexons_grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        // Header
                        ui.label(egui::RichText::new("Hexon ID").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("Type").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("Required").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("Status").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("").color(theme::TEXT_SECTION));
                        ui.end_row();

                        for (i, entry) in manifest.hexons.iter_mut().enumerate() {
                            ui.label(&entry.hexon_id);
                            ui.label(&entry.hexon_type);

                            if ui.checkbox(&mut entry.required, "").changed() {
                                *dirty = true;
                            }

                            // Status: installed locally?
                            let installed = available_hexon_ids.contains(&entry.hexon_id);
                            if installed {
                                ui.label(
                                    egui::RichText::new("Installed").color(egui::Color32::from_rgb(100, 200, 100)),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("Missing").color(egui::Color32::from_rgb(200, 100, 80)),
                                );
                            }

                            if ui
                                .add(egui::Button::new("X").fill(theme::BG_DANGER))
                                .on_hover_text("Remove from manifest")
                                .clicked()
                            {
                                remove_idx = Some(i);
                            }
                            ui.end_row();
                        }
                    });

                if let Some(idx) = remove_idx {
                    manifest.hexons.remove(idx);
                    *dirty = true;
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // --- Add hexon ---
            ui.label(egui::RichText::new("Add Hexon").color(theme::TEXT_SECTION).strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("ID:");
                ui.add(
                    egui::TextEdit::singleline(add_hexon_id_buf)
                        .hint_text("hexon-id")
                        .desired_width(200.0),
                );
                ui.label("Type:");
                egui::ComboBox::from_id_salt("hexon_type_combo")
                    .selected_text(add_hexon_type_buf.as_str())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for t in &[
                            "Scene", "Model", "Material", "Skybox", "Terrain",
                            "TerrainTileset", "GpxCollection", "Surface",
                            "Sound", "VisualLayer", "Theme", "Bundle",
                        ] {
                            ui.selectable_value(add_hexon_type_buf, t.to_string(), *t);
                        }
                    });

                let can_add = !add_hexon_id_buf.trim().is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Add").fill(theme::BG_SAVE))
                    .clicked()
                {
                    manifest.hexons.push(ManifestHexonEntry {
                        hexon_id: add_hexon_id_buf.trim().to_string(),
                        hexon_type: add_hexon_type_buf.clone(),
                        required: true,
                    });
                    add_hexon_id_buf.clear();
                    *dirty = true;
                }
            });

            // --- Sharing hint ---
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Sharing: peers joining this petal will be prompted to download missing hexons from connected seeders.")
                    .small()
                    .italics()
                    .color(theme::TEXT_MUTED),
            );

            // JSON preview (collapsible)
            ui.add_space(8.0);
            egui::CollapsingHeader::new(
                egui::RichText::new("JSON Schema Preview").small().color(theme::TEXT_DIM),
            )
            .default_open(false)
            .show(ui, |ui| {
                let json = serde_json::to_string_pretty(&*manifest).unwrap_or_default();
                ui.add(
                    egui::TextEdit::multiline(&mut json.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
            });

            // --- Save / Close ---
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let save_label = if *dirty { "Save *" } else { "Save" };
                if ui
                    .add_enabled(*dirty, egui::Button::new(save_label).fill(theme::BG_SAVE))
                    .clicked()
                {
                    actions.push(UiAction::PetalManifestSave {
                        petal_id: current_petal_id.clone(),
                        manifest: manifest.clone(),
                    });
                    *dirty = false;
                }
                if ui.add(egui::Button::new("Close").fill(theme::BG_BUTTON)).clicked() {
                    close = true;
                }
            });
        });

    for action in actions {
        ui_mgr.push_action(action);
    }

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}
