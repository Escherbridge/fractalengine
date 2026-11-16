//! Model library UI state, op queue and result folding; see `fe-ui/src/model_library/AGENTS.md`.

pub mod panel;
pub mod text;

use std::collections::{HashMap, HashSet};

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::terrain_map::dto::{
    MAX_REGISTRY_ERROR_CHARS, MAX_REGISTRY_ROWS, MAX_REGISTRY_TEXT_CHARS, RegistryPackageRow,
};
use text::display_text;

/// Largest package the library installs (bytes); mirrors the registry client's cap.
pub const PACKAGE_LIMIT_BYTES: u64 = fe_hexon::registry_source::PACKAGE_MAX_BYTES;
/// Largest preview member fetched (bytes); a larger listed preview shows the placeholder.
pub const THUMBNAIL_LIMIT_BYTES: u64 = fe_hexon::thumbnail::MAX_THUMBNAIL_BYTES as u64;
/// Decoded thumbnails kept as textures; the least recently shown is evicted.
pub const THUMBNAIL_CACHE_CAPACITY: usize = 64;
/// Thumbnail fetches in flight at once.
pub const MAX_THUMBNAIL_FETCHES: usize = 4;
/// Longest attribution text kept (chars).
pub const MAX_ATTRIBUTION_CHARS: usize = 512;
/// Longest description kept (chars).
pub const MAX_DESCRIPTION_CHARS: usize = 1000;
/// Longest SPDX id kept (chars).
const MAX_SPDX_CHARS: usize = 64;
/// Longest tag kept (chars).
const MAX_TAG_CHARS: usize = 40;
/// Most tags kept per package or entry.
const MAX_TAGS: usize = 16;
/// Most tag filter chips offered.
const MAX_TAG_CHIPS: usize = 24;
/// Prefix of every Place request id, so the import result router can tell library placements apart.
pub const PLACE_REQUEST_PREFIX: &str = "model-library-";

/// True when a `NativeImportRequest.request_id` was minted by this panel's Place.
pub fn is_library_request(request_id: &str) -> bool {
    request_id.starts_with(PLACE_REQUEST_PREFIX)
}

/// Which of the section's three views is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelView {
    #[default]
    Browse,
    Installed,
    Credits,
}

/// One view's data: not requested yet, loading, ready, or failed with a reason.
#[derive(Debug, Clone)]
pub enum Load<T> {
    NotAsked,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Default for Load<T> {
    fn default() -> Self {
        Self::NotAsked
    }
}

/// Licence family of a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenceKind {
    Free,
    Attribution,
    Paid,
    NotStated,
}

/// A package licence with display-safe text; build with [`Licence::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Licence {
    pub kind: LicenceKind,
    pub spdx_id: Option<String>,
    pub attribution: Option<String>,
    /// Chip text, e.g. "Free · CC0-1.0".
    pub chip: String,
}

impl Licence {
    /// From the wire words (`free` / `attribution` / `paid`); anything else reads as not stated.
    pub fn new(license_type: Option<&str>, spdx_id: Option<&str>, attribution: Option<&str>) -> Self {
        let kind = match license_type.map(str::trim) {
            Some(word) if word.eq_ignore_ascii_case("free") => LicenceKind::Free,
            Some(word) if word.eq_ignore_ascii_case("attribution") => LicenceKind::Attribution,
            Some(word) if word.eq_ignore_ascii_case("paid") => LicenceKind::Paid,
            _ => LicenceKind::NotStated,
        };
        let spdx_id = non_empty(spdx_id.map(|value| display_text(value, MAX_SPDX_CHARS)));
        let attribution =
            non_empty(attribution.map(|value| display_text(value, MAX_ATTRIBUTION_CHARS)));
        let word = match kind {
            LicenceKind::Free => "Free",
            LicenceKind::Attribution => "Attribution",
            LicenceKind::Paid => "Paid",
            LicenceKind::NotStated => "Not stated",
        };
        let chip = match &spdx_id {
            Some(spdx) => format!("{word} \u{00b7} {spdx}"),
            None => word.to_owned(),
        };
        Self { kind, spdx_id, attribution, chip }
    }
}

/// One registry model package as listed; build with [`ModelRow::new`] so every shown string is sanitised.
#[derive(Debug, Clone)]
pub struct ModelRow {
    /// Raw listing facts; `hexon_id` and `version` address ops, never shown unsanitised.
    pub package: RegistryPackageRow,
    /// `{hexon_id}@{version}`.
    pub uri: String,
    pub title: String,
    pub publisher: String,
    pub description: String,
    pub tags: Vec<String>,
    pub licence: Licence,
    /// Preview member path inside the package.
    pub preview: Option<String>,
    pub preview_bytes: Option<u64>,
    pub size_label: String,
}

impl ModelRow {
    /// Sanitise the publisher-controlled fields once, at the fold boundary.
    pub fn new(
        package: RegistryPackageRow,
        description: Option<&str>,
        tags: &[String],
        licence: Licence,
        preview: Option<String>,
        preview_bytes: Option<u64>,
    ) -> Self {
        let uri = format!("{}@{}", package.hexon_id, package.version);
        let title = title_text(&package.name, &package.hexon_id);
        let publisher = display_text(&package.publisher_did, MAX_REGISTRY_TEXT_CHARS);
        Self {
            uri,
            title,
            publisher,
            description: display_text(description.unwrap_or_default(), MAX_DESCRIPTION_CHARS),
            tags: tag_texts(tags),
            licence,
            preview,
            preview_bytes,
            size_label: format_size(package.size_bytes),
            package,
        }
    }

    /// Over the package limit: listed, never installable.
    pub fn too_large(&self) -> bool {
        self.package.size_bytes > PACKAGE_LIMIT_BYTES
    }
}

/// One model inside a package; `entry_id` stays raw for ops.
#[derive(Debug, Clone)]
pub struct ModelEntryRow {
    pub entry_id: String,
    pub label: String,
    pub tags: Vec<String>,
    pub placeable: bool,
}

impl ModelEntryRow {
    /// Sanitised label (falls back to the entry id) and tags.
    pub fn new(entry_id: &str, label: &str, tags: &[String], placeable: bool) -> Self {
        Self {
            entry_id: entry_id.to_owned(),
            label: title_text(label, entry_id),
            tags: tag_texts(tags),
            placeable,
        }
    }
}

/// The reviewed build an install must match: the verified header's content hash and signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPin {
    pub content_hash: String,
    pub signer_did: String,
}

/// Identity facts of a verified header, as the bridge read them.
#[derive(Debug, Clone)]
pub struct VerifiedIdentity<'a> {
    pub hexon_id: &'a str,
    /// Manifest name.
    pub name: &'a str,
    /// Publisher DID the seal check returned.
    pub signer_did: &'a str,
    /// BLAKE3 of the signed manifest with the signature blanked.
    pub content_hash: &'a str,
}

/// Signed facts of one package, read from its verified header; never from the listing.
#[derive(Debug, Clone)]
pub struct ModelDetail {
    /// Verified manifest name (sanitised; the id when blank).
    pub title: String,
    /// Verified signer DID, shortened for display.
    pub signer_short: String,
    /// Verified signer DID, sanitised, for hover text.
    pub signer_full: String,
    /// Raw verified identity an install is pinned to; never painted.
    pub pin: InstallPin,
    pub description: String,
    pub publisher_name: Option<String>,
    pub licence: Licence,
    pub models: Vec<ModelEntryRow>,
    /// The signed licence, signer or name differs from the listing's claim.
    pub listing_out_of_date: bool,
}

impl ModelDetail {
    /// `listing_out_of_date` is decided when folded against the listed row.
    pub fn new(
        identity: VerifiedIdentity<'_>,
        description: Option<&str>,
        publisher_name: Option<&str>,
        licence: Licence,
        models: Vec<ModelEntryRow>,
    ) -> Self {
        let signer_full = display_text(identity.signer_did, MAX_REGISTRY_TEXT_CHARS);
        Self {
            title: title_text(identity.name, identity.hexon_id),
            signer_short: short_did(&signer_full),
            signer_full,
            pin: InstallPin {
                content_hash: identity.content_hash.to_owned(),
                signer_did: identity.signer_did.to_owned(),
            },
            description: display_text(description.unwrap_or_default(), MAX_DESCRIPTION_CHARS),
            publisher_name: non_empty(
                publisher_name.map(|name| display_text(name, MAX_REGISTRY_TEXT_CHARS)),
            ),
            licence,
            models,
            listing_out_of_date: false,
        }
    }

    /// The listing disagrees with the verified header on licence, signer or name.
    fn differs_from(&self, row: &ModelRow) -> bool {
        row.licence != self.licence
            || row.package.publisher_did != self.pin.signer_did
            || row.title != self.title
    }
}

/// First 16 and last 6 characters of a long DID around an ellipsis; short ones unchanged.
fn short_did(did: &str) -> String {
    const HEAD: usize = 16;
    const TAIL: usize = 6;
    let count = did.chars().count();
    if count <= HEAD + TAIL + 1 {
        return did.to_owned();
    }
    let head: String = did.chars().take(HEAD).collect();
    let tail: String = did.chars().skip(count - TAIL).collect();
    format!("{head}\u{2026}{tail}")
}

/// One package installed on this device.
#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub uri: String,
    pub hexon_id: String,
    pub title: String,
    /// Publisher name when given, otherwise the publisher DID.
    pub publisher: String,
    pub description: String,
    pub licence: Licence,
    pub models: Vec<ModelEntryRow>,
    pub size_label: String,
}

impl InstalledPackage {
    /// Sanitise once at the fold boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        uri: String,
        hexon_id: String,
        name: &str,
        publisher_did: &str,
        publisher_name: Option<&str>,
        description: Option<&str>,
        licence: Licence,
        models: Vec<ModelEntryRow>,
        size_bytes: u64,
    ) -> Self {
        let publisher = match publisher_name.map(str::trim) {
            Some(name) if !name.is_empty() => display_text(name, MAX_REGISTRY_TEXT_CHARS),
            _ => display_text(publisher_did, MAX_REGISTRY_TEXT_CHARS),
        };
        Self {
            title: title_text(name, &hexon_id),
            uri,
            hexon_id,
            publisher,
            description: display_text(description.unwrap_or_default(), MAX_DESCRIPTION_CHARS),
            licence,
            models,
            size_label: format_size(size_bytes),
        }
    }
}

/// The active petal's credits: installed packages whose models it uses.
#[derive(Debug, Clone, Default)]
pub struct Credits {
    pub packages: Vec<InstalledPackage>,
    /// Distinct used assets with no installed source package on this device.
    pub sources_not_installed: usize,
}

/// A registry install in progress or failed, keyed by uri.
#[derive(Debug, Clone)]
pub enum InstallState {
    Running { received_bytes: u64, total_bytes: Option<u64> },
    Failed(String),
}

/// Work the main binary performs off the Bevy main thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOp {
    /// Search the registry for `type=model` packages; echo `generation` in `ModelResult::Listed`.
    List { generation: u64, query: String, tags: Vec<String> },
    /// List packages installed on this device.
    ListInstalled,
    /// Read the signed header (licence, entries) of one package.
    Detail { uri: String },
    /// Fetch and decode one preview member (verified against the signed root).
    Thumbnail { uri: String, member: String },
    /// Fetch, verify and install a package; `pin` is the reviewed detail's build, else the bridge pins to a fresh verified header.
    Install { uri: String, pin: Option<InstallPin> },
    /// Remove an installed package.
    Remove { uri: String },
    /// Place one model entry in the petal at the camera focus; `request_id` correlates the import result.
    Place { request_id: String, uri: String, entry_id: String, petal_id: String },
    /// Credits for a petal: its asset hashes joined to installed packages.
    LoadCredits { petal_id: String },
}

/// Queue of model ops; fe-ui has no registry or filesystem access.
#[derive(Debug, Default, Resource)]
pub struct PendingModelOps(pub Vec<ModelOp>);

/// Outcome of a [`ModelOp`], folded by [`ModelLibraryState::apply`].
#[derive(Debug, Clone)]
pub enum ModelResult {
    Listed { generation: u64, result: Result<Vec<ModelRow>, String> },
    InstalledListed(Result<Vec<InstalledPackage>, String>),
    Detail { uri: String, result: Result<ModelDetail, String> },
    /// Already-decoded RGBA from `fe_hexon::thumbnail::decode` in the bridge.
    Thumbnail { uri: String, result: Result<fe_hexon::thumbnail::Rgba, String> },
    InstallProgress { uri: String, received_bytes: u64, total_bytes: Option<u64> },
    Installed { uri: String, result: Result<InstalledPackage, String> },
    Removed { uri: String, result: Result<(), String> },
    /// The placement committed (routed from `GltfImported` carrying a library request).
    Placed { request_id: String },
    /// The placement was refused before or by the DB.
    PlaceFailed { request_id: String, error: String },
    Credits { petal_id: String, result: Result<Credits, String> },
}

/// A thumbnail's lifecycle in the cache.
enum ThumbnailSlot {
    Fetching,
    Decoded(fe_hexon::thumbnail::Rgba),
    Ready(egui::TextureHandle),
    Unavailable,
}

struct ThumbnailEntry {
    slot: ThumbnailSlot,
    last_used: u64,
}

/// Bounded LRU of thumbnail textures with a cap on fetches in flight.
#[derive(Default)]
pub struct ThumbnailCache {
    entries: HashMap<String, ThumbnailEntry>,
    in_flight: usize,
    tick: u64,
}

impl ThumbnailCache {
    /// Advance the recency clock once per panel frame.
    fn next_frame(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// Queue a fetch for a visible row when it is not cached and a fetch slot is free.
    fn request(&mut self, uri: &str, preview: Option<&str>, preview_bytes: Option<u64>) -> Option<ModelOp> {
        if let Some(entry) = self.entries.get_mut(uri) {
            entry.last_used = self.tick;
            return None;
        }
        let member = preview?;
        let fits = preview_bytes.is_none_or(|bytes| bytes <= THUMBNAIL_LIMIT_BYTES);
        if !fits {
            self.insert(uri, ThumbnailSlot::Unavailable);
            return None;
        }
        if self.in_flight >= MAX_THUMBNAIL_FETCHES {
            return None;
        }
        self.in_flight += 1;
        self.insert(uri, ThumbnailSlot::Fetching);
        Some(ModelOp::Thumbnail { uri: uri.to_owned(), member: member.to_owned() })
    }

    fn insert(&mut self, uri: &str, slot: ThumbnailSlot) {
        self.entries.insert(uri.to_owned(), ThumbnailEntry { slot, last_used: self.tick });
        while self.entries.len() > THUMBNAIL_CACHE_CAPACITY {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| !matches!(entry.slot, ThumbnailSlot::Fetching))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Fold a fetch outcome; malformed pixels read as unavailable.
    fn receive(&mut self, uri: &str, result: Result<fe_hexon::thumbnail::Rgba, String>) {
        let Some(entry) = self.entries.get_mut(uri) else { return };
        if !matches!(entry.slot, ThumbnailSlot::Fetching) {
            return;
        }
        self.in_flight = self.in_flight.saturating_sub(1);
        entry.slot = match result {
            Ok(image) if pixels_valid(&image) => ThumbnailSlot::Decoded(image),
            _ => ThumbnailSlot::Unavailable,
        };
    }

    /// The row's texture and pixel size, uploading decoded pixels on first use; `None` shows the placeholder.
    fn texture(&mut self, ctx: &egui::Context, uri: &str) -> Option<(egui::TextureId, egui::Vec2)> {
        let entry = self.entries.get_mut(uri)?;
        entry.last_used = self.tick;
        if let ThumbnailSlot::Decoded(image) = &entry.slot {
            let size = [image.width as usize, image.height as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, &image.pixels);
            let handle =
                ctx.load_texture(format!("model-thumbnail-{uri}"), color, egui::TextureOptions::LINEAR);
            entry.slot = ThumbnailSlot::Ready(handle);
        }
        match &entry.slot {
            ThumbnailSlot::Ready(handle) => Some((handle.id(), handle.size_vec2())),
            _ => None,
        }
    }

    /// Cached entries (for the bound check).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No cached entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Exact RGBA8 length within the decoder's side cap; guards `ColorImage`'s length assert.
fn pixels_valid(image: &fe_hexon::thumbnail::Rgba) -> bool {
    let side = fe_hexon::thumbnail::MAX_THUMBNAIL_SIDE;
    if image.width == 0 || image.height == 0 || image.width > side || image.height > side {
        return false;
    }
    (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|count| count.checked_mul(4))
        .is_some_and(|length| length == image.pixels.len())
}

/// Models section state (UI only, like `PetalMapState`); written by the panel and [`Self::apply`].
#[derive(Resource, Default)]
pub struct ModelLibraryState {
    pub view: ModelView,
    pub search_text: String,
    pub tag_filter: Vec<String>,
    browse: Load<Vec<ModelRow>>,
    browse_generation: u64,
    /// Tag chips: tags of the last listing plus the selected ones.
    available_tags: Vec<String>,
    /// Uri of the package whose detail is open.
    selected: Option<String>,
    detail: Load<ModelDetail>,
    installed: Load<Vec<InstalledPackage>>,
    installs: HashMap<String, InstallState>,
    removing: HashSet<String>,
    remove_error: Option<String>,
    /// Place request id -> (uri, entry id).
    placing: HashMap<String, (String, String)>,
    place_error: Option<String>,
    credits: Load<Credits>,
    credits_petal: Option<String>,
    thumbnails: ThumbnailCache,
}

impl ModelLibraryState {
    /// Fold one bridge result; stale or unrequested results are dropped.
    pub fn apply(&mut self, result: ModelResult) {
        match result {
            ModelResult::Listed { generation, result } => {
                if generation != self.browse_generation {
                    return;
                }
                self.browse = match result {
                    Ok(mut rows) => {
                        rows.truncate(MAX_REGISTRY_ROWS);
                        self.available_tags = tag_chips(&rows, &self.tag_filter);
                        Load::Ready(rows)
                    }
                    Err(error) => Load::Failed(error_text(&error)),
                };
            }
            ModelResult::InstalledListed(result) => {
                self.installed = match result {
                    Ok(packages) => Load::Ready(packages),
                    Err(error) => Load::Failed(error_text(&error)),
                };
            }
            ModelResult::Detail { uri, result } => {
                if self.selected.as_deref() != Some(uri.as_str()) {
                    return;
                }
                self.detail = match result {
                    Ok(mut detail) => {
                        if let Some(row) = self.row(&uri) {
                            detail.listing_out_of_date = detail.differs_from(row);
                        }
                        Load::Ready(detail)
                    }
                    Err(error) => Load::Failed(error_text(&error)),
                };
            }
            ModelResult::Thumbnail { uri, result } => self.thumbnails.receive(&uri, result),
            ModelResult::InstallProgress { uri, received_bytes, total_bytes } => {
                if matches!(self.installs.get(&uri), Some(InstallState::Running { .. })) {
                    self.installs
                        .insert(uri, InstallState::Running { received_bytes, total_bytes });
                }
            }
            ModelResult::Installed { uri, result } => match result {
                Ok(package) => {
                    self.installs.remove(&uri);
                    if let Load::Ready(packages) = &mut self.installed {
                        packages.retain(|existing| existing.uri != package.uri);
                        packages.push(package);
                        packages.sort_by(|a, b| a.uri.cmp(&b.uri));
                    } else {
                        self.installed = Load::NotAsked;
                    }
                    self.credits_petal = None;
                }
                Err(error) => {
                    self.installs.insert(uri, InstallState::Failed(error_text(&error)));
                }
            },
            ModelResult::Removed { uri, result } => {
                self.removing.remove(&uri);
                match result {
                    Ok(()) => {
                        if let Load::Ready(packages) = &mut self.installed {
                            packages.retain(|package| package.uri != uri);
                        }
                        self.remove_error = None;
                        self.credits_petal = None;
                    }
                    Err(error) => self.remove_error = Some(error_text(&error)),
                }
            }
            ModelResult::Placed { request_id } => {
                if self.placing.remove(&request_id).is_some() {
                    self.place_error = None;
                    self.credits_petal = None;
                }
            }
            ModelResult::PlaceFailed { request_id, error } => {
                if self.placing.remove(&request_id).is_some() {
                    self.place_error = Some(error_text(&error));
                }
            }
            ModelResult::Credits { petal_id, result } => {
                if self.credits_petal.as_deref() != Some(petal_id.as_str()) {
                    return;
                }
                self.credits = match result {
                    Ok(credits) => Load::Ready(credits),
                    Err(error) => Load::Failed(error_text(&error)),
                };
            }
        }
    }

    /// Per-frame: issue first loads for the shown view and follow the active petal for credits.
    fn prepare(&mut self, active_petal_id: Option<&str>, ops: &mut PendingModelOps) {
        self.thumbnails.next_frame();
        if matches!(self.installed, Load::NotAsked) {
            self.request_installed(ops);
        }
        let view = self.view;
        match view {
            ModelView::Browse if matches!(self.browse, Load::NotAsked) => self.request_listing(ops),
            ModelView::Credits if self.credits_petal.as_deref() != active_petal_id => {
                match active_petal_id {
                    Some(petal_id) => self.request_credits(petal_id, ops),
                    None => {
                        self.credits_petal = None;
                        self.credits = Load::NotAsked;
                    }
                }
            }
            _ => {}
        }
    }

    /// Search the registry with the current text and tag filter.
    fn request_listing(&mut self, ops: &mut PendingModelOps) {
        self.browse_generation = self.browse_generation.wrapping_add(1);
        self.browse = Load::Loading;
        self.selected = None;
        ops.0.push(ModelOp::List {
            generation: self.browse_generation,
            query: self.search_text.trim().to_owned(),
            tags: self.tag_filter.clone(),
        });
    }

    fn request_installed(&mut self, ops: &mut PendingModelOps) {
        self.installed = Load::Loading;
        ops.0.push(ModelOp::ListInstalled);
    }

    fn request_credits(&mut self, petal_id: &str, ops: &mut PendingModelOps) {
        self.credits_petal = Some(petal_id.to_owned());
        self.credits = Load::Loading;
        ops.0.push(ModelOp::LoadCredits { petal_id: petal_id.to_owned() });
    }

    fn open_detail(&mut self, uri: &str, ops: &mut PendingModelOps) {
        self.selected = Some(uri.to_owned());
        self.detail = Load::Loading;
        ops.0.push(ModelOp::Detail { uri: uri.to_owned() });
    }

    fn toggle_tag(&mut self, tag: &str, ops: &mut PendingModelOps) {
        if let Some(index) = self.tag_filter.iter().position(|selected| selected == tag) {
            self.tag_filter.remove(index);
        } else {
            self.tag_filter.push(tag.to_owned());
        }
        self.request_listing(ops);
    }

    /// Queue an install, pinned to the open detail's verified build when it is this package's.
    fn install(&mut self, uri: &str, ops: &mut PendingModelOps) {
        let pin = match (&self.selected, &self.detail) {
            (Some(selected), Load::Ready(detail)) if selected == uri => Some(detail.pin.clone()),
            _ => None,
        };
        self.installs.insert(
            uri.to_owned(),
            InstallState::Running { received_bytes: 0, total_bytes: None },
        );
        ops.0.push(ModelOp::Install { uri: uri.to_owned(), pin });
    }

    fn remove(&mut self, uri: &str, ops: &mut PendingModelOps) {
        self.removing.insert(uri.to_owned());
        self.remove_error = None;
        ops.0.push(ModelOp::Remove { uri: uri.to_owned() });
    }

    fn place(&mut self, uri: &str, entry_id: &str, petal_id: &str, ops: &mut PendingModelOps) {
        let request_id = format!("{PLACE_REQUEST_PREFIX}{}", ulid::Ulid::new());
        self.placing
            .insert(request_id.clone(), (uri.to_owned(), entry_id.to_owned()));
        self.place_error = None;
        ops.0.push(ModelOp::Place {
            request_id,
            uri: uri.to_owned(),
            entry_id: entry_id.to_owned(),
            petal_id: petal_id.to_owned(),
        });
    }

    fn row(&self, uri: &str) -> Option<&ModelRow> {
        match &self.browse {
            Load::Ready(rows) => rows.iter().find(|row| row.uri == uri),
            _ => None,
        }
    }
}

/// A Place for this entry is awaiting its result.
fn is_placing(placing: &HashMap<String, (String, String)>, uri: &str, entry_id: &str) -> bool {
    placing
        .values()
        .any(|(placing_uri, placing_entry)| placing_uri == uri && placing_entry == entry_id)
}

/// Some version of this package id is installed.
fn is_installed(installed: &Load<Vec<InstalledPackage>>, hexon_id: &str) -> bool {
    matches!(installed, Load::Ready(packages) if packages.iter().any(|package| package.hexon_id == hexon_id))
}

/// Union of listed tags plus the selected ones, sorted and capped.
fn tag_chips(rows: &[ModelRow], selected: &[String]) -> Vec<String> {
    let mut chips: Vec<String> = selected.to_vec();
    for tag in rows.iter().flat_map(|row| row.tags.iter()) {
        if chips.len() >= MAX_TAG_CHIPS {
            break;
        }
        if !tag.is_empty() && !chips.contains(tag) {
            chips.push(tag.clone());
        }
    }
    chips.sort();
    chips
}

fn tag_texts(tags: &[String]) -> Vec<String> {
    tags.iter()
        .take(MAX_TAGS)
        .map(|tag| display_text(tag, MAX_TAG_CHARS))
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// Sanitised name, falling back to the sanitised id when the name is blank.
fn title_text(name: &str, fallback: &str) -> String {
    let title = display_text(name, MAX_REGISTRY_TEXT_CHARS);
    if title.is_empty() {
        display_text(fallback, MAX_REGISTRY_TEXT_CHARS)
    } else {
        title
    }
}

fn error_text(error: &str) -> String {
    display_text(error, MAX_REGISTRY_ERROR_CHARS)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// Human-readable byte size.
pub(crate) fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} KB", value / KIB)
    } else if value < KIB * KIB * KIB {
        format!("{:.1} MB", value / (KIB * KIB))
    } else {
        format!("{:.2} GB", value / (KIB * KIB * KIB))
    }
}
