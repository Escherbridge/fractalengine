//! Status-bar "Saving" indicator and "Unsaved edits" chip; see actions/AGENTS.md §scene-write-door.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::theme;

/// Warning text colour for the chip (matches the advisory notice family).
const UNSAVED_WARNING: egui::Color32 = egui::Color32::from_rgb(224, 176, 80);

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
            egui::Button::new(
                egui::RichText::new(format!("{dead} unsaved"))
                    .small()
                    .color(UNSAVED_WARNING),
            ),
        )
        .button_named(ui, "Unsaved edits");
    let mut action = None;
    egui::Popup::from_toggle_button_response(&chip)
        .id(egui::Id::new("footer_unsaved_edits"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_max_width(420.0_f32.min((ui.ctx().viewport_rect().width() - 32.0).max(120.0)));
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
    action
}

/// Applies a chosen action; a refused retry is already queued as an advisory by the door.
pub(crate) fn apply_unsaved_edit_action(
    ui_mgr: &mut UiManager,
    action: UnsavedEditAction,
    now_secs: f64,
) {
    match action {
        UnsavedEditAction::Retry(id) => {
            let label = crate::actions::node::dead_letters()
                .into_iter()
                .find(|letter| letter.id == id)
                .map(|letter| letter.label);
            crate::actions::node::retry_dead_letter(id);
            if let Some(label) = label {
                ui_mgr.show_toast(format!("Retrying {label}"), now_secs);
            }
        }
        UnsavedEditAction::Discard(id) => {
            crate::actions::node::discard_dead_letter(id);
        }
    }
}
