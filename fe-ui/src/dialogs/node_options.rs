//! Node options dialog (rename + portal URL + delete).

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

pub fn render_node_options_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    hierarchy: &mut VerseManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::NodeOptions {
        ref node_id,
        ref mut node_name_buf,
        ref mut webpage_url_buf,
        ref mut pending_delete,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let current_node_id = node_id.clone();
    let mut close = false;

    let mut still_open = true;
    egui::Window::new("Node Options")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            ui.add(egui::TextEdit::singleline(node_name_buf).desired_width(f32::INFINITY));

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Portal URL:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(webpage_url_buf)
                    .hint_text("https://\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            ui.label(
                egui::RichText::new(
                    "This URL will be rendered in the embedded webview when selected.",
                )
                .small()
                .italics()
                .color(theme::TEXT_MUTED),
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Save").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let url = webpage_url_buf.trim().to_string();
                    let url_opt = if url.is_empty() { None } else { Some(url) };
                    node_options_save_url(hierarchy, &current_node_id, url_opt, db_tx);
                    close = true;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    close = true;
                }
            });

            // --- Delete (two-step confirm; mirrors entity_settings.rs) ---
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            if !*pending_delete {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Delete Node").color(egui::Color32::WHITE),
                        )
                        .fill(theme::BG_DANGER),
                    )
                    .clicked()
                {
                    *pending_delete = true;
                }
            } else {
                ui.label(
                    egui::RichText::new("Are you sure? This cannot be undone.")
                        .color(theme::STATUS_OFFLINE),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE),
                            )
                            .fill(theme::BG_DANGER),
                        )
                        .clicked()
                    {
                        db_tx
                            .send(DbCommand::DeleteNode {
                                node_id: current_node_id.clone(),
                            })
                            .ok();
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                        .clicked()
                    {
                        *pending_delete = false;
                    }
                });
            }
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

pub fn node_options_save_url(
    hierarchy: &mut VerseManager,
    node_id: &str,
    url: Option<String>,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    hierarchy.update_node_url(node_id, url.clone());
    if db_tx
        .send(DbCommand::UpdateNodeUrl {
            node_id: node_id.to_string(),
            url,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — UpdateNodeUrl (node options) not persisted");
    }
}
