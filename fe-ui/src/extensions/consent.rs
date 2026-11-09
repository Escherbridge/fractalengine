//! Consent v2 modal: renders a manifest v2 grant list, re-prompts on a capability delta, confirms kill-list overrides.
//! Pure state in [`ConsentState`]; egui in [`render`]. See `fe-ui/src/extensions/AGENTS.md` §consent.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use crate::theme;

/// One titled group of grant lines (binding, scene, readings, ingress, egress, bus, ui, provides, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsentSection {
    /// Section heading, e.g. `Ingress`.
    pub title: String,
    /// One rendered line per grant, stable across versions so the delta can diff them.
    pub lines: Vec<String>,
}

/// How the dialog was reached.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConsentOrigin {
    /// The operator chose Install or Update directly.
    #[default]
    Direct,
    /// A binding offered the install ("Allow in <verse>" acceptance line, bindings.md section 5.5).
    InstallOffer {
        /// Verse the extension is accepted in once consent is given (the id the host records; names repeat).
        verse_id: String,
        /// That verse's display name.
        verse_name: String,
        /// Scope level the binding comes from, e.g. `petal Harbour`.
        binding_scope: String,
    },
}

/// The verse Owner's kill-list override offer for a revoked build (governance.md section Kill-list override).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillListOverrideOffer {
    /// Reason carried by the accepted revocation entry.
    pub reason: String,
    /// Verse the override would apply to (the host keys the override `(verse_id, content_hash)`).
    pub verse_id: String,
    /// That verse's display name.
    pub verse_name: String,
}

/// What the host knows about a package awaiting consent (fe-ui has no fe-plugin dependency, so this is copied data).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsentRequest {
    /// Local installation the consent record is keyed on.
    pub installation_id: String,
    /// Reverse-DNS package id.
    pub package_id: String,
    /// Display name from the manifest.
    pub name: String,
    /// Package version string.
    pub version: String,
    /// Manifest kind (`simulation`, `device-adapter`, `bi-connector`, `general`).
    pub kind: String,
    /// Hash of the sealed manifest, echoed in the audit entry.
    pub content_hash: String,
    /// Digest consent is recorded against; REST consent must echo exactly this value.
    pub capability_digest: String,
    /// Digest of the consent in force for the running version, when this is an update.
    pub consented_digest: Option<String>,
    /// Grants of the version already consented to; the delta is computed against these.
    pub previous_sections: Vec<ConsentSection>,
    /// Every grant of the version awaiting consent.
    pub sections: Vec<ConsentSection>,
    /// Signer label and trust state, e.g. `Example Publisher (trusted root)`; `None` renders as unsigned.
    pub signer: Option<String>,
    /// Policy verdict line, e.g. `Admitted by verse and local policy`.
    pub policy_verdict: String,
    /// Licence state line, e.g. `Free` or `Paid: token valid`.
    pub licence: String,
    /// A refusal that no consent can fix (untrusted signer, policy denied, role denied); disables Enable.
    pub blocked: Option<String>,
    /// Set when the build is kill-listed and the local DID may override as verse Owner.
    pub kill_list: Option<KillListOverrideOffer>,
    /// How the dialog was opened.
    pub origin: ConsentOrigin,
    /// Web bundle only: its `portal` section, rendered through [`portal_sections`] (A-10); `capability_digest`
    /// is then the portal digest (README section 7.2), which is what consent is recorded against.
    pub portal: Option<serde_json::Value>,
}

/// Render a web bundle's `portal` manifest section as consent sections (portal A-10): one section per capability
/// key, `schema`/`entry`/`title` skipped (they are not capabilities, README section 7.1), a `false` or empty value
/// shown as "None" so nothing the bundle lacks is hidden. Keys sort so the delta against an update is stable.
pub fn portal_sections(portal: &serde_json::Value) -> Vec<ConsentSection> {
    fn lines(value: &serde_json::Value) -> Vec<String> {
        match value {
            serde_json::Value::Array(items) => items.iter().map(lines_one).collect(),
            serde_json::Value::Object(map) => map
                .iter()
                .flat_map(|(key, inner)| match inner {
                    serde_json::Value::Array(items) => items.iter().map(|item| format!("{key}: {}", lines_one(item))).collect(),
                    serde_json::Value::Bool(false) => Vec::new(),
                    other => vec![format!("{key}: {}", lines_one(other))],
                })
                .collect(),
            serde_json::Value::Bool(false) | serde_json::Value::Null => Vec::new(),
            other => vec![lines_one(other)],
        }
    }
    fn lines_one(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
    let Some(map) = portal.as_object() else { return Vec::new() };
    let mut keys: Vec<&String> = map.keys().filter(|key| !matches!(key.as_str(), "schema" | "entry" | "title")).collect();
    keys.sort();
    keys.into_iter()
        .map(|key| ConsentSection { title: format!("Portal {key}"), lines: lines(&map[key]) })
        .collect()
}

/// Grant lines that appear or disappear between two versions of one package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityDelta {
    /// `Section: line` entries new in this version.
    pub added: Vec<String>,
    /// `Section: line` entries the old version had and this one drops.
    pub removed: Vec<String>,
}

impl CapabilityDelta {
    /// True when nothing differs.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

impl ConsentRequest {
    /// True when consent already covers this exact digest, so no prompt is owed (an update with the same digest).
    pub fn already_consented(&self) -> bool {
        self.consented_digest.as_deref() == Some(self.capability_digest.as_str())
    }

    /// True when an earlier, different digest was consented to: the dialog is a re-prompt.
    pub fn is_reprompt(&self) -> bool {
        self.consented_digest.as_deref().is_some_and(|digest| digest != self.capability_digest)
    }

    /// Set difference of the grant lines against the previously consented version.
    pub fn delta(&self) -> CapabilityDelta {
        fn flatten(sections: &[ConsentSection]) -> Vec<String> {
            sections
                .iter()
                .flat_map(|section| section.lines.iter().map(|line| format!("{}: {}", section.title, line)))
                .collect()
        }
        let before = flatten(&self.previous_sections);
        let after = flatten(&self.sections);
        CapabilityDelta {
            added: after.iter().filter(|line| !before.contains(line)).cloned().collect(),
            removed: before.iter().filter(|line| !after.contains(line)).cloned().collect(),
        }
    }
}

/// The user's answer, drained by the host that owns installation and the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentDecision {
    /// Consent given to exactly this digest; `accept_in_verse` is the offer's verse id for an install offer.
    Approved {
        installation_id: String,
        capability_digest: String,
        accept_in_verse: Option<String>,
    },
    /// The verse Owner confirmed the kill-list override by typing the package id; carries what the host's
    /// `KillListOverrideRequest` needs (`{verse_id, package_id, content_hash, confirm}`).
    KillListOverride {
        installation_id: String,
        verse_id: String,
        package_id: String,
        content_hash: String,
        confirm: String,
    },
    /// Cancelled or closed; the running version, if any, keeps running.
    Declined { installation_id: String },
}

/// Modal state: at most one package awaits consent at a time.
#[derive(Resource, Debug, Default)]
pub struct ConsentState {
    pending: Option<ConsentRequest>,
    list_rendered: bool,
    override_text: String,
    decision: Option<ConsentDecision>,
}

impl ConsentState {
    /// Open the modal; an update whose digest is already consented to opens nothing and returns false,
    /// unless the build is kill-listed: the Owner's override path must stay reachable after a revocation.
    pub fn request(&mut self, request: ConsentRequest) -> bool {
        if request.already_consented() && request.kill_list.is_none() {
            return false;
        }
        self.pending = Some(request);
        self.list_rendered = false;
        self.override_text.clear();
        true
    }

    /// The package currently awaiting consent.
    pub fn pending(&self) -> Option<&ConsentRequest> {
        self.pending.as_ref()
    }

    /// Take the user's answer, if one was given since the last call.
    pub fn take_decision(&mut self) -> Option<ConsentDecision> {
        self.decision.take()
    }

    /// Enable stays off until the full grant list was drawn once, and while a refusal or a kill-list entry stands.
    pub fn confirm_enabled(&self) -> bool {
        self.list_rendered
            && self
                .pending
                .as_ref()
                .is_some_and(|request| request.blocked.is_none() && request.kill_list.is_none())
    }

    /// The override button needs the exact package id typed (mirrors the host's `confirmation-mismatch`).
    pub fn override_enabled(&self) -> bool {
        self.pending.as_ref().is_some_and(|request| {
            request.kill_list.is_some() && request.blocked.is_none() && self.override_text == request.package_id
        })
    }

    fn approve(&mut self) {
        if let Some(request) = self.pending.take() {
            let accept_in_verse = match &request.origin {
                ConsentOrigin::InstallOffer { verse_id, .. } => Some(verse_id.clone()),
                ConsentOrigin::Direct => None,
            };
            self.decision = Some(ConsentDecision::Approved {
                installation_id: request.installation_id,
                capability_digest: request.capability_digest,
                accept_in_verse,
            });
        }
        self.list_rendered = false;
    }

    fn override_kill_list(&mut self) {
        if let Some(request) = self.pending.take() {
            let verse_id = request.kill_list.map(|offer| offer.verse_id).unwrap_or_default();
            self.decision = Some(ConsentDecision::KillListOverride {
                installation_id: request.installation_id,
                verse_id,
                package_id: request.package_id,
                content_hash: request.content_hash,
                confirm: std::mem::take(&mut self.override_text),
            });
        }
        self.list_rendered = false;
    }

    fn decline(&mut self) {
        if let Some(request) = self.pending.take() {
            self.decision = Some(ConsentDecision::Declined { installation_id: request.installation_id });
        }
        self.list_rendered = false;
        self.override_text.clear();
    }
}

/// Draw the consent modal when a request is pending.
pub fn render(ctx: &egui::Context, state: &mut ConsentState) {
    let Some(request) = state.pending.clone() else {
        return;
    };
    let mut still_open = true;
    let mut approve = false;
    let mut cancel = false;
    let mut override_clicked = false;
    let mut listed = false;
    let confirm_enabled = state.confirm_enabled();
    let override_enabled = state.override_enabled();
    let mut override_text = std::mem::take(&mut state.override_text);

    crate::ui_shell::layout_policy::bounded_window(ctx, title(&request))
        // Above the Extensions window: a click there must never bury an open review (R-UI-28).
        .order(egui::Order::Foreground)
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .min_width(crate::ui_shell::layout_policy::dialog_width(ctx, 460.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                header(ui, &request);
                if let Some(reason) = &request.blocked {
                    problem(ui, reason);
                }
                if let Some(offer) = &request.kill_list {
                    kill_list_block(ui, &request, offer, &mut override_text);
                }
                if request.is_reprompt() {
                    delta_block(ui, &request.delta());
                }
                for section in &request.sections {
                    section_block(ui, section);
                }
                if let Some(portal) = &request.portal {
                    for section in &portal_sections(portal) {
                        section_block(ui, section);
                    }
                    verdict_row(ui, "Capability digest", &request.capability_digest);
                }
                verdict_row(ui, "Signer", request.signer.as_deref().unwrap_or("Unsigned package"));
                verdict_row(ui, "Policy", &request.policy_verdict);
                verdict_row(ui, "Licence", &request.licence);
                if let ConsentOrigin::InstallOffer { verse_name, .. } = &request.origin {
                    verdict_row(ui, "Acceptance", &format!("Allow in {verse_name}"));
                }
                listed = true;
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let label = if request.is_reprompt() { "Update" } else { "Enable" };
                if ui
                    .add_enabled(confirm_enabled, egui::Button::new(label))
                    .button_named(ui, format!("{label} {}", request.package_id))
                    .clicked()
                {
                    approve = true;
                }
                if request.kill_list.is_some()
                    && ui
                        .add_enabled(override_enabled, egui::Button::new("Override kill list"))
                        .button_named(ui, format!("Override kill list for {}", request.package_id))
                        .clicked()
                {
                    override_clicked = true;
                }
                if ui.button("Cancel").button_named(ui, "Cancel extension consent").clicked() {
                    cancel = true;
                }
            });
        });

    state.override_text = override_text;
    if approve && confirm_enabled {
        state.approve();
    } else if override_clicked && override_enabled {
        state.override_kill_list();
    } else if cancel || !still_open {
        state.decline();
    } else if listed {
        state.list_rendered = true;
    }
}

fn title(request: &ConsentRequest) -> String {
    if request.is_reprompt() {
        format!("Review changes to {}", request.name)
    } else {
        format!("Enable {}", request.name)
    }
}

fn header(ui: &mut egui::Ui, request: &ConsentRequest) {
    ui.label(
        egui::RichText::new(format!("{} {} · {} · {}", request.name, request.version, request.kind, request.package_id))
            .small()
            .color(theme::TEXT_MUTED),
    );
    if let ConsentOrigin::InstallOffer { binding_scope, .. } = &request.origin {
        ui.label(egui::RichText::new(format!("Offered by the binding at {binding_scope}")).small().color(theme::TEXT_MUTED));
    }
    ui.add_space(8.0);
}

fn problem(ui: &mut egui::Ui, reason: &str) {
    ui.label(egui::RichText::new(reason).small().color(theme::STATUS_OFFLINE));
    ui.add_space(6.0);
}

fn kill_list_block(ui: &mut egui::Ui, request: &ConsentRequest, offer: &KillListOverrideOffer, text: &mut String) {
    ui.label(egui::RichText::new("Revoked build").strong().color(theme::STATUS_OFFLINE));
    ui.label(egui::RichText::new(&offer.reason).small());
    ui.label(
        egui::RichText::new(format!(
            "As owner of {} you may run it on this host only. Type {} to confirm.",
            offer.verse_name, request.package_id
        ))
        .small()
        .color(theme::TEXT_MUTED),
    );
    ui.add(egui::TextEdit::singleline(text).hint_text(request.package_id.as_str()))
        .named(ui, "Confirm package id to override kill list");
    ui.add_space(8.0);
}

fn delta_block(ui: &mut egui::Ui, delta: &CapabilityDelta) {
    ui.label(egui::RichText::new("Changed since you last allowed it").strong().color(theme::TEXT_SECTION));
    if delta.is_empty() {
        ui.label(egui::RichText::new("Presentation only; grants are unchanged").small().color(theme::TEXT_MUTED));
    }
    for line in &delta.added {
        ui.label(egui::RichText::new(format!("+ {line}")).monospace().small().color(theme::TEXT_ROLE_EDITOR));
    }
    for line in &delta.removed {
        ui.label(egui::RichText::new(format!("- {line}")).monospace().small().color(theme::STATUS_OFFLINE));
    }
    ui.label(egui::RichText::new("The running version stays on until you accept.").small().color(theme::TEXT_MUTED));
    ui.add_space(8.0);
}

fn section_block(ui: &mut egui::Ui, section: &ConsentSection) {
    ui.label(egui::RichText::new(&section.title).strong().color(theme::TEXT_SECTION));
    if section.lines.is_empty() {
        ui.label(egui::RichText::new("None").small().color(theme::TEXT_MUTED));
    }
    for line in &section.lines {
        ui.label(egui::RichText::new(line).monospace().small());
    }
    ui.add_space(4.0);
}

fn verdict_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).strong().color(theme::TEXT_SECTION));
    ui.label(egui::RichText::new(value).small().color(theme::TEXT_MUTED));
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(state: &mut ConsentState) {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| render(ctx, state));
    }

    fn section(title: &str, lines: &[&str]) -> ConsentSection {
        ConsentSection { title: title.into(), lines: lines.iter().map(|line| line.to_string()).collect() }
    }

    fn request(digest: &str, egress: &[&str]) -> ConsentRequest {
        ConsentRequest {
            installation_id: "inst-1".into(),
            package_id: "org.example.fleet-sim".into(),
            name: "Fleet Sim".into(),
            version: "1.0.0".into(),
            kind: "simulation".into(),
            capability_digest: digest.into(),
            sections: vec![section("Scene", &["read", "write: set_property"]), section("Egress", egress)],
            policy_verdict: "Admitted".into(),
            licence: "Free".into(),
            ..Default::default()
        }
    }

    /// Journey: Enable waits for the drawn list; an update with a new grant re-prompts with the delta and a same-digest update does not.
    #[test]
    fn consent_v2_enables_after_list_and_reprompts_on_capability_delta() {
        let mut state = ConsentState::default();
        assert!(state.request(request("d1", &[])));
        assert!(!state.confirm_enabled(), "no frame drawn yet");
        frame(&mut state);
        assert!(state.confirm_enabled(), "list was drawn");
        state.approve();
        assert_eq!(
            state.take_decision(),
            Some(ConsentDecision::Approved {
                installation_id: "inst-1".into(),
                capability_digest: "d1".into(),
                accept_in_verse: None
            })
        );

        let mut same = request("d1", &[]);
        same.consented_digest = Some("d1".into());
        assert!(!state.request(same), "same digest: no prompt");

        let mut update = request("d2", &["https://bi.example.org/"]);
        update.consented_digest = Some("d1".into());
        update.previous_sections = request("d1", &[]).sections;
        assert!(update.is_reprompt());
        assert_eq!(update.delta().added, ["Egress: https://bi.example.org/"]);
        assert!(update.delta().removed.is_empty());
        assert!(state.request(update));
        assert!(!state.confirm_enabled(), "re-prompt must be re-read");

        state.decline();
        assert_eq!(state.take_decision(), Some(ConsentDecision::Declined { installation_id: "inst-1".into() }));

        // Install offer for a web bundle: the portal section is listed and approval carries the verse id.
        let mut offered = request("portal-digest", &[]);
        offered.portal = Some(serde_json::json!({
            "schema": "fe-portal-bundle/1",
            "entry": "index.html",
            "csp": { "connect": ["https://feeds.example.org"], "img": [] },
            "storage": false
        }));
        offered.origin = ConsentOrigin::InstallOffer {
            verse_id: "verse-harbour".into(),
            verse_name: "Harbour".into(),
            binding_scope: "petal Quay".into(),
        };
        let portal = portal_sections(offered.portal.as_ref().unwrap());
        assert_eq!(portal.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Portal csp", "Portal storage"]);
        assert_eq!(portal[0].lines, ["connect: https://feeds.example.org"]);
        assert!(portal[1].lines.is_empty(), "a false capability renders as None");
        let unresolved = super::super::install_offer::UnresolvedBinding {
            verse_id: "verse-harbour".into(),
            verse_name: "Harbour".into(),
            binding_scope: "petal Quay".into(),
            hook: "portal.open".into(),
            package_id: "org.example.fleet-sim".into(),
            version_req: "^1".into(),
            reason: super::super::install_offer::UnresolvedReason::NotInstalled,
            policy_admits: true,
            registry_has_build: false,
            local_is_owner: false,
        };
        assert_eq!(unresolved.action(), None, "no registry build: state shows without a dead control");
        let unresolved = super::super::install_offer::UnresolvedBinding { registry_has_build: true, ..unresolved };
        assert_eq!(unresolved.action(), Some(super::super::install_offer::OfferAction::Install));
        let mut dismissals = super::super::install_offer::OfferDismissals::default();
        dismissals.dismiss(&unresolved);
        assert_eq!(dismissals.visible_action(&unresolved), None, "dismissed for this binding version");
        assert!(state.request(unresolved.consent_request(offered, None)));
        frame(&mut state);
        state.approve();
        assert_eq!(
            state.take_decision(),
            Some(ConsentDecision::Approved {
                installation_id: "inst-1".into(),
                capability_digest: "portal-digest".into(),
                accept_in_verse: Some("verse-harbour".into())
            })
        );
    }

    /// Journey: a kill-listed build never enables; only the exact typed package id enables the Owner override.
    #[test]
    fn kill_listed_build_needs_typed_confirmation() {
        let mut state = ConsentState::default();
        let mut revoked = request("d1", &[]);
        revoked.content_hash = "ab".repeat(32);
        revoked.kill_list = Some(KillListOverrideOffer {
            reason: "key leaked".into(),
            verse_id: "verse-harbour".into(),
            verse_name: "Harbour".into(),
        });
        // Revoked after consent: the same digest still opens, or the override would be unreachable (G2).
        revoked.consented_digest = Some("d1".into());
        assert!(state.request(revoked));
        frame(&mut state);
        assert!(!state.confirm_enabled(), "consent cannot clear a kill-list entry");
        assert!(!state.override_enabled());
        state.override_text = "org.example.fleet".into();
        assert!(!state.override_enabled(), "near miss");
        state.override_text = "org.example.fleet-sim".into();
        assert!(state.override_enabled());
        state.override_kill_list();
        assert_eq!(
            state.take_decision(),
            Some(ConsentDecision::KillListOverride {
                installation_id: "inst-1".into(),
                verse_id: "verse-harbour".into(),
                package_id: "org.example.fleet-sim".into(),
                content_hash: "ab".repeat(32),
                confirm: "org.example.fleet-sim".into()
            })
        );
    }
}
