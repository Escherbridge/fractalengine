//! Plugin install consent modal: lists what a package asks for before Install enables.
//! Pure state in [`PluginInstallState`]; egui in [`render`]. See `fe-ui/src/AGENTS.md` §plugin-consent.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use crate::theme;

/// What a package asks for, copied from its capability manifest (fe-ui has no fe-plugin dependency).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginInstallRequest {
    /// Stable plugin identifier the host installs under.
    pub plugin_id: String,
    /// Human-readable plugin name.
    pub display_name: String,
    /// Package version string.
    pub version: String,
    /// Publisher label from the verified signature, when the package is signed.
    pub publisher: Option<String>,
    /// Requested verse scope patterns.
    pub verse_scope: Vec<String>,
    /// Requested petal scope patterns.
    pub petal_scope: Vec<String>,
    /// Requested property key patterns.
    pub property_scope: Vec<String>,
    /// Named capability grants (e.g. `storage.read`).
    pub capabilities: Vec<String>,
    /// Requested network level (`none` is the only grantable value in v1).
    pub network_scope: String,
    /// Whether outbound HTTP is requested.
    pub external_http: bool,
}

impl PluginInstallRequest {
    /// True when the manifest asks for network access the v1 host cannot grant.
    pub fn requests_network(&self) -> bool {
        let scope = self.network_scope.trim();
        (!scope.is_empty() && scope != "none") || self.external_http
    }
}

/// The user's answer, drained by the host that owns installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginInstallDecision {
    /// User confirmed after reviewing the capability list.
    Approved(String),
    /// User cancelled or closed the modal.
    Declined(String),
}

/// Modal state: at most one package awaits consent at a time.
#[derive(Resource, Debug, Default)]
pub struct PluginInstallState {
    pending: Option<PluginInstallRequest>,
    manifest_rendered: bool,
    decision: Option<PluginInstallDecision>,
}

impl PluginInstallState {
    /// Open the modal for a package; replaces any earlier unanswered request.
    pub fn request_install(&mut self, request: PluginInstallRequest) {
        self.pending = Some(request);
        self.manifest_rendered = false;
    }

    /// The package currently awaiting consent.
    pub fn pending(&self) -> Option<&PluginInstallRequest> {
        self.pending.as_ref()
    }

    /// Take the user's answer, if one was given since the last call.
    pub fn take_decision(&mut self) -> Option<PluginInstallDecision> {
        self.decision.take()
    }

    /// Install is enabled only after the capability list was drawn and no ungrantable network access is requested.
    pub fn confirm_enabled(&self) -> bool {
        self.manifest_rendered && self.pending.as_ref().is_some_and(|request| !request.requests_network())
    }

    fn resolve(&mut self, approved: bool) {
        if let Some(request) = self.pending.take() {
            self.decision = Some(if approved {
                PluginInstallDecision::Approved(request.plugin_id)
            } else {
                PluginInstallDecision::Declined(request.plugin_id)
            });
        }
        self.manifest_rendered = false;
    }
}

/// Draw the consent modal when a request is pending.
pub fn render(ctx: &egui::Context, state: &mut PluginInstallState) {
    let Some(request) = state.pending.clone() else {
        return;
    };
    let mut still_open = true;
    let mut approve = false;
    let mut cancel = false;
    let mut listed = false;
    let confirm_enabled = state.confirm_enabled();

    crate::ui_shell::layout_policy::bounded_window(ctx, format!("Install {}", request.display_name))
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .min_width(crate::ui_shell::layout_policy::dialog_width(ctx, 420.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            let publisher = request.publisher.as_deref().unwrap_or("Unsigned package");
            ui.label(
                egui::RichText::new(format!("{} {} · {}", request.display_name, request.version, publisher))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(8.0);
            scope_row(ui, "Verses", &request.verse_scope);
            scope_row(ui, "Petals", &request.petal_scope);
            scope_row(ui, "Properties", &request.property_scope);
            scope_row(ui, "Capabilities", &request.capabilities);
            network_row(ui, &request);
            listed = true;
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(confirm_enabled, egui::Button::new("Install"))
                    .button_named(ui, format!("Install {}", request.display_name))
                    .clicked()
                {
                    approve = true;
                }
                if ui.button("Cancel").button_named(ui, "Cancel plugin install").clicked() {
                    cancel = true;
                }
            });
        });

    if approve && confirm_enabled {
        state.resolve(true);
    } else if cancel || !still_open {
        state.resolve(false);
    } else if listed {
        state.manifest_rendered = true;
    }
}

fn scope_row(ui: &mut egui::Ui, label: &str, items: &[String]) {
    ui.label(egui::RichText::new(label).strong().color(theme::TEXT_SECTION));
    if items.is_empty() {
        ui.label(egui::RichText::new("None").small().color(theme::TEXT_MUTED));
    } else {
        for item in items {
            ui.label(egui::RichText::new(item).monospace().small());
        }
    }
    ui.add_space(4.0);
}

fn network_row(ui: &mut egui::Ui, request: &PluginInstallRequest) {
    ui.label(egui::RichText::new("Network").strong().color(theme::TEXT_SECTION));
    if request.requests_network() {
        ui.label(
            egui::RichText::new(format!(
                "Requests network access ({}{}) that this version cannot grant. Install is unavailable.",
                request.network_scope,
                if request.external_http { ", outbound HTTP" } else { "" }
            ))
            .small()
            .color(theme::STATUS_OFFLINE),
        );
    } else {
        ui.label(egui::RichText::new("No network access").small().color(theme::TEXT_MUTED));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(state: &mut PluginInstallState) {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| render(ctx, state));
    }

    fn request(network_scope: &str) -> PluginInstallRequest {
        PluginInstallRequest {
            plugin_id: "iot-bridge".into(),
            display_name: "IoT Bridge".into(),
            version: "1.0.0".into(),
            publisher: Some("Example Publisher".into()),
            capabilities: vec!["storage.read".into()],
            network_scope: network_scope.into(),
            ..Default::default()
        }
    }

    /// Journey: confirm is off until the list has been drawn, then on; a network ask never enables it; closing declines.
    #[test]
    fn install_dialog_renders_capability_list_before_confirm_enabled() {
        let mut state = PluginInstallState::default();
        state.request_install(request("none"));
        assert!(!state.confirm_enabled(), "no frame drawn yet");
        frame(&mut state);
        assert!(state.confirm_enabled(), "list was drawn");

        state.request_install(request("internet"));
        frame(&mut state);
        assert!(!state.confirm_enabled(), "ungrantable network scope keeps Install disabled");

        state.resolve(false);
        assert_eq!(state.take_decision(), Some(PluginInstallDecision::Declined("iot-bridge".into())));
        assert!(state.pending().is_none());
    }
}
