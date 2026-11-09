//! Extensions panel: Installed list, view routing and the queued `ExtensionOp` seam to the bridge.
//! Design, bridge contract and consent rules: `docs/extensions/extensions-ui.md`.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;

use super::browse::{self, BrowseEvent, BrowseState};
use super::consent::{ConsentDecision, ConsentRequest, ConsentSection, ConsentState};
use super::details::{self, UpdateStage};
use crate::accessibility::ResponseExt;
use crate::theme;

/// Whether the package signer is a configured trust root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustState {
    /// Signed by a trust root.
    Trusted,
    /// Signed by a key that is not a trust root.
    #[default]
    Untrusted,
    /// No signature (development builds only).
    Unsigned,
}

impl TrustState {
    /// Plain-language label for the row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Trusted => "trusted publisher",
            Self::Untrusted => "publisher not trusted",
            Self::Unsigned => "unsigned",
        }
    }
}

/// Where an installed extension stands on this host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RowStatus {
    /// At least one binding runs.
    Enabled,
    /// Installed and consented, nothing runs.
    #[default]
    Disabled,
    /// Cannot run until the reason is resolved (revoked build, policy, role).
    Blocked(String),
    /// Installed but no consent is on record.
    NeedsConsent,
}

impl RowStatus {
    /// Short status word shown in the list.
    pub fn label(&self) -> String {
        match self {
            Self::Enabled => "Enabled".into(),
            Self::Disabled => "Disabled".into(),
            Self::Blocked(reason) => format!("Blocked: {reason}"),
            Self::NeedsConsent => "Needs consent".into(),
        }
    }
}

/// One binding of an extension that can be switched on or off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BindingRow {
    /// Stable binding id the bridge acts on.
    pub binding_id: String,
    /// Human label, e.g. `Petal map in Harbour`.
    pub label: String,
    /// Whether the binding currently runs.
    pub enabled: bool,
}

/// One audit trail line (newest first as supplied by the host).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuditLine {
    /// Display timestamp.
    pub at: String,
    /// Who acted (a name, or a DID when none is known).
    pub actor: String,
    /// What happened, e.g. `Enabled in Harbour`.
    pub action: String,
}

/// The consent in force for the running version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConsentSummary {
    /// Display timestamp of the consent.
    pub consented_at: String,
    /// Who consented.
    pub consented_by: String,
    /// Digest consent is recorded against.
    pub capability_digest: String,
    /// The grants consented to, as the consent dialog showed them.
    pub sections: Vec<ConsentSection>,
}

/// Everything the panel shows about one installation (a snapshot; fe-ui never reaches the host).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstalledRow {
    /// Local installation key.
    pub installation_id: String,
    /// Reverse-DNS package id.
    pub package_id: String,
    /// Display name.
    pub name: String,
    /// Running version.
    pub version: String,
    /// Publisher display name.
    pub publisher: String,
    /// Signer label.
    pub signer: String,
    /// Trust state of the signer.
    pub trust: TrustState,
    /// Status on this host.
    pub status: RowStatus,
    /// Newer version known from a registry, when any.
    pub update_available: Option<String>,
    /// Manifest description.
    pub description: String,
    /// Bindings that can be toggled.
    pub bindings: Vec<BindingRow>,
    /// Counters as `(label, value)` pairs, e.g. `("Ticks", "1204")`.
    pub counters: Vec<(String, String)>,
    /// Audit trail.
    pub audit: Vec<AuditLine>,
    /// Consent in force, if any.
    pub consent: Option<ConsentSummary>,
}

/// An operation fe-ui queues and the binary bridge executes (the Hexon Manager pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionOp {
    /// Ask the bridge for a fresh `ExtensionEvent::Rows` snapshot.
    Refresh,
    /// Verify and stage a package from a local path; the bridge answers with `ConsentNeeded`.
    InstallFromFile { path: String },
    /// Fetch, verify and stage a package from a registry source.
    InstallFromRegistry { source_id: String, entry: fe_hexon::registry_source::RegistryEntry },
    /// List a registry source.
    BrowseList { source_id: String, query: String },
    /// Switch one binding on.
    Enable { installation_id: String, binding_id: String },
    /// Switch one binding off.
    Disable { installation_id: String, binding_id: String },
    /// Stage the newer version, re-verify it and answer with `ConsentNeeded`.
    Update { installation_id: String },
    /// Activate a staged update whose capability digest is already consented.
    ActivateUpdate { installation_id: String, capability_digest: String },
    /// Record consent to exactly this digest.
    Consent { installation_id: String, capability_digest: String, accept_in_verse: Option<String> },
    /// Verse Owner's kill-list override, confirmed by the typed package id.
    KillListOverride { package_id: String, confirm: String },
    /// Drop a staged install or update; a running version keeps running.
    DiscardStaged { installation_id: String },
    /// Remove consent and stop every binding.
    RevokeConsent { installation_id: String },
    /// Remove the installation.
    Uninstall { installation_id: String },
}

/// What the bridge reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionEvent {
    /// Full snapshot of the installed list.
    Rows(Vec<InstalledRow>),
    /// An operation failed; the message is shown verbatim.
    Failed { message: String },
    /// A staged install or update awaits consent (or needs no prompt when the digest is already consented).
    ConsentNeeded { request: ConsentRequest, is_update: bool },
    /// Progress of an update for one installation.
    UpdateStage { installation_id: String, stage: UpdateStage },
    /// A registry listing or failure.
    Browse(BrowseEvent),
}

/// Which view is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PanelView {
    /// The installed list.
    #[default]
    Installed,
    /// Registry Browse.
    Browse,
    /// One installation.
    Details { installation_id: String, tab: DetailsTab },
}

/// Sub-view of one installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailsTab {
    /// Description, bindings, counters, audit trail.
    #[default]
    Overview,
    /// Consent record and capability list.
    Permissions,
}

/// A destructive action awaiting its second click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingConfirm {
    /// Remove the installation.
    Uninstall(String),
    /// Revoke consent.
    Revoke(String),
}

/// Panel state; the only thing fe-ui code mutates.
#[derive(Resource, Debug, Default)]
pub struct ExtensionsPanelState {
    /// Installed list snapshot.
    pub rows: Vec<InstalledRow>,
    /// Current view.
    pub view: PanelView,
    /// Browse view state.
    pub browse: BrowseState,
    /// Text of the install-from-file field.
    pub install_path: String,
    /// What the bridge is working on, when something is in flight.
    pub working: Option<String>,
    /// Last failure, until dismissed.
    pub error: Option<String>,
    pub(super) confirm: Option<PendingConfirm>,
    pub(super) ops: Vec<ExtensionOp>,
    pub(super) updates: HashMap<String, UpdateStage>,
    /// Installation whose consent dialog is open; one review at a time.
    pub(super) reviewing: Option<String>,
    requested_rows: bool,
}

impl ExtensionsPanelState {
    /// Queued operations for the bridge to execute.
    pub fn take_ops(&mut self) -> Vec<ExtensionOp> {
        std::mem::take(&mut self.ops)
    }

    /// True while the bridge works or a consent dialog is open: state-changing buttons are disabled.
    pub fn busy(&self) -> bool {
        self.working.is_some() || self.reviewing.is_some()
    }

    /// Ask for the first snapshot (rows and Browse sources) the first time the panel draws.
    fn ensure_loaded(&mut self) {
        if !self.requested_rows {
            self.requested_rows = true;
            self.queue(ExtensionOp::Refresh, "Loading extensions");
        }
    }

    pub(super) fn queue(&mut self, op: ExtensionOp, working: &str) {
        self.error = None;
        self.working = Some(working.to_owned());
        self.ops.push(op);
    }

    /// Install the package at `install_path`; ignored while the field is empty.
    pub fn request_install_from_file(&mut self) {
        let path = self.install_path.trim().to_owned();
        if path.is_empty() {
            return;
        }
        self.queue(ExtensionOp::InstallFromFile { path }, "Verifying package");
    }

    /// Switch a binding on or off.
    pub fn request_toggle(&mut self, installation_id: &str, binding_id: &str, enable: bool) {
        let (installation_id, binding_id) = (installation_id.to_owned(), binding_id.to_owned());
        let op = if enable {
            ExtensionOp::Enable { installation_id, binding_id }
        } else {
            ExtensionOp::Disable { installation_id, binding_id }
        };
        self.queue(op, if enable { "Enabling" } else { "Disabling" });
    }

    /// First click of a destructive action.
    pub fn ask_confirm(&mut self, confirm: PendingConfirm) {
        self.confirm = Some(confirm);
    }

    /// Cancel a pending confirmation.
    pub fn cancel_confirm(&mut self) {
        self.confirm = None;
    }

    /// Second click: queue the confirmed destructive op.
    pub fn accept_confirm(&mut self) {
        match self.confirm.take() {
            Some(PendingConfirm::Uninstall(installation_id)) => {
                if matches!(&self.view, PanelView::Details { installation_id: open, .. } if *open == installation_id) {
                    self.view = PanelView::Installed;
                }
                self.queue(ExtensionOp::Uninstall { installation_id }, "Uninstalling");
            }
            Some(PendingConfirm::Revoke(installation_id)) => {
                self.queue(ExtensionOp::RevokeConsent { installation_id }, "Revoking consent");
            }
            None => {}
        }
    }

    /// The confirmation currently showing, if any.
    pub fn pending_confirm(&self) -> Option<&PendingConfirm> {
        self.confirm.as_ref()
    }

    /// Apply one bridge event.
    pub fn apply_event(&mut self, event: ExtensionEvent, consent: &mut ConsentState) {
        match event {
            ExtensionEvent::Rows(rows) => {
                self.working = None;
                self.rows = rows;
                let rows = &self.rows;
                self.updates.retain(|id, _| rows.iter().any(|row| row.installation_id == *id));
                if let PanelView::Details { installation_id, .. } = &self.view {
                    if !self.rows.iter().any(|row| row.installation_id == *installation_id) {
                        self.view = PanelView::Installed;
                    }
                }
            }
            ExtensionEvent::Failed { message } => {
                self.working = None;
                // A failed re-verify must not leave the update stuck in flight (Update would stay disabled).
                self.updates.retain(|_, stage| *stage != UpdateStage::Verifying);
                self.error = Some(message);
            }
            ExtensionEvent::ConsentNeeded { mut request, is_update } => {
                self.working = None;
                if consent.pending().is_some_and(|open| open.installation_id != request.installation_id) {
                    // One review at a time: the newer staged build is dropped, never left staged unseen.
                    self.updates.remove(&request.installation_id);
                    self.queue(ExtensionOp::DiscardStaged { installation_id: request.installation_id }, "Discarding");
                    self.error = Some("Finish the open review first".into());
                    return;
                }
                if is_update {
                    details::on_update_offer(self, consent, request);
                } else {
                    // A fresh install always prompts: only a running version's consent may skip the dialog.
                    request.consented_digest = None;
                    request.previous_sections.clear();
                    self.reviewing = Some(request.installation_id.clone());
                    consent.request(request);
                }
            }
            ExtensionEvent::UpdateStage { installation_id, stage } => details::on_update_stage(self, installation_id, stage),
            ExtensionEvent::Browse(event) => {
                self.working = None;
                self.browse.apply(event);
            }
        }
    }

    /// Turn the consent dialog's answer into the matching op.
    pub fn route_consent(&mut self, decision: ConsentDecision) {
        self.reviewing = None;
        match decision {
            ConsentDecision::Approved { installation_id, capability_digest, accept_in_verse } => {
                self.updates.remove(&installation_id);
                self.queue(ExtensionOp::Consent { installation_id, capability_digest, accept_in_verse }, "Enabling");
            }
            ConsentDecision::KillListOverride { package_id, confirm } => {
                self.queue(ExtensionOp::KillListOverride { package_id, confirm }, "Applying override");
            }
            ConsentDecision::Declined { installation_id } => {
                self.updates.remove(&installation_id);
                self.queue(ExtensionOp::DiscardStaged { installation_id }, "Discarding");
            }
        }
    }

    /// Open the details view of one installation.
    pub fn open_details(&mut self, installation_id: &str) {
        self.confirm = None;
        self.view = PanelView::Details { installation_id: installation_id.to_owned(), tab: DetailsTab::Overview };
    }
}

/// Registers the panel state and the consent routing system.
pub struct ExtensionsPanelPlugin;

impl Plugin for ExtensionsPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExtensionsPanelState>()
            .init_resource::<ConsentState>()
            .add_systems(Update, route_consent_decisions);
    }
}

fn route_consent_decisions(mut panel: ResMut<ExtensionsPanelState>, mut consent: ResMut<ConsentState>) {
    while let Some(decision) = consent.take_decision() {
        panel.route_consent(decision);
    }
}

/// Draw the panel body into `ui`.
pub fn draw(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    state.ensure_loaded();
    tab_strip(ui, state);
    ui.add_space(6.0);
    if let Some(message) = state.error.clone() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(message).color(theme::STATUS_OFFLINE));
            if ui.small_button("Dismiss").button_named(ui, "Dismiss extension error").clicked() {
                state.error = None;
            }
        });
    }
    if let Some(what) = &state.working {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(format!("{what}...")).small().color(theme::TEXT_MUTED));
        });
    }
    match state.view.clone() {
        PanelView::Installed => draw_installed(ui, state),
        PanelView::Browse => browse::draw(ui, state),
        PanelView::Details { installation_id, tab } => details::draw(ui, state, &installation_id, tab),
    }
}

fn tab_strip(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    ui.horizontal(|ui| {
        let installed = matches!(state.view, PanelView::Installed | PanelView::Details { .. });
        if ui.selectable_label(installed, "Installed").button_named(ui, "Installed extensions").clicked() {
            state.view = PanelView::Installed;
        }
        if ui.selectable_label(state.view == PanelView::Browse, "Browse").button_named(ui, "Browse extensions").clicked() {
            state.view = PanelView::Browse;
        }
    });
}

fn draw_installed(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.install_path).hint_text("Path to a .hexon file"))
            .named(ui, "Package file path");
        let ready = !state.install_path.trim().is_empty() && !state.busy();
        if ui.add_enabled(ready, egui::Button::new("Install from file")).button_named(ui, "Install from file").clicked() {
            state.request_install_from_file();
        }
    });
    ui.add_space(8.0);
    if state.rows.is_empty() {
        let text = if state.working.is_some() { "Loading extensions" } else { "No extensions installed" };
        ui.label(egui::RichText::new(text).color(theme::TEXT_MUTED));
        return;
    }
    let mut open: Option<String> = None;
    for row in &state.rows {
        let update = state.updates.get(&row.installation_id);
        ui.horizontal(|ui| {
            let title = format!("{} {}", row.name, row.version);
            if ui.link(title).button_named(ui, format!("Open {}", row.package_id)).clicked() {
                open = Some(row.installation_id.clone());
            }
            ui.label(egui::RichText::new(row.status.label()).small().color(status_color(&row.status)));
        });
        let mut meta = format!("{} · {}", row.publisher, row.trust.label());
        if let Some(newer) = &row.update_available {
            meta.push_str(&format!(" · update {newer} available"));
        }
        if let Some(stage) = update {
            meta.push_str(&format!(" · {}", stage.label()));
        }
        ui.label(egui::RichText::new(meta).small().color(theme::TEXT_MUTED));
        ui.separator();
    }
    if let Some(id) = open {
        state.open_details(&id);
    }
}

fn status_color(status: &RowStatus) -> egui::Color32 {
    match status {
        RowStatus::Enabled => theme::TEXT_ROLE_EDITOR,
        RowStatus::Blocked(_) => theme::STATUS_OFFLINE,
        RowStatus::Disabled | RowStatus::NeedsConsent => theme::TEXT_MUTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw one frame; returns every accessible name with whether its control is enabled.
    fn frame(state: &mut ExtensionsPanelState) -> Vec<(String, bool)> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| draw(ui, state));
        });
        output
            .platform_output
            .accesskit_update
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| node.label().map(|label| (label.to_string(), !node.is_disabled())))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn enabled(names: &[(String, bool)], name: &str) -> bool {
        names.iter().any(|(shown, on)| shown == name && *on)
    }

    fn disabled(names: &[(String, bool)], name: &str) -> bool {
        names.iter().any(|(shown, on)| shown == name && !*on)
    }

    fn section(title: &str, lines: &[&str]) -> ConsentSection {
        ConsentSection { title: title.into(), lines: lines.iter().map(|line| line.to_string()).collect() }
    }

    fn request(installation: &str, digest: &str, consented: Option<&str>, egress: &[&str]) -> ConsentRequest {
        ConsentRequest {
            installation_id: installation.into(),
            package_id: "org.example.fleet-sim".into(),
            name: "Fleet Sim".into(),
            version: "1.1.0".into(),
            capability_digest: digest.into(),
            consented_digest: consented.map(str::to_owned),
            previous_sections: vec![section("Egress", &[])],
            sections: vec![section("Egress", egress)],
            ..ConsentRequest::default()
        }
    }

    fn row(version: &str, status: RowStatus, update: Option<&str>) -> InstalledRow {
        InstalledRow {
            installation_id: "inst-1".into(),
            package_id: "org.example.fleet-sim".into(),
            name: "Fleet Sim".into(),
            version: version.into(),
            update_available: update.map(str::to_owned),
            bindings: vec![BindingRow { binding_id: "b1".into(), label: "Fleet in Harbour".into(), enabled: false }],
            consent: (status != RowStatus::NeedsConsent).then(|| ConsentSummary {
                consented_at: "2026-11-08 10:00".into(),
                consented_by: "local owner".into(),
                capability_digest: "d1".into(),
                sections: vec![section("Egress", &[])],
            }),
            status,
            ..InstalledRow::default()
        }
    }

    /// J9 (fake bridge): load, install from file, consent, enable, disable, update with and without a capability
    /// delta, rollback, failed re-verify, one review at a time, revoke then re-consent, uninstall; by accessible name.
    #[test]
    fn extensions_panel_journey() {
        const ENABLE: &str = "Enable Fleet in Harbour for org.example.fleet-sim";
        const UPDATE: &str = "Update org.example.fleet-sim to 1.1.0";
        let mut state = ExtensionsPanelState::default();
        let mut consent = ConsentState::default();

        // First draw asks the bridge for rows and sources; Install stays disabled until a path is typed.
        let names = frame(&mut state);
        assert_eq!(state.take_ops(), vec![ExtensionOp::Refresh]);
        assert!(disabled(&names, "Install from file"));
        state.apply_event(ExtensionEvent::Rows(Vec::new()), &mut consent);
        state.request_install_from_file();
        assert!(state.take_ops().is_empty());
        state.install_path = "C:/pkgs/fleet-sim.hexon".into();
        assert!(enabled(&frame(&mut state), "Install from file"));
        state.request_install_from_file();
        assert_eq!(state.take_ops(), vec![ExtensionOp::InstallFromFile { path: "C:/pkgs/fleet-sim.hexon".into() }]);

        // A fresh install always prompts, even if the bridge reports a matching consent; the panel is busy meanwhile.
        state.apply_event(ExtensionEvent::ConsentNeeded { request: request("inst-1", "d1", Some("d1"), &[]), is_update: false }, &mut consent);
        assert!(consent.pending().is_some_and(|pending| pending.consented_digest.is_none()));
        assert!(disabled(&frame(&mut state), "Install from file"));

        // One review at a time: a second staged build is discarded, never left staged unseen.
        state.apply_event(ExtensionEvent::ConsentNeeded { request: request("inst-2", "d9", None, &[]), is_update: false }, &mut consent);
        assert_eq!(state.take_ops(), vec![ExtensionOp::DiscardStaged { installation_id: "inst-2".into() }]);
        assert_eq!(consent.pending().map(|pending| pending.installation_id.as_str()), Some("inst-1"));

        // Approving consent queues exactly the digest shown.
        state.route_consent(ConsentDecision::Approved { installation_id: "inst-1".into(), capability_digest: "d1".into(), accept_in_verse: None });
        consent = ConsentState::default(); // the dialog closed on its answer
        assert_eq!(
            state.take_ops(),
            vec![ExtensionOp::Consent { installation_id: "inst-1".into(), capability_digest: "d1".into(), accept_in_verse: None }]
        );
        state.apply_event(ExtensionEvent::Rows(vec![row("1.0.0", RowStatus::Disabled, Some("1.1.0"))]), &mut ConsentState::default());
        assert!(enabled(&frame(&mut state), "Open org.example.fleet-sim"));

        // Enable then disable a binding; the row's controls are named and disabled while the bridge works.
        state.open_details("inst-1");
        let names = frame(&mut state);
        assert!(enabled(&names, ENABLE) && enabled(&names, UPDATE) && enabled(&names, "Uninstall org.example.fleet-sim"));
        state.request_toggle("inst-1", "b1", true);
        assert!(disabled(&frame(&mut state), ENABLE));
        state.request_toggle("inst-1", "b1", false);
        assert_eq!(
            state.take_ops(),
            vec![
                ExtensionOp::Enable { installation_id: "inst-1".into(), binding_id: "b1".into() },
                ExtensionOp::Disable { installation_id: "inst-1".into(), binding_id: "b1".into() },
            ]
        );
        state.apply_event(ExtensionEvent::Rows(vec![row("1.0.0", RowStatus::Disabled, Some("1.1.0"))]), &mut consent);

        // Update with a delta: re-prompt; the previous version stays until the new one is active; a failed start rolls back.
        details::begin_update(&mut state, "inst-1");
        assert_eq!(state.take_ops(), vec![ExtensionOp::Update { installation_id: "inst-1".into() }]);
        state.apply_event(ExtensionEvent::ConsentNeeded { request: request("inst-1", "d2", Some("d1"), &["https://api.example"]), is_update: true }, &mut consent);
        assert!(consent.pending().is_some_and(|pending| !pending.delta().is_empty()));
        assert!(disabled(&frame(&mut state), UPDATE));
        state.route_consent(ConsentDecision::Approved { installation_id: "inst-1".into(), capability_digest: "d2".into(), accept_in_verse: None });
        consent = ConsentState::default();
        assert_eq!(
            state.take_ops(),
            vec![ExtensionOp::Consent { installation_id: "inst-1".into(), capability_digest: "d2".into(), accept_in_verse: None }]
        );
        state.apply_event(ExtensionEvent::UpdateStage { installation_id: "inst-1".into(), stage: UpdateStage::Activating { version: "1.1.0".into() } }, &mut consent);
        state.apply_event(ExtensionEvent::UpdateStage { installation_id: "inst-1".into(), stage: UpdateStage::RolledBack { reason: "start failed".into() } }, &mut consent);
        assert_eq!(state.rows[0].version, "1.0.0");
        assert!(state.error.as_deref().is_some_and(|error| error.contains("previous version kept")));
        assert!(enabled(&frame(&mut state), UPDATE), "a rolled-back update can be retried");

        // A failed re-verify does not leave the update stuck in flight.
        details::begin_update(&mut state, "inst-1");
        state.apply_event(ExtensionEvent::Failed { message: "package-invalid (bad-signature)".into() }, &mut consent);
        assert_eq!(state.error.as_deref(), Some("package-invalid (bad-signature)"));
        state.take_ops();
        assert!(enabled(&frame(&mut state), UPDATE));

        // Update without a delta: no prompt, activation queued directly.
        details::begin_update(&mut state, "inst-1");
        state.take_ops();
        state.apply_event(ExtensionEvent::ConsentNeeded { request: request("inst-1", "d1", Some("d1"), &[]), is_update: true }, &mut consent);
        assert!(consent.pending().is_none());
        assert_eq!(state.take_ops(), vec![ExtensionOp::ActivateUpdate { installation_id: "inst-1".into(), capability_digest: "d1".into() }]);
        state.apply_event(ExtensionEvent::Rows(vec![row("1.0.0", RowStatus::Disabled, None)]), &mut consent);

        // Revoke consent (two clicks), then Enable on the Needs-consent row asks the bridge for a fresh review.
        state.view = PanelView::Details { installation_id: "inst-1".into(), tab: DetailsTab::Permissions };
        state.ask_confirm(PendingConfirm::Revoke("inst-1".into()));
        assert!(enabled(&frame(&mut state), "Confirm revoke consent for org.example.fleet-sim"));
        state.accept_confirm();
        assert_eq!(state.take_ops(), vec![ExtensionOp::RevokeConsent { installation_id: "inst-1".into() }]);
        state.apply_event(ExtensionEvent::Rows(vec![row("1.0.0", RowStatus::NeedsConsent, None)]), &mut consent);
        state.open_details("inst-1");
        assert!(enabled(&frame(&mut state), ENABLE));

        // Uninstall needs the second click and returns to the list.
        state.ask_confirm(PendingConfirm::Uninstall("inst-1".into()));
        assert!(enabled(&frame(&mut state), "Confirm uninstall org.example.fleet-sim"));
        assert!(state.take_ops().is_empty());
        state.accept_confirm();
        assert_eq!(state.view, PanelView::Installed);
        assert_eq!(state.take_ops(), vec![ExtensionOp::Uninstall { installation_id: "inst-1".into() }]);
        state.apply_event(ExtensionEvent::Rows(Vec::new()), &mut consent);
        assert!(!frame(&mut state).iter().any(|(name, _)| name == "Open org.example.fleet-sim"));
    }
}
