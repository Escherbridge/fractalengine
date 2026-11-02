//! Node options dialog — the object-scoped comprehensive-verb surface for a
//! single node (contextual_controls_20260725 FR-2/FR-3/FR-4). Homes rename +
//! portal URL, plus the object verbs (delete-as-tombstone, duplicate, clear
//! properties, copy-API, report). The right-click context menu shares this
//! track's verb machinery (`context_menu::{Verb, render_verb_button}`); see
//! `dialogs/AGENTS.md` §context-menu for the verb→action map.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::context_menu::{render_verb_button, verb_action, Verb};
use super::ActiveDialog;
use crate::actions::{UiAction, UiManager};
use crate::theme;
use crate::ui_shell::modal::{
    cascade_confirm_message, close_confirmation_scope, confirmation_scope,
};
use crate::verse_manager::VerseManager;
use fe_runtime::messages::{CallerAuth, DbCommand};

pub fn render_node_options_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    hierarchy: &mut VerseManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &crate::navigation_manager::NavigationManager,
    local_role: &crate::plugin::LocalUserRole,
) {
    let ActiveDialog::NodeOptions {
        ref node_id,
        ref mut node_name_buf,
        ref mut webpage_url_buf,
        ref mut pending_delete,
        ref mut descendant_count,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let current_node_id = node_id.clone();
    let confirmation = confirmation_scope(ctx, "node_options", &current_node_id);

    // Current display name, for the Save-time rename diff (a read-only walk
    // before the mutable-borrow window closure).
    let current_name: Option<String> = hierarchy
        .all_nodes()
        .find(|n| n.id == current_node_id)
        .map(|n| n.name.clone());
    let current_url = hierarchy
        .all_nodes()
        .find(|node| node.id == current_node_id)
        .and_then(|node| node.webpage_url.clone());

    let storage = hierarchy.node_url_storage(&current_node_id);
    let storage_allows_save = storage.is_none_or(|storage| storage.supports_save());
    let can_edit = local_role.can_edit()
        && crate::actions::portal::node_scope(hierarchy, &current_node_id)
            .is_some_and(|scope| nav.active_scope().as_ref() == Some(&scope));
    let mut close = false;
    // Verbs that touch `ui_mgr` are queued here and flushed after the window, so
    // they don't collide with the `active_dialog` borrow held by the destructure.
    let mut pending_actions: Vec<UiAction> = Vec::new();

    let mut still_open = true;
    crate::ui_shell::layout_policy::bounded_window(ctx, "Node Options")
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
        .show_with_targets(ctx, |ui| {
            if !can_edit {
                ui.label("Navigate to this object's Petal with Editor access to edit it.");
            }
            let name_label = ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            ui.add_enabled(
                can_edit,
                crate::ui_shell::layout_policy::text_input(ui, node_name_buf).desired_width(f32::INFINITY),
            )
            .labelled(ui, &name_label);

            ui.add_space(6.0);
            let url_label = ui.label(
                egui::RichText::new("Portal URL:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add_enabled(
                can_edit && storage_allows_save,
                crate::ui_shell::layout_policy::text_input(ui, webpage_url_buf)
                    .hint_text("https://\u{2026}")
                    .desired_width(f32::INFINITY),
            )
            .labelled(ui, &url_label);
            ui.label(
                egui::RichText::new(
                    "This URL will be rendered in the embedded webview when selected.",
                )
                .small()
                .italics()
                .color(theme::TEXT_MUTED),
            );

            if let Some(explanation) = storage.and_then(|storage| storage.explanation()) {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(explanation)
                            .small()
                            .color(theme::TEXT_DIM),
                    )
                    .wrap(),
                );
            }
            let validated_url = crate::actions::portal::validate_url(webpage_url_buf);
            if let Err(reason) = &validated_url {
                ui.colored_label(theme::STATUS_OFFLINE, reason);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_edit && validated_url.is_ok(),
                        egui::Button::new("Save").fill(theme::BG_SAVE),
                    )
                    .observed(ui)
                    .clicked()
                {
                    if let Ok(url) = &validated_url {
                        if *url != current_url && storage_allows_save {
                            pending_actions.push(UiAction::SaveNodeUrl {
                                node_id: current_node_id.clone(),
                                url: url.clone(),
                            });
                        }
                    }
                    // The Name field is the node-rename surface: a changed,
                    // non-empty name routes the T4 spine op (Editor+ enforced
                    // DB-side via CallerAuth::Local, N-5); the tree updates on
                    // the NodeRenamed result, not optimistically.
                    if let Some(new_name) = rename_request(current_name.as_deref(), node_name_buf) {
                        if db_tx
                            .send(DbCommand::RenameNode {
                                node_id: current_node_id.clone(),
                                new_name,
                                auth: CallerAuth::Local,
                            })
                            .is_err()
                        {
                            bevy::log::warn!(
                                "db_sender channel closed \u{2014} RenameNode not sent"
                            );
                        }
                    }
                    close = true;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .observed(ui)
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
                // Duplicate — the T4 spine op (full row copy, " (copy)" name,
                // +1m x/z, path-binding keys stripped); replies NodeCreated,
                // which fe-ui already applies. No fake CreateNode copy.
                if ui
                    .add_enabled_ui(can_edit, |ui| render_verb_button(ui, Verb::Duplicate, true))
                    .inner
                {
                    pending_actions.extend(verb_action(Verb::Duplicate, &current_node_id, false));
                    close = true;
                }

                if ui
                    .add_enabled_ui(can_edit, |ui| {
                        crate::panels::widgets::confirm_button(
                            ui,
                            confirmation.with("clear_properties"),
                            "Clear Properties",
                            "Confirm clear",
                            "Clear node properties",
                        )
                    })
                    .inner
                {
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
                    .add_enabled(
                        can_edit,
                        egui::Button::new(
                            egui::RichText::new("Delete Node").color(egui::Color32::WHITE),
                        )
                        .fill(theme::BG_DANGER),
                    )
                    .observed(ui)
                    .on_hover_text("Delete this node and its children. This cannot be undone.")
                    .clicked()
                {
                    // Arm the confirm and fetch the authoritative subtree size
                    // (spine query); the generic copy shows until it lands.
                    *pending_delete = true;
                    *descendant_count = None;
                    if db_tx
                        .send(DbCommand::CountNodeDescendants {
                            node_id: current_node_id.clone(),
                        })
                        .is_err()
                    {
                        bevy::log::warn!(
                            "db_sender channel closed \u{2014} CountNodeDescendants not sent"
                        );
                    }
                }
            } else {
                // Q-2: always confirm, with the live descendant count once the
                // NodeDescendantCount result lands (db_results stashes it here).
                ui.label(
                    egui::RichText::new(cascade_confirm_message(*descendant_count))
                        .color(theme::STATUS_OFFLINE),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            can_edit && descendant_count.is_some(),
                            egui::Button::new(
                                egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE),
                            )
                            .fill(theme::BG_DANGER),
                        )
                        .observed(ui)
                        .clicked()
                    {
                        // Real remove-a-node path (fixes the husk bug): route
                        // through the tombstone-cascade handler, not a raw drop.
                        if let Some(action) =
                            confirmed_node_delete(&current_node_id, *descendant_count)
                        {
                            pending_actions.push(action);
                            close = true;
                        }
                    }
                    if ui
                        .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                        .observed(ui)
                        .clicked()
                    {
                        *pending_delete = false;
                    }
                });
            }
        });

    if !still_open || close {
        close_confirmation_scope(ctx, "node_options");
        ui_mgr.close_dialog();
    }
    for action in pending_actions {
        ui_mgr.push_action(action);
    }
}

/// No node deletion is emitted before the authoritative count arrives.
fn confirmed_node_delete(node_id: &str, count: Option<usize>) -> Option<UiAction> {
    count.map(|_| UiAction::DeleteNode {
        node_id: node_id.to_owned(),
        cascade: true,
    })
}

/// The rename to persist on Save, if any: the trimmed buffer when it is
/// non-empty and differs from the current name. Pure — the Save wiring's
/// decision is testable without egui.
pub(crate) fn rename_request(current_name: Option<&str>, name_buf: &str) -> Option<String> {
    let trimmed = name_buf.trim();
    if trimmed.is_empty() || Some(trimmed) == current_name {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_request_fires_only_on_a_real_change() {
        assert!(confirmed_node_delete("node-a", None).is_none());
        for count in [0, 1, 7] {
            assert!(
                matches!(confirmed_node_delete("node-a", Some(count)), Some(UiAction::DeleteNode { node_id, cascade: true }) if node_id == "node-a")
            );
        }
        let ctx = egui::Context::default();
        let first = confirmation_scope(&ctx, "node_options", "node-a");
        let second = confirmation_scope(&ctx, "node_options", "node-b");
        assert_ne!(
            first.with("clear_properties"),
            second.with("clear_properties")
        );

        assert_eq!(rename_request(Some("Old"), "New"), Some("New".to_string()));
        assert_eq!(
            rename_request(Some("Old"), "  New  "),
            Some("New".to_string()),
            "buffer is trimmed before the diff"
        );
        assert_eq!(rename_request(Some("Same"), "Same"), None);
        assert_eq!(rename_request(Some("Same"), " Same "), None);
        assert_eq!(rename_request(Some("Old"), ""), None, "empty never renames");
        assert_eq!(rename_request(Some("Old"), "   "), None);
        // Unknown current name (node not in the loaded tree): any non-empty
        // buffer counts as a change.
        assert_eq!(rename_request(None, "Name"), Some("Name".to_string()));
    }
}
