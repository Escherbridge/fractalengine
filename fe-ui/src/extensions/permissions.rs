//! Permissions view: the consent record, the capability list in plain language, and revoke consent.

use bevy_egui::egui;

use super::details::section_title;
use super::panel::{ExtensionsPanelState, InstalledRow, PendingConfirm};
use crate::accessibility::ResponseExt;
use crate::theme;

/// Draw the consent record and grants of one installation.
pub fn draw(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, row: &InstalledRow) {
    let Some(consent) = &row.consent else {
        ui.label(egui::RichText::new("No consent on record. Enable a binding to review its permissions.").color(theme::TEXT_MUTED));
        return;
    };
    let idle = !state.busy();
    ui.label(egui::RichText::new(format!("Allowed by {} on {}", consent.consented_by, consent.consented_at)).small().color(theme::TEXT_MUTED));
    for section in &consent.sections {
        section_title(ui, &section.title);
        if section.lines.is_empty() {
            ui.label(egui::RichText::new("None").small().color(theme::TEXT_MUTED));
        }
        for line in &section.lines {
            ui.label(egui::RichText::new(line).monospace().small());
        }
    }
    ui.add_space(12.0);
    let confirming = matches!(state.pending_confirm(), Some(PendingConfirm::Revoke(id)) if *id == row.installation_id);
    ui.horizontal(|ui| {
        if confirming {
            ui.label(egui::RichText::new("Stops every binding until you allow it again.").small());
            if ui
                .add_enabled(idle, egui::Button::new("Revoke"))
                .button_named(ui, format!("Confirm revoke consent for {}", row.package_id))
                .clicked()
            {
                state.accept_confirm();
            }
            if ui.button("Keep").button_named(ui, format!("Keep consent for {}", row.package_id)).clicked() {
                state.cancel_confirm();
            }
        } else if ui
            .add_enabled(idle, egui::Button::new("Revoke consent"))
            .button_named(ui, format!("Revoke consent for {}", row.package_id))
            .clicked()
        {
            state.ask_confirm(PendingConfirm::Revoke(row.installation_id.clone()));
        }
    });
}
