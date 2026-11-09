//! Browse view: list a registry source and queue an install (verification and consent happen on the bridge).

use bevy_egui::egui;
use fe_hexon::registry_source::{Listing, Refusal, RegistryEntry};

use super::panel::{ExtensionOp, ExtensionsPanelState};
use crate::accessibility::ResponseExt;
use crate::theme;

/// One configured source the operator can browse.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowseSource {
    /// Stable id the bridge resolves.
    pub id: String,
    /// Label shown in the source picker.
    pub label: String,
}

/// Where a listing stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowseStatus {
    /// Nothing requested yet.
    #[default]
    Idle,
    /// A listing is in flight.
    Loading,
    /// The source refused or was unreachable; the reason is shown verbatim.
    Failed(String),
    /// A listing arrived.
    Ready,
}

/// What the bridge reports for Browse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseEvent {
    /// The configured sources.
    Sources(Vec<BrowseSource>),
    /// A verified listing.
    Listing { source_id: String, listing: Listing },
    /// A refused or failed listing (tampered index, protocol mismatch, unreachable).
    Failed { source_id: String, reason: String },
}

/// Browse view state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowseState {
    /// Sources offered in the picker.
    pub sources: Vec<BrowseSource>,
    /// Selected source id.
    pub selected: Option<String>,
    /// Search text.
    pub query: String,
    /// Listing status.
    pub status: BrowseStatus,
    /// Entries of the last listing.
    pub entries: Vec<RegistryEntry>,
    /// Items the source could not vouch for.
    pub refused: Vec<Refusal>,
    /// True when the listing came from the last verified cached index.
    pub from_cache: bool,
    /// True when the index has expired (still installable).
    pub stale: bool,
}

impl BrowseState {
    /// Apply a bridge event; a result for a source other than the selected one is dropped.
    pub fn apply(&mut self, event: BrowseEvent) {
        match event {
            BrowseEvent::Sources(sources) => {
                if self.selected.as_ref().is_none_or(|id| !sources.iter().any(|source| source.id == *id)) {
                    self.selected = sources.first().map(|source| source.id.clone());
                }
                self.sources = sources;
            }
            BrowseEvent::Listing { source_id, listing } => {
                if self.selected.as_deref() == Some(source_id.as_str()) {
                    self.entries = listing.entries;
                    self.refused = listing.refused;
                    self.from_cache = listing.from_cache;
                    self.stale = listing.stale;
                    self.status = BrowseStatus::Ready;
                }
            }
            BrowseEvent::Failed { source_id, reason } => {
                if self.selected.as_deref() == Some(source_id.as_str()) {
                    self.entries.clear();
                    self.refused.clear();
                    self.status = BrowseStatus::Failed(reason);
                }
            }
        }
    }
}

/// Draw the Browse view.
pub fn draw(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    if state.browse.sources.is_empty() {
        ui.label(egui::RichText::new("No extension sources are configured").color(theme::TEXT_MUTED));
        return;
    }
    let mut refresh = false;
    ui.horizontal(|ui| {
        let selected_label = state
            .browse
            .sources
            .iter()
            .find(|source| Some(&source.id) == state.browse.selected.as_ref())
            .map(|source| source.label.clone())
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("extension-browse-source")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                for source in state.browse.sources.clone() {
                    let chosen = state.browse.selected.as_ref() == Some(&source.id);
                    if ui.selectable_label(chosen, &source.label).clicked() && !chosen {
                        state.browse.selected = Some(source.id.clone());
                        state.browse.status = BrowseStatus::Idle;
                        state.browse.entries.clear();
                        refresh = true;
                    }
                }
            })
            .response
            .named(ui, "Extension source");
        let submitted = ui
            .add(egui::TextEdit::singleline(&mut state.browse.query).hint_text("Search"))
            .named(ui, "Search extensions")
            .lost_focus()
            && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if submitted || ui.button("Refresh").button_named(ui, "Refresh extension list").clicked() {
            refresh = true;
        }
    });
    // A selected source lists itself once; Refresh and Enter re-list it.
    let first_listing = state.browse.status == BrowseStatus::Idle && state.working.is_none();
    if refresh || first_listing {
        if let Some(source_id) = state.browse.selected.clone() {
            state.browse.status = BrowseStatus::Loading;
            let query = state.browse.query.clone();
            state.queue(ExtensionOp::BrowseList { source_id, query }, "Listing");
        }
    }
    ui.add_space(8.0);
    match state.browse.status.clone() {
        BrowseStatus::Idle | BrowseStatus::Loading => {}
        BrowseStatus::Failed(reason) => {
            ui.label(egui::RichText::new(reason).color(theme::STATUS_OFFLINE));
        }
        BrowseStatus::Ready => draw_entries(ui, state),
    }
}

fn draw_entries(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    if state.browse.from_cache {
        ui.label(egui::RichText::new("Showing the last verified list").small().color(theme::TEXT_MUTED));
    }
    if state.browse.stale {
        ui.label(egui::RichText::new("This list has expired and may be out of date").small().color(theme::TEXT_MUTED));
    }
    if state.browse.entries.is_empty() {
        ui.label(egui::RichText::new("No extensions match").color(theme::TEXT_MUTED));
    }
    let Some(source_id) = state.browse.selected.clone() else { return };
    let busy = state.busy();
    let mut install: Option<RegistryEntry> = None;
    for entry in &state.browse.entries {
        let installed = state.rows.iter().any(|row| row.package_id == entry.package_id && row.version == entry.version);
        let newer = state.rows.iter().any(|row| row.package_id == entry.package_id && row.version != entry.version);
        ui.horizontal(|ui| {
            let title = if entry.name.is_empty() { entry.package_id.as_str() } else { entry.name.as_str() };
            ui.label(egui::RichText::new(format!("{title} {}", entry.version)).strong());
            let label = if installed { "Installed" } else if newer { "Install this version" } else { "Install" };
            if ui
                .add_enabled(!installed && !busy, egui::Button::new(label))
                .button_named(ui, format!("Install {} {}", entry.package_id, entry.version))
                .clicked()
            {
                install = Some(entry.clone());
            }
        });
        // A static index states sizes in the detail document, so a listed size of 0 means "not stated".
        let meta = if entry.size > 0 { format!("{} · {}", entry.package_id, human_size(entry.size)) } else { entry.package_id.clone() };
        ui.label(egui::RichText::new(meta).small().color(theme::TEXT_MUTED));
        ui.separator();
    }
    for refusal in &state.browse.refused {
        ui.label(egui::RichText::new(format!("{}: {}", refusal.what, refusal.reason)).small().color(theme::STATUS_OFFLINE));
    }
    if let Some(entry) = install {
        state.queue(ExtensionOp::InstallFromRegistry { source_id, entry }, "Fetching and verifying package");
    }
}

fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KIB * KIB {
        format!("{:.1} MiB", value / (KIB * KIB))
    } else if value >= KIB {
        format!("{:.0} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}
