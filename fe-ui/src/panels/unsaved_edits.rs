//! Status-bar "Saving" indicator and "Unsaved edits" chip; see actions/AGENTS.md §scene-write-door.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::theme;

/// A player choice in the "Unsaved edits" list, applied after the panel closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnsavedEditAction {
    Retry(u64),
    Discard(u64),
}

/// Renders the saving spinner and the unsaved chip; returns the clicked row action, if any.
pub(crate) fn unsaved_edits_segment(
    ui: &mut egui::Ui,
    minimum: f32,
) -> Option<UnsavedEditAction> {
    let (queued, dead) = crate::actions::node::scene_door_counts();
    if queued == 0 && dead == 0 {
        return None;
    }
    if queued > 0 {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Saving {queued}..."))
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .selectable(false),
            )
            .on_hover_text("Waiting for the database; edits are kept and retried.");
        });
        ui.separator();
    }
    if dead == 0 {
        return None;
    }
    let chip = ui
        .add_sized(
            [0.0, minimum],
            // The advisory notice palette (contrast-checked in panels::tests).
            egui::Button::new(
                egui::RichText::new(format!("{dead} unsaved"))
                    .small()
                    .color(theme::NOTICE_TEXT),
            )
            .fill(theme::notice_fill(crate::notice::Severity::Advisory)),
        )
        .button_named(ui, "Unsaved edits");
    let mut action = None;
    egui::Popup::from_toggle_button_response(&chip)
        .id(egui::Id::new("footer_unsaved_edits"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            crate::native_accessibility::surface::popup(ui.ctx());
            // Up to 32 letters: scroll inside the viewport like the other footer popups.
            super::status_bar::bounded_details(ui, |ui| {
                ui.heading("Unsaved edits");
                for letter in crate::actions::node::dead_letters() {
                    ui.horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            ui.label(letter.label);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&letter.reason)
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                )
                                .wrap(),
                            );
                        });
                        if ui
                            .add_sized([0.0, minimum], egui::Button::new("Retry"))
                            .button_named(ui, format!("Retry {}", letter.label))
                            .clicked()
                        {
                            action = Some(UnsavedEditAction::Retry(letter.id));
                        }
                        if ui
                            .add_sized([0.0, minimum], egui::Button::new("Discard"))
                            .button_named(ui, format!("Discard {}", letter.label))
                            .clicked()
                        {
                            action = Some(UnsavedEditAction::Discard(letter.id));
                        }
                    });
                    ui.separator();
                }
            });
        });
    action
}

/// Applies a chosen action; a gate refusal of a retry is already queued as an advisory by the door.
pub(crate) fn apply_unsaved_edit_action(
    ui_mgr: &mut UiManager,
    action: UnsavedEditAction,
    now_secs: f64,
) {
    use crate::actions::node::GuiSend;
    match action {
        UnsavedEditAction::Retry(id) => {
            let Some(label) = crate::actions::node::dead_letters()
                .into_iter()
                .find(|letter| letter.id == id)
                .map(|letter| letter.label)
            else {
                return;
            };
            match crate::actions::node::retry_dead_letter(id) {
                GuiSend::Sent | GuiSend::Waiting => {
                    ui_mgr.show_toast(format!("Retrying {label}"), now_secs)
                }
                GuiSend::Closed | GuiSend::Busy => ui_mgr.show_error(
                    format!("{label} was not saved: Database connection closed. It stays under Unsaved edits."),
                    now_secs,
                ),
                GuiSend::Refused(_) => {}
            }
        }
        UnsavedEditAction::Discard(id) => {
            crate::actions::node::discard_dead_letter(id);
        }
    }
}
