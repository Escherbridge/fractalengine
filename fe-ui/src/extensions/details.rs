//! Details view of one installation, and the update flow (re-verify, re-prompt on a delta, keep the old version until the new one runs).

use bevy_egui::egui;
use fe_hexon::registry_source::{RegistryEntry, VerifiedAttestation, VerifiedDetail, ACCESS_LICENSED};

use super::browse::{human_size, BrowseEvent};
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

/// Licence state of one licensed package on this host (the verified token, never a document field).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EntitlementState {
    /// Not asked yet.
    #[default]
    Unknown,
    /// A token request is in flight.
    Checking,
    /// A verified token covers this device; installing is allowed.
    Held { expires: Option<String> },
    /// The registry holds no licence for this licensee and device.
    Missing,
    /// The registry or the token check refused; the reason is shown verbatim.
    Refused(String),
}

impl EntitlementState {
    /// Row text for the state.
    pub fn label(&self) -> String {
        match self {
            Self::Unknown => "Licence not checked".into(),
            Self::Checking => "Checking licence".into(),
            Self::Held { expires: Some(expires) } => format!("Licensed until {expires}"),
            Self::Held { expires: None } => "Licensed".into(),
            Self::Missing => "No licence yet".into(),
            Self::Refused(reason) => format!("Licence refused: {reason}"),
        }
    }

    /// True when the install may proceed.
    pub fn held(&self) -> bool {
        matches!(self, Self::Held { .. })
    }
}

/// One version line of a catalogue package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreVersion {
    pub version: String,
    pub size_bytes: u64,
    pub published_at: Option<String>,
    pub yanked: bool,
}

/// A catalogue package as the Details view shows it (copied from the verified detail; fe-ui verifies nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDetail {
    /// The build an install fetches (newest listed).
    pub entry: RegistryEntry,
    pub name: String,
    pub description: String,
    pub publisher: String,
    /// `free` or `licensed`.
    pub access: String,
    pub categories: Vec<String>,
    pub versions: Vec<StoreVersion>,
    /// Why the package cannot be installed (kill list); no action is offered while set.
    pub unavailable: Option<String>,
    /// True only when a counter-signature verified against a trust root.
    pub reviewed: bool,
    /// The registry offers a licence page for this package.
    pub can_acquire: bool,
    pub entitlement: EntitlementState,
    /// Versions the host skipped, with the reason.
    pub notes: Vec<String>,
}

impl StoreDetail {
    /// Build the view from the verified detail; `attestation` is `None` when the header phase has not run or failed,
    /// which never shows "Reviewed".
    pub fn from_verified(verified: &VerifiedDetail, attestation: Option<&VerifiedAttestation>, entitlement: EntitlementState) -> Self {
        let mut entry = verified.listed.to_registry_entry();
        if let Some(latest) = verified.detail.versions.iter().find(|version| version.version == entry.version) {
            entry.size = latest.size_bytes;
        }
        Self {
            entry,
            name: verified.detail.name.clone(),
            description: verified.detail.description.clone(),
            publisher: verified.detail.publisher.name.clone(),
            access: verified.detail.access.clone(),
            categories: verified.detail.categories.clone(),
            versions: verified
                .detail
                .versions
                .iter()
                .map(|version| StoreVersion {
                    version: version.version.clone(),
                    size_bytes: version.size_bytes,
                    published_at: version.published_at.clone(),
                    yanked: version.yanked,
                })
                .collect(),
            unavailable: verified.revoked.then(|| "Revoked by the registry".to_owned()),
            reviewed: attestation.is_some_and(VerifiedAttestation::is_reviewed),
            can_acquire: verified.acquire_url.is_some(),
            entitlement,
            notes: verified.refused.iter().map(|refusal| format!("{}: {}", refusal.what, refusal.reason)).collect(),
        }
    }

    fn licensed(&self) -> bool {
        self.access == ACCESS_LICENSED
    }
}

/// Draw one catalogue package: publisher, versions, review state, licence and the install action.
pub fn draw_store(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, package_id: &str) {
    if ui.small_button("Back to results").button_named(ui, "Back to extension results").clicked() {
        state.view = PanelView::Browse;
        return;
    }
    ui.add_space(4.0);
    let Some(detail) = state.browse.detail.clone().filter(|detail| detail.entry.package_id == package_id) else {
        // The bridge's refusal is shown verbatim (a tampered or mismatched detail document), never a generic line.
        match (&state.browse.detail_error, state.working.is_some()) {
            (Some(reason), _) => {
                ui.label(egui::RichText::new(format!("Details are unavailable: {reason}")).color(theme::STATUS_OFFLINE));
            }
            (None, true) => {
                ui.label(egui::RichText::new("Loading details").color(theme::TEXT_MUTED));
            }
            (None, false) => {
                ui.label(egui::RichText::new("Details are unavailable").color(theme::TEXT_MUTED));
            }
        }
        return;
    };
    let title = if detail.name.is_empty() { package_id } else { detail.name.as_str() };
    ui.label(egui::RichText::new(format!("{title} {}", detail.entry.version)).heading());
    let mut meta = vec![package_id.to_owned()];
    if !detail.publisher.is_empty() {
        meta.push(detail.publisher.clone());
    }
    ui.label(egui::RichText::new(meta.join(" · ")).small().color(theme::TEXT_MUTED));
    if detail.reviewed {
        ui.label(egui::RichText::new("Reviewed").small().color(theme::TEXT_ROLE_EDITOR)).named(ui, format!("Reviewed {package_id}"));
    }
    if !detail.description.is_empty() {
        ui.add_space(6.0);
        ui.label(&detail.description);
    }
    if !detail.categories.is_empty() {
        ui.label(egui::RichText::new(detail.categories.join(", ")).small().color(theme::TEXT_MUTED));
    }
    if let Some(reason) = &detail.unavailable {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("Unavailable: {reason}")).color(theme::STATUS_OFFLINE));
    } else {
        store_actions(ui, state, &detail);
    }
    section_title(ui, "Versions");
    for version in &detail.versions {
        let mut line = format!("{} · {}", version.version, human_size(version.size_bytes));
        if let Some(at) = &version.published_at {
            line.push_str(&format!(" · {at}"));
        }
        if version.yanked {
            line.push_str(" · withdrawn from new installs");
        }
        ui.label(egui::RichText::new(line).small().color(theme::TEXT_MUTED));
    }
    for note in &detail.notes {
        ui.label(egui::RichText::new(note).small().color(theme::STATUS_OFFLINE));
    }
}

/// Licence controls (licensed only) and Install / Update; a licensed package installs only with a verified token.
fn store_actions(ui: &mut egui::Ui, state: &mut ExtensionsPanelState, detail: &StoreDetail) {
    let package_id = detail.entry.package_id.as_str();
    let Some(source_id) = state.browse.selected.clone() else { return };
    let idle = !state.busy();
    ui.add_space(8.0);
    if detail.licensed() {
        ui.label(egui::RichText::new(detail.entitlement.label()).small().color(theme::TEXT_SECTION));
        if !detail.entitlement.held() {
            ui.horizontal(|ui| {
                let acquire = detail.can_acquire
                    && ui.add_enabled(idle, egui::Button::new("Get licence")).button_named(ui, format!("Get licence for {package_id}")).clicked();
                if acquire {
                    state.queue(
                        ExtensionOp::OpenAcquire { source_id: source_id.clone(), package_id: package_id.to_owned() },
                        "Opening the licence page",
                    );
                }
                if ui.add_enabled(idle, egui::Button::new("Check licence")).button_named(ui, format!("Check licence for {package_id}")).clicked() {
                    let checking = EntitlementState::Checking;
                    state.browse.apply(BrowseEvent::Entitlement { source_id: source_id.clone(), package_id: package_id.to_owned(), state: checking });
                    state.queue(
                        ExtensionOp::RequestLicenceToken { source_id: source_id.clone(), package_id: package_id.to_owned() },
                        "Checking licence",
                    );
                }
            });
        }
    }
    let installed = state.rows.iter().any(|row| row.package_id == package_id && row.version == detail.entry.version);
    let newer = state.rows.iter().any(|row| row.package_id == package_id && row.version != detail.entry.version);
    let label = if installed {
        "Installed".to_owned()
    } else if newer {
        format!("Update to {}", detail.entry.version)
    } else {
        "Install".to_owned()
    };
    let allowed = !installed && idle && (!detail.licensed() || detail.entitlement.held());
    if ui.add_enabled(allowed, egui::Button::new(label)).button_named(ui, format!("Install {package_id} {}", detail.entry.version)).clicked() {
        state.queue(ExtensionOp::InstallFromRegistry { source_id, entry: detail.entry.clone() }, "Fetching and verifying package");
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
