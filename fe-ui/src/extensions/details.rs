//! Details view of one installation, and the update flow (re-verify, re-prompt on a delta, keep the old version until the new one runs).

use bevy_egui::egui;

use super::consent::{ConsentRequest, ConsentState};
use super::panel::{DetailsTab, ExtensionOp, ExtensionsPanelState, InstalledRow, PendingConfirm, PanelView, RowStatus};
use super::permissions;
use crate::accessibility::ResponseExt;
use crate::theme;

/// Where an update of one installation stands; the previous version runs until `Active`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStage {
    /// The newer package is being fetched and verified.
    Verifying,
    /// Consent dialog is open for a capability delta.
    AwaitingConsent,
    /// The new version is starting; the previous one still runs.
    Activating { version: String },
    /// The new version runs and the previous one was retired.
    Active { version: String },
    /// The new version failed to start; the previous version kept running.
    RolledBack { reason: String },
}

impl UpdateStage {
    /// Row text for the stage.
    pub fn label(&self) -> String {
        match self {
            Self::Verifying => "verifying update".into(),
            Self::AwaitingConsent => "update awaiting your review".into(),
            Self::Activating { version } => format!("starting {version}, previous version still runs"),
            Self::Active { version } => format!("updated to {version}"),
            Self::RolledBack { reason } => format!("update failed, previous version kept: {reason}"),
        }
    }

    fn in_flight(&self) -> bool {
        matches!(self, Self::Verifying | Self::AwaitingConsent | Self::Activating { .. })
    }
}

/// Start an update; ignored while one is already in flight for the installation.
pub fn begin_update(state: &mut ExtensionsPanelState, installation_id: &str) {
    if state.updates.get(installation_id).is_some_and(UpdateStage::in_flight) {
        return;
    }
    state.updates.insert(installation_id.to_owned(), UpdateStage::Verifying);
    state.queue(ExtensionOp::Update { installation_id: installation_id.to_owned() }, "Verifying update");
}

/// The bridge staged a newer version: prompt only when the grants differ from the consent in force.
pub(super) fn on_update_offer(state: &mut ExtensionsPanelState, consent: &mut ConsentState, request: ConsentRequest) {
    let installation_id = request.installation_id.clone();
    let digest = request.capability_digest.clone();
    if consent.request(request) {
        state.reviewing = Some(installation_id.clone());
        state.updates.insert(installation_id, UpdateStage::AwaitingConsent);
    } else {
        // The bridge re-checks that this digest is the consent on record before it activates (R19 consent-missing).
        state.queue(ExtensionOp::ActivateUpdate { installation_id, capability_digest: digest }, "Updating");
    }
}

pub(super) fn on_update_stage(state: &mut ExtensionsPanelState, installation_id: String, stage: UpdateStage) {
    state.working = None;
    if let UpdateStage::RolledBack { reason } = &stage {
        state.error = Some(format!("Update failed, previous version kept: {reason}"));
    }
    state.updates.insert(installation_id, stage);
}

/// Draw one installation: header, tab strip, then Overview or Permissions.
pub fn draw(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, installation_id: &str, tab: DetailsTab) {
    let Some(row) = state.rows.iter().find(|row| row.installation_id == installation_id).cloned() else {
        state.view = PanelView::Installed;
        return;
    };
    if ui.small_button("Back to installed").button_named(ui, "Back to installed extensions").clicked() {
        state.view = PanelView::Installed;
        return;
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new(format!("{} {}", row.name, row.version)).heading());
    ui.label(egui::RichText::new(format!("{} · {} · {}", row.package_id, row.publisher, row.trust.label())).small().color(theme::TEXT_MUTED));
    ui.label(egui::RichText::new(row.status.label()).small());
    ui.horizontal(|ui| {
        for (target, label) in [(DetailsTab::Overview, "Overview"), (DetailsTab::Permissions, "Permissions")] {
            if ui.selectable_label(tab == target, label).button_named(ui, format!("{label} of {}", row.package_id)).clicked() {
                state.view = PanelView::Details { installation_id: installation_id.to_owned(), tab: target };
            }
        }
    });
    ui.separator();
    match tab {
        DetailsTab::Overview => overview(ui, state, &row),
        DetailsTab::Permissions => permissions::draw(ui, state, &row),
    }
}

fn overview(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, row: &InstalledRow) {
    if !row.description.is_empty() {
        ui.label(&row.description);
        ui.add_space(6.0);
    }
    update_block(ui, state, row);
    section_title(ui, "Bindings");
    if row.bindings.is_empty() {
        ui.label(egui::RichText::new("No bindings use this extension").small().color(theme::TEXT_MUTED));
    }
    // Needs consent: Enable asks the bridge, which answers with the consent dialog (R19 consent-missing), never runs it.
    let can_toggle = matches!(row.status, RowStatus::Enabled | RowStatus::Disabled | RowStatus::NeedsConsent) && !state.busy();
    for binding in &row.bindings {
        ui.horizontal(|ui| {
            ui.label(&binding.label);
            let (label, enable) = if binding.enabled { ("Disable", false) } else { ("Enable", true) };
            if ui
                .add_enabled(can_toggle, egui::Button::new(label))
                .button_named(ui, format!("{label} {} for {}", binding.label, row.package_id))
                .clicked()
            {
                state.request_toggle(&row.installation_id, &binding.binding_id, enable);
            }
        });
    }
    section_title(ui, "Counters");
    if row.counters.is_empty() {
        ui.label(egui::RichText::new("No counters yet").small().color(theme::TEXT_MUTED));
    }
    for (label, value) in &row.counters {
        ui.label(egui::RichText::new(format!("{label}: {value}")).monospace().small());
    }
    section_title(ui, "Audit trail");
    if row.audit.is_empty() {
        ui.label(egui::RichText::new("Nothing recorded yet").small().color(theme::TEXT_MUTED));
    }
    for line in &row.audit {
        ui.label(egui::RichText::new(format!("{} · {} · {}", line.at, line.actor, line.action)).small().color(theme::TEXT_MUTED));
    }
    ui.add_space(12.0);
    uninstall_row(ui, state, row);
}

fn update_block(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, row: &InstalledRow) {
    let stage = state.updates.get(&row.installation_id).cloned();
    if let Some(stage) = &stage {
        ui.label(egui::RichText::new(stage.label()).small().color(theme::TEXT_SECTION));
    }
    let Some(newer) = &row.update_available else { return };
    let busy = stage.as_ref().is_some_and(UpdateStage::in_flight) || state.busy();
    if ui.add_enabled(!busy, egui::Button::new(format!("Update to {newer}"))).button_named(ui, format!("Update {} to {newer}", row.package_id)).clicked() {
        begin_update(state, &row.installation_id);
    }
    ui.add_space(6.0);
}

fn uninstall_row(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, row: &InstalledRow) {
    let confirming = matches!(state.pending_confirm(), Some(PendingConfirm::Uninstall(id)) if *id == row.installation_id);
    let idle = !state.busy();
    ui.horizontal(|ui| {
        if confirming {
            ui.label(egui::RichText::new(format!("Remove {} and stop all its bindings?", row.name)).small());
            if ui
                .add_enabled(idle, egui::Button::new("Uninstall"))
                .button_named(ui, format!("Confirm uninstall {}", row.package_id))
                .clicked()
            {
                state.accept_confirm();
            }
            if ui.button("Keep").button_named(ui, format!("Keep {}", row.package_id)).clicked() {
                state.cancel_confirm();
            }
        } else if ui
            .add_enabled(idle, egui::Button::new("Uninstall"))
            .button_named(ui, format!("Uninstall {}", row.package_id))
            .clicked()
        {
            state.ask_confirm(PendingConfirm::Uninstall(row.installation_id.clone()));
        }
    });
}

pub(super) fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).strong().color(theme::TEXT_SECTION));
}
