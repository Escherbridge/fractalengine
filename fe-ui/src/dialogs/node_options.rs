//! Node options dialog — the object-scoped comprehensive-verb surface for a
//! single node (contextual_controls_20260725 FR-2/FR-3/FR-4). Homes rename +
//! portal URL, plus the object verbs (delete-as-tombstone, duplicate, clear
//! properties, copy-API, report). The right-click context menu shares this
//! track's verb machinery (`context_menu::{Verb, render_verb_button}`); see
//! `dialogs/AGENTS.md` §context-menu for the verb→action map.

use bevy_egui::egui;

use super::context_menu::{render_verb_button, verb_action, Verb};
use super::ActiveDialog;
use crate::actions::{UiAction, UiManager};
use crate::theme;
use crate::ui_shell::modal::cascade_confirm_message;
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

    // Resolve the node's (petal_id, name, position) up front for Duplicate — a
    // read-only walk before the mutable-borrow window closure. `None` when the
    // node isn't in the loaded hierarchy (Duplicate then shows disabled).
    let dup_target: Option<(String, String, [f32; 3])> = hierarchy
        .verses
        .iter()
        .flat_map(|v| &v.fractals)
        .flat_map(|f| &f.petals)
        .flat_map(|p| p.nodes.iter().map(move |n| (p.id.clone(), n)))
        .find(|(_, n)| n.id == current_node_id)
        .map(|(petal_id, n)| (petal_id, n.name.clone(), n.position));

    let mut close = false;
    // Verbs that touch `ui_mgr` are queued here and flushed after the window, so
    // they don't collide with the `active_dialog` borrow held by the destructure.
    let mut pending_actions: Vec<UiAction> = Vec::new();

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

            // --- Object verbs (FR-3): duplicate / clear-props / copy-API / report ---
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Actions")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal_wrapped(|ui| {
                // Duplicate — functional here (we resolved the source's petal +
                // name + position); sends a fresh CreateNode offset nearby.
                if render_verb_button(ui, Verb::Duplicate, dup_target.is_some()) {
                    if let Some((petal_id, name, pos)) = &dup_target {
                        let _ = db_tx.send(DbCommand::CreateNode {
                            petal_id: petal_id.clone(),
                            name: format!("{name} (copy)"),
                            position: [pos[0] + 1.0, pos[1], pos[2] + 1.0],
                            correlation_id: None,
                        });
                        close = true;
                    }
                }

                // Clear properties — the husk distinction: keeps the node. Verb
                // → action goes through the shared, documented `verb_action` map.
                if render_verb_button(ui, Verb::ClearProperties, true) {
                    pending_actions.extend(verb_action(
                        Verb::ClearProperties,
                        &current_node_id,
                        false,
                    ));
                }

                // Copy API string (FR-4) — seam-gated; the clipboard write is
                // render-side (only egui `ctx` can touch the clipboard).
                let api = crate::gis::egress_strings::api_string_for(&current_node_id);
                if render_verb_button(ui, Verb::CopyApi, api.is_some()) {
                    if let Some(s) = &api {
                        ui.ctx().copy_text(s.clone());
                        pending_actions.extend(verb_action(Verb::CopyApi, &current_node_id, false));
                    }
                }

                // Report / query (FR-4) — seam-gated; report text to clipboard.
                let report = crate::gis::egress_strings::report_for(&current_node_id);
                if render_verb_button(ui, Verb::Report, report.is_some()) {
                    if let Some(r) = &report {
                        ui.ctx().copy_text(r.clone());
                        pending_actions.extend(verb_action(Verb::Report, &current_node_id, false));
                    }
                }
            });

            // --- Delete (two-step confirm; sync-safe tombstone + cascade, FR-2) ---
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
                    .on_hover_text(
                        "Delete this node (sync-safe tombstone). Cascades to child nodes.",
                    )
                    .clicked()
                {
                    *pending_delete = true;
                }
            } else {
                // Descendant count isn't resolvable from the flat UI hierarchy;
                // the confirm still names the cascade (Q-2: always confirm). An
                // authoritative count is a T1/T5 query follow-up (see AGENTS.md).
                ui.label(
                    egui::RichText::new(cascade_confirm_message(0)).color(theme::STATUS_OFFLINE),
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
                        // Real remove-a-node path (fixes the husk bug): route
                        // through the tombstone-cascade handler, not a raw drop.
                        pending_actions.push(UiAction::DeleteNode {
                            node_id: current_node_id.clone(),
                            cascade: true,
                        });
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
    for action in pending_actions {
        ui_mgr.push_action(action);
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
