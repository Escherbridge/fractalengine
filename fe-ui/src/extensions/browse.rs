//! Browse view: search a registry source, list results and open a package's details (verification, consent and
//! licence checks happen on the bridge). Contract and names: `docs/extensions/extensions-ui.md`.

use bevy_egui::egui;
use fe_hexon::registry_source::{installable_access, Listing, Refusal, RegistryEntry, SearchHit, SearchQuery, SearchResults, SearchSort};

use super::details::{EntitlementState, StoreDetail};
use super::panel::{ExtensionOp, ExtensionsPanelState, PanelView};
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
    /// A verified listing from a source without search.
    Listing { source_id: String, listing: Listing },
    /// Search results (server search, or the local fallback over the verified index).
    Results { source_id: String, results: SearchResults },
    /// One package's verified detail document.
    Detail { source_id: String, detail: Box<StoreDetail> },
    /// The licence state of one package after an acquire or token request.
    Entitlement { source_id: String, package_id: String, state: EntitlementState },
    /// A refused or failed listing (tampered index, protocol mismatch, unreachable).
    Failed { source_id: String, reason: String },
}

/// One result row: what Browse needs from a search hit or a plain listing entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseRow {
    /// The build an install would fetch.
    pub entry: RegistryEntry,
    /// One-line summary.
    pub summary: String,
    /// Publisher display name.
    pub publisher: String,
    /// `extension` or `web-bundle`; empty for a plain listing.
    pub kind: String,
    /// `free`, `licensed`, or empty when the source does not say.
    pub access: String,
    /// True when the row came from a verified index (a plain listing is verified by the source).
    pub verified: bool,
    /// True when a kill list revokes the build: shown unavailable.
    pub revoked: bool,
}

impl BrowseRow {
    /// Row for a search hit.
    pub fn from_hit(hit: &SearchHit) -> Self {
        Self {
            entry: hit.entry.to_registry_entry(),
            summary: hit.entry.summary.clone(),
            publisher: hit.entry.publisher.name.clone(),
            kind: hit.entry.kind.clone(),
            access: hit.entry.access.clone(),
            verified: hit.verified,
            revoked: hit.revoked,
        }
    }

    /// Row for a plain listing entry.
    pub fn from_entry(entry: &RegistryEntry) -> Self {
        Self {
            entry: entry.clone(),
            summary: String::new(),
            publisher: String::new(),
            kind: String::new(),
            access: String::new(),
            verified: true,
            revoked: false,
        }
    }

    /// True for a licensed package: installed from its details once a licence is held.
    pub fn licensed(&self) -> bool {
        self.access == fe_hexon::registry_source::ACCESS_LICENSED
    }

    /// Actions show only for a verified, unrevoked, installable row.
    pub fn actionable(&self) -> bool {
        self.verified && !self.revoked && (self.access.is_empty() || installable_access(&self.access))
    }
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
    /// Result order.
    pub sort: SearchSort,
    /// Listing status.
    pub status: BrowseStatus,
    /// Rows of the last search or listing.
    pub rows: Vec<BrowseRow>,
    /// Items the source could not vouch for, or that a kill list revokes.
    pub refused: Vec<Refusal>,
    /// Cursor of the next hosted page, when the endpoint paginates.
    pub next_cursor: Option<String>,
    /// True while a next page is requested: its rows are appended instead of replacing.
    pub appending: bool,
    /// True when the listing came from the last verified cached index.
    pub from_cache: bool,
    /// True when the index has expired (still installable).
    pub stale: bool,
    /// Detail of the package the Details view shows.
    pub detail: Option<StoreDetail>,
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
                if self.is_selected(&source_id) {
                    self.rows = listing.entries.iter().map(BrowseRow::from_entry).collect();
                    self.finish(listing.refused, None, listing.from_cache, listing.stale);
                }
            }
            BrowseEvent::Results { source_id, results } => {
                if self.is_selected(&source_id) {
                    let rows = results.hits.iter().map(BrowseRow::from_hit);
                    if self.appending {
                        self.rows.extend(rows);
                    } else {
                        self.rows = rows.collect();
                    }
                    self.finish(results.refused, results.next_cursor, results.from_cache, results.stale);
                }
            }
            BrowseEvent::Detail { source_id, detail } => {
                if self.is_selected(&source_id) {
                    self.detail = Some(*detail);
                }
            }
            BrowseEvent::Entitlement { source_id, package_id, state } => {
                if self.is_selected(&source_id) {
                    if let Some(detail) = self.detail.as_mut().filter(|detail| detail.entry.package_id == package_id) {
                        detail.entitlement = state;
                    }
                }
            }
            BrowseEvent::Failed { source_id, reason } => {
                if self.is_selected(&source_id) {
                    self.rows.clear();
                    self.refused.clear();
                    self.next_cursor = None;
                    self.appending = false;
                    self.status = BrowseStatus::Failed(reason);
                }
            }
        }
    }

    fn is_selected(&self, source_id: &str) -> bool {
        self.selected.as_deref() == Some(source_id)
    }

    fn finish(&mut self, refused: Vec<Refusal>, next_cursor: Option<String>, from_cache: bool, stale: bool) {
        self.refused = refused;
        self.next_cursor = next_cursor;
        self.appending = false;
        self.from_cache = from_cache;
        self.stale = stale;
        self.status = BrowseStatus::Ready;
    }

    /// The search the controls describe, optionally continuing from `cursor`.
    pub fn search_query(&self, cursor: Option<String>) -> SearchQuery {
        let text = self.query.trim();
        SearchQuery { q: (!text.is_empty()).then(|| text.to_owned()), sort: self.sort, cursor, ..SearchQuery::default() }
    }
}

impl ExtensionsPanelState {
    /// Search the selected source again from the first page.
    pub fn request_search(&mut self) {
        let Some(source_id) = self.browse.selected.clone() else { return };
        self.browse.status = BrowseStatus::Loading;
        self.browse.appending = false;
        let query = self.browse.search_query(None);
        self.queue(ExtensionOp::Search { source_id, query }, "Searching");
    }

    /// Fetch the next hosted page and append it.
    pub fn request_more_results(&mut self) {
        let (Some(source_id), Some(cursor)) = (self.browse.selected.clone(), self.browse.next_cursor.clone()) else { return };
        self.browse.appending = true;
        let query = self.browse.search_query(Some(cursor));
        self.queue(ExtensionOp::Search { source_id, query }, "Loading more");
    }

    /// Open the details of one result; the bridge answers with `BrowseEvent::Detail`.
    pub fn open_store_details(&mut self, entry: &RegistryEntry) {
        let Some(source_id) = self.browse.selected.clone() else { return };
        self.browse.detail = None;
        self.view = PanelView::StoreDetails { package_id: entry.package_id.clone() };
        self.queue(ExtensionOp::LoadDetail { source_id, entry: entry.clone() }, "Loading details");
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
                        state.browse.rows.clear();
                        state.browse.next_cursor = None;
                        state.browse.detail = None;
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
        let sort_before = state.browse.sort;
        egui::ComboBox::from_id_salt("extension-browse-sort")
            .selected_text(sort_label(state.browse.sort))
            .show_ui(ui, |ui| {
                for sort in [SearchSort::Updated, SearchSort::Name] {
                    ui.selectable_value(&mut state.browse.sort, sort, sort_label(sort));
                }
            })
            .response
            .named(ui, "Sort extensions");
        if submitted || state.browse.sort != sort_before || ui.button("Refresh").button_named(ui, "Refresh extension list").clicked() {
            refresh = true;
        }
    });
    // A selected source lists itself once; Refresh, a new sort and Enter search it again.
    let first_listing = state.browse.status == BrowseStatus::Idle && state.working.is_none();
    if refresh || first_listing {
        state.request_search();
    }
    ui.add_space(8.0);
    match state.browse.status.clone() {
        BrowseStatus::Idle | BrowseStatus::Loading => {}
        BrowseStatus::Failed(reason) => {
            ui.label(egui::RichText::new(reason).color(theme::STATUS_OFFLINE));
        }
        BrowseStatus::Ready => draw_rows(ui, state),
    }
}

fn sort_label(sort: SearchSort) -> &'static str {
    match sort {
        SearchSort::Updated => "Newest",
        SearchSort::Name => "Name",
    }
}

fn draw_rows(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    if state.browse.from_cache {
        ui.label(egui::RichText::new("Showing the last verified list").small().color(theme::TEXT_MUTED));
    }
    if state.browse.stale {
        ui.label(egui::RichText::new("This list has expired and may be out of date").small().color(theme::TEXT_MUTED));
    }
    if state.browse.rows.is_empty() {
        ui.label(egui::RichText::new("No extensions match").color(theme::TEXT_MUTED));
    }
    let Some(source_id) = state.browse.selected.clone() else { return };
    let busy = state.busy();
    let mut install: Option<RegistryEntry> = None;
    let mut details: Option<RegistryEntry> = None;
    for row in &state.browse.rows {
        let entry = &row.entry;
        let installed = state.rows.iter().any(|installed| installed.package_id == entry.package_id && installed.version == entry.version);
        let newer = state.rows.iter().any(|installed| installed.package_id == entry.package_id && installed.version != entry.version);
        ui.horizontal(|ui| {
            let title = if entry.name.is_empty() { entry.package_id.as_str() } else { entry.name.as_str() };
            let heading = egui::RichText::new(format!("{title} {}", entry.version)).strong();
            if !row.actionable() {
                ui.label(heading);
                let why = if row.revoked { "Unavailable: revoked by the registry" } else { "Unavailable: not verified" };
                ui.label(egui::RichText::new(why).small().color(theme::STATUS_OFFLINE));
                return;
            }
            if ui.link(heading).button_named(ui, format!("Details of {}", entry.package_id)).clicked() {
                details = Some(entry.clone());
            }
            if row.licensed() {
                ui.label(egui::RichText::new("Licensed").small().color(theme::TEXT_MUTED));
                return;
            }
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
        let mut meta = vec![entry.package_id.clone()];
        if !row.publisher.is_empty() {
            meta.push(row.publisher.clone());
        }
        if entry.size > 0 {
            meta.push(human_size(entry.size));
        }
        ui.label(egui::RichText::new(meta.join(" · ")).small().color(theme::TEXT_MUTED));
        if !row.summary.is_empty() {
            ui.label(egui::RichText::new(&row.summary).small());
        }
        ui.separator();
    }
    for refusal in &state.browse.refused {
        ui.label(egui::RichText::new(format!("{}: {}", refusal.what, refusal.reason)).small().color(theme::STATUS_OFFLINE));
    }
    if state.browse.next_cursor.is_some()
        && ui.add_enabled(!busy, egui::Button::new("Load more")).button_named(ui, "Load more extensions").clicked()
    {
        state.request_more_results();
    }
    if let Some(entry) = details {
        state.open_store_details(&entry);
    } else if let Some(entry) = install {
        state.queue(ExtensionOp::InstallFromRegistry { source_id, entry }, "Fetching and verifying package");
    }
}

pub(super) fn human_size(bytes: u64) -> String {
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
