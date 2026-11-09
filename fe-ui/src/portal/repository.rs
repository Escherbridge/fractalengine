//! Inspector "Repository" section for nodes that reference a portal bundle. See `docs/spec/portal-bundle/ux.md` §2.

use bevy_egui::egui;
use fe_webview::ipc::BundleState;

use super::chrome::{chrome_model, short_did, ChromeTone};
use crate::accessibility::ResponseExt;
use crate::theme;

/// Plain data the call site assembles from the node, the portal summary read and governance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryView {
    pub title: String,
    /// Sealing DID, `None` for an unsigned build.
    pub publisher_did: Option<String>,
    pub trust_root_name: Option<String>,
    pub version: String,
    /// Full hex content hash.
    pub content_hash: String,
    /// Admission state the node would open under right now.
    pub state: BundleState,
    /// Human capability names, one per entry.
    pub capabilities: Vec<String>,
    /// Editor or above (the `portal.bundle_asset` property write).
    pub can_replace: bool,
}

/// What the user asked for; the call site maps it to a `UiAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryIntent {
    Open,
    Admit,
    Revoke,
    Replace,
}

/// Pure: the buttons shown, primary first (Open is always present).
pub fn repository_intents(view: &RepositoryView) -> Vec<RepositoryIntent> {
    let mut intents = vec![RepositoryIntent::Open];
    match view.state {
        BundleState::StaticSandbox { can_admit: true } => intents.push(RepositoryIntent::Admit),
        BundleState::Admitted { .. } => intents.push(RepositoryIntent::Revoke),
        _ => {}
    }
    if view.can_replace {
        intents.push(RepositoryIntent::Replace);
    }
    intents
}

/// Pure: "short DID plus trust-root name", or "unsigned".
pub fn publisher_text(view: &RepositoryView) -> String {
    match (&view.publisher_did, &view.trust_root_name) {
        (Some(did), Some(root)) => format!("{} ({root})", short_did(did)),
        (Some(did), None) => short_did(did),
        (None, _) => "unsigned".into(),
    }
}

/// Pure: version plus the first 8 hex of the content hash.
pub fn version_text(view: &RepositoryView) -> String {
    let short: String = view.content_hash.chars().take(8).collect();
    format!("{} ({short})", view.version)
}

impl RepositoryIntent {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Admit => "Admit…",
            Self::Revoke => "Revoke on this peer",
            Self::Replace => "Replace bundle",
        }
    }

    fn accessible_name(self) -> &'static str {
        match self {
            Self::Open => "Open bundle",
            Self::Admit => "Admit bundle",
            Self::Revoke => "Revoke bundle on this peer",
            Self::Replace => "Replace bundle",
        }
    }
}

fn row(ui: &mut egui::Ui, label: &str, value: &str, name: &str, hover: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let response = ui.add(egui::Label::new(value).truncate()).named(ui, name);
            response.on_hover_text(hover.unwrap_or(value));
        });
    });
}

/// Draw the section; returns the intent clicked this frame.
pub fn show_repository_section(ui: &mut egui::Ui, view: &RepositoryView) -> Option<RepositoryIntent> {
    ui.label(egui::RichText::new("Repository").strong().color(theme::TEXT_HEADING))
        .named(ui, "Repository");
    let badge = chrome_model(&view.state);
    let (badge_text, badge_name) = badge
        .as_ref()
        .map(|model| (model.badge_text.clone(), model.badge_name.clone()))
        .unwrap_or_default();

    row(ui, "Bundle", &view.title, &format!("Bundle title: {}", view.title), None);
    let publisher = publisher_text(view);
    row(ui, "Publisher", &publisher, &format!("Bundle publisher: {publisher}"), view.publisher_did.as_deref());
    let version = version_text(view);
    row(ui, "Version", &version, &format!("Bundle version: {version}"), Some(&view.content_hash));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Status").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let color = match badge.as_ref().map(|model| model.tone) {
                Some(ChromeTone::Error) => theme::STATUS_OFFLINE,
                Some(ChromeTone::Accent) => theme::TREE_PRIMARY_ACCENT,
                _ => theme::TEXT_DIM,
            };
            ui.add(egui::Label::new(egui::RichText::new(&badge_text).color(color)).truncate())
                .named(ui, format!("Bundle status: {badge_name}"))
                .on_hover_text(&badge_text);
        });
    });
    ui.label(egui::RichText::new("Access").color(theme::TEXT_DIM));
    ui.vertical(|ui| {
        if view.capabilities.is_empty() {
            ui.label("None");
        }
        for capability in &view.capabilities {
            ui.label(capability);
        }
    })
    .response
    .named(ui, "Bundle access");

    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        for intent in repository_intents(view) {
            let size = crate::ui_shell::layout_policy::control_size(ui.ctx(), [28.0, 24.0]);
            if ui
                .add(egui::Button::new(intent.label()).min_size(size))
                .named(ui, intent.accessible_name())
                .clicked()
            {
                clicked = Some(intent);
            }
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_follow_state_and_role() {
        let mut view = RepositoryView {
            title: "Manual".into(),
            publisher_did: None,
            trust_root_name: None,
            version: "1.0.0".into(),
            content_hash: "0123456789abcdef".into(),
            state: BundleState::StaticSandbox { can_admit: true },
            capabilities: vec![],
            can_replace: false,
        };
        assert_eq!(repository_intents(&view), [RepositoryIntent::Open, RepositoryIntent::Admit]);
        assert_eq!(publisher_text(&view), "unsigned");
        assert_eq!(version_text(&view), "1.0.0 (01234567)");
        view.state = BundleState::Admitted { publisher: "did:key:z6Mkexample0000".into() };
        view.can_replace = true;
        assert_eq!(
            repository_intents(&view),
            [RepositoryIntent::Open, RepositoryIntent::Revoke, RepositoryIntent::Replace]
        );
        view.state = BundleState::WaitingForBytes;
        view.can_replace = false;
        assert_eq!(repository_intents(&view), [RepositoryIntent::Open]);
    }
}
