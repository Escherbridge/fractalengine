//! Models section bridge: runs fe-ui's `PendingModelOps` on worker threads and folds results back.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;

use fe_hexon::manifest::{EntryKind, HexonType, License, LicenseType};
use fe_hexon::model_library::{ModelLibrary, ModelLibraryError, ModelPackageSummary, MODEL_HEADER_MAX_BYTES};
use fe_hexon::partial::{FetchStrategy, FetchedHeader, PartialFetchError, SharedSource};
use fe_hexon::registry_source::{RegistryError, PACKAGE_MAX_BYTES};
use fe_hexon::remote::RemoteRegistryClient;
use fe_runtime::app::DbCommandSender;
use fe_runtime::blob_store::BlobStoreHandle;
use fe_runtime::messages::{DbCommand, DbResult, NativeImportRequest};
use fe_ui::model_library::text::display_text;
use fe_ui::model_library::{
    Credits, InstallPin, InstalledPackage, Licence, ModelDetail, ModelEntryRow, ModelLibraryState, ModelOp,
    ModelResult, ModelRow, PendingModelOps, VerifiedIdentity, MAX_THUMBNAIL_FETCHES, THUMBNAIL_LIMIT_BYTES,
};
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::node_manager::{try_submit_scene_write, GuiSend};
use fe_ui::settings::AppSettings;

use crate::registry_listing::{package_row, registry_hits, REGISTRY_MODEL_TYPE};
use crate::terrain_bridge::{resolve_hexon_registry_token, HEXON_REGISTRY_TOKEN_ENV_VAR};

/// User-facing text for any failed signature, seal, digest or content-hash check (spec U2).
const VERIFICATION_FAILED: &str = "Failed verification";
/// User-facing text for a package over the install cap (spec U1).
const TOO_LARGE: &str = "Too large to install (64 MiB limit)";
/// Worker jobs (list, detail, install, remove, place, credits) running at once; thumbnails are capped separately.
const MAX_WORKER_JOBS: usize = 6;
/// Ops waiting for a free worker; past this, new ops are refused with a retry message.
const MAX_BACKLOG: usize = 256;
/// Most tags sent as one search filter.
const MAX_SEARCH_TAGS: usize = 16;
/// Longest preview member path accepted from a listing.
const MAX_PREVIEW_PATH_CHARS: usize = 256;
/// Longest node name a placement writes.
const MAX_NODE_NAME_CHARS: usize = 120;
/// How long a credits read may wait for the database before it is reported failed.
const CREDITS_TIMEOUT: Duration = Duration::from_secs(30);
/// Prefix of the `ReadPetalAssetHashes` correlation ids this bridge mints.
const CREDITS_CORRELATION_PREFIX: &str = "model-credits-";
/// Verified headers kept for reuse by Detail, Thumbnail and Install; the least recently used is dropped.
const HEADER_CACHE_CAPACITY: usize = 32;
/// User-facing text when the downloaded build is not the reviewed one (signer or content hash).
const BUILD_CHANGED: &str = "Not installed: the registry delivered a different build than the one reviewed";

/// Identity of one op while it runs; a second op with the same key is not started.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum OpKey {
    List(u64),
    Installed,
    Detail(String),
    Thumbnail(String),
    Install(String),
    Remove(String),
    Place(String),
    Credits(String),
}

impl OpKey {
    fn of(op: &ModelOp) -> Self {
        match op {
            ModelOp::List { generation, .. } => Self::List(*generation),
            ModelOp::ListInstalled => Self::Installed,
            ModelOp::Detail { uri } => Self::Detail(uri.clone()),
            ModelOp::Thumbnail { uri, .. } => Self::Thumbnail(uri.clone()),
            ModelOp::Install { uri, .. } => Self::Install(uri.clone()),
            ModelOp::Remove { uri } => Self::Remove(uri.clone()),
            ModelOp::Place { request_id, .. } => Self::Place(request_id.clone()),
            ModelOp::LoadCredits { petal_id } => Self::Credits(petal_id.clone()),
        }
    }
}

/// What an in-flight op is waiting on.
#[derive(Debug)]
enum Slot {
    Worker,
    Thumbnail,
    /// A credits read sent to the DB thread, keyed by its correlation id.
    Database { correlation_id: String, since: Instant },
}

/// A placeable entry resolved off-thread; the main thread adds navigation and camera focus.
#[derive(Debug, Clone, PartialEq)]
struct PlaceTarget {
    request_id: String,
    petal_id: String,
    name: String,
    content_hash: String,
    size_bytes: u64,
}

/// Worker-to-main-thread message.
enum BridgeEvent {
    /// Interim result; the op stays in flight.
    Progress(ModelResult),
    /// Final result of the op holding `key`.
    Done { key: OpKey, result: ModelResult },
    /// A Place entry resolved; the main thread submits it through the scene-write door.
    PlaceReady { key: OpKey, target: PlaceTarget },
}

/// One cache slot; its mutex is held while the header is fetched, so concurrent ops for one package fetch once.
type HeaderSlot = Arc<Mutex<Option<Arc<FetchedHeader>>>>;

/// Bounded LRU of VERIFIED headers keyed by (registry url, package uri); a failed fetch is never stored.
#[derive(Default)]
struct HeaderCache {
    /// Most recently used first.
    slots: Mutex<VecDeque<((String, String), HeaderSlot)>>,
}

impl HeaderCache {
    /// The slot for `key`, now most recent; slots past capacity are dropped.
    fn slot(&self, key: (String, String)) -> HeaderSlot {
        let mut slots = self.slots.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = match slots.iter().position(|(held, _)| *held == key) {
            Some(index) => slots.remove(index).map(|(_, slot)| slot).unwrap_or_default(),
            None => HeaderSlot::default(),
        };
        slots.push_front((key, slot.clone()));
        slots.truncate(HEADER_CACHE_CAPACITY);
        slot
    }

    /// The cached header for `key`, else `fetch`'s verified header, which is then cached.
    fn get_or_fetch(
        &self,
        key: (String, String),
        fetch: impl FnOnce() -> Result<FetchedHeader, String>,
    ) -> Result<Arc<FetchedHeader>, String> {
        let slot = self.slot(key);
        let mut held = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(header) = held.as_ref() {
            return Ok(header.clone());
        }
        let header = Arc::new(fetch()?);
        *held = Some(header.clone());
        Ok(header)
    }
}

/// Library, result channel and in-flight bookkeeping for the Models section (see `src/AGENTS.md` §model-library).
#[derive(Resource)]
pub struct ModelLibraryBridge {
    library: Arc<ModelLibrary>,
    /// Shared blob store; placement reads the stored GLB's size from it.
    blobs: BlobStoreHandle,
    /// Verified headers shared by Detail, Thumbnail and Install.
    headers: Arc<HeaderCache>,
    /// Serialises install and remove (the library's not-installed pre-check assumes one writer).
    write_lock: Arc<Mutex<()>>,
    tx: crossbeam::channel::Sender<BridgeEvent>,
    rx: crossbeam::channel::Receiver<BridgeEvent>,
    in_flight: HashMap<OpKey, Slot>,
    backlog: VecDeque<ModelOp>,
}

impl ModelLibraryBridge {
    /// Bridge over an opened library and the app's shared blob store.
    pub fn new(library: ModelLibrary, blobs: BlobStoreHandle) -> Self {
        let (tx, rx) = crossbeam::channel::unbounded();
        Self {
            library: Arc::new(library),
            blobs,
            headers: Arc::new(HeaderCache::default()),
            write_lock: Arc::new(Mutex::new(())),
            tx,
            rx,
            in_flight: HashMap::new(),
            backlog: VecDeque::new(),
        }
    }

    fn running(&self, thumbnails: bool) -> usize {
        self.in_flight
            .values()
            .filter(|slot| match slot {
                Slot::Thumbnail => thumbnails,
                Slot::Worker => !thumbnails,
                Slot::Database { .. } => false,
            })
            .count()
    }

    /// A worker slot is free for this op's kind.
    fn has_capacity(&self, op: &ModelOp) -> bool {
        match op {
            ModelOp::Thumbnail { .. } => self.running(true) < MAX_THUMBNAIL_FETCHES,
            ModelOp::LoadCredits { .. } => true,
            _ => self.running(false) < MAX_WORKER_JOBS,
        }
    }
}

/// The library directory: a `models` sibling of the map tileset store (same root, QA override included).
pub fn library_dir(tileset_dir: &std::path::Path) -> std::path::PathBuf {
    match tileset_dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("models"),
        _ => tileset_dir.join("models"),
    }
}

/// Wake the reactive render loop while a worker result waits; see src/AGENTS.md §render-loop.
pub fn watch_results(world: &World, work: &crate::render_wake::PendingWork) {
    if let Some(bridge) = world.get_resource::<ModelLibraryBridge>() {
        work.watch(bridge.rx.clone());
    }
}

/// Registry address and token, resolved once per drain at this seam (the token is never logged).
#[derive(Clone)]
struct RegistryAccess {
    url: String,
    token: Option<String>,
}

impl RegistryAccess {
    fn resolve(settings: &AppSettings) -> Self {
        let env_url = std::env::var(fe_ui::settings::HEXON_REGISTRY_URL_ENV_VAR).ok();
        let url = fe_ui::settings::resolve_hexon_registry_url(
            env_url.as_deref(),
            settings.hexon_registry_url.as_deref(),
        );
        let token =
            resolve_hexon_registry_token(std::env::var(HEXON_REGISTRY_TOKEN_ENV_VAR).ok().as_deref());
        Self { url, token }
    }
}

/// Takes queued model ops and starts each on a worker thread, bounded and deduplicated.
pub fn drain_model_ops(
    mut ops: ResMut<PendingModelOps>,
    bridge: Option<ResMut<ModelLibraryBridge>>,
    settings: Res<AppSettings>,
    db_tx: Option<Res<DbCommandSender>>,
    mut state: Option<ResMut<ModelLibraryState>>,
) {
    let Some(mut bridge) = bridge else {
        if !ops.0.is_empty() {
            tracing::warn!(count = ops.0.len(), "Model ops refused: the model library is unavailable");
            for op in ops.0.drain(..) {
                apply(&mut state, refusal_result(&op, "The model library is unavailable on this device"));
            }
        }
        return;
    };
    if ops.0.is_empty() && bridge.backlog.is_empty() {
        return;
    }
    for op in ops.0.drain(..) {
        if bridge.backlog.len() >= MAX_BACKLOG {
            tracing::warn!("Model op refused: too many library requests waiting");
            apply(&mut state, refusal_result(&op, "Too many library requests at once; try again"));
            continue;
        }
        bridge.backlog.push_back(op);
    }

    let access = RegistryAccess::resolve(&settings);
    let mut waiting = VecDeque::new();
    while let Some(op) = bridge.backlog.pop_front() {
        let key = OpKey::of(&op);
        if bridge.in_flight.contains_key(&key) {
            // Already running: its result answers this request too.
            continue;
        }
        if !bridge.has_capacity(&op) {
            waiting.push_back(op);
            continue;
        }
        if let Some(refusal) = start_op(&mut bridge, op, key, &access, db_tx.as_deref()) {
            apply(&mut state, refusal);
        }
    }
    bridge.backlog = waiting;
}

/// Start one op; `Some` is an immediate failure result for the panel.
fn start_op(
    bridge: &mut ModelLibraryBridge,
    op: ModelOp,
    key: OpKey,
    access: &RegistryAccess,
    db_tx: Option<&DbCommandSender>,
) -> Option<ModelResult> {
    if let ModelOp::LoadCredits { petal_id } = &op {
        let Some(db_tx) = db_tx else {
            tracing::warn!(petal_id, "Credits read dropped: no database channel");
            return Some(refusal_result(&op, "Not connected to the database"));
        };
        let correlation_id = format!("{CREDITS_CORRELATION_PREFIX}{}", ulid::Ulid::new());
        let command = DbCommand::ReadPetalAssetHashes {
            petal_id: petal_id.clone(),
            correlation_id: Some(correlation_id.clone()),
        };
        if let Err(error) = db_tx.0.try_send(command) {
            tracing::warn!(petal_id, "ReadPetalAssetHashes send failed: {error}");
            return Some(refusal_result(&op, "Database busy; credits were not loaded"));
        }
        bridge
            .in_flight
            .insert(key, Slot::Database { correlation_id, since: Instant::now() });
        return None;
    }

    let slot = if matches!(op, ModelOp::Thumbnail { .. }) { Slot::Thumbnail } else { Slot::Worker };
    bridge.in_flight.insert(key.clone(), slot);
    let tx = bridge.tx.clone();
    let library = bridge.library.clone();
    let write_lock = bridge.write_lock.clone();
    let blobs = bridge.blobs.clone();
    let headers = bridge.headers.clone();
    let access = access.clone();
    fe_runtime::maintenance::spawn_job(move || {
        let event = run_op(op, key, &library, &write_lock, &blobs, &headers, &access, &tx);
        if tx.send(event).is_err() {
            tracing::warn!("Model library result channel closed");
        }
    });
    None
}

/// The worker body: one op to its final event.
#[allow(clippy::too_many_arguments)]
fn run_op(
    op: ModelOp,
    key: OpKey,
    library: &ModelLibrary,
    write_lock: &Mutex<()>,
    blobs: &BlobStoreHandle,
    headers: &HeaderCache,
    access: &RegistryAccess,
    tx: &crossbeam::channel::Sender<BridgeEvent>,
) -> BridgeEvent {
    let result = match op {
        ModelOp::List { generation, query, tags } => ModelResult::Listed {
            generation,
            result: list_registry(access, &query, &tags),
        },
        ModelOp::ListInstalled => {
            ModelResult::InstalledListed(Ok(library.list().iter().map(installed_package).collect()))
        }
        ModelOp::Detail { uri } => {
            let result = cached_header(headers, access, &uri).and_then(|header| detail_from_header(&header));
            ModelResult::Detail { uri, result }
        }
        ModelOp::Thumbnail { uri, member } => {
            let result = fetch_thumbnail(headers, access, &uri, &member);
            ModelResult::Thumbnail { uri, result }
        }
        ModelOp::Install { uri, pin } => {
            let result = install_from_registry(library, write_lock, headers, access, &uri, pin, tx);
            ModelResult::Installed { uri, result }
        }
        ModelOp::Remove { uri } => {
            let _write = write_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let result = library.remove(&uri).map_err(|error| library_error_text(&error));
            ModelResult::Removed { uri, result }
        }
        ModelOp::Place { request_id, uri, entry_id, petal_id } => {
            match resolve_place(library, blobs, &request_id, &uri, &entry_id, &petal_id) {
                Ok(target) => return BridgeEvent::PlaceReady { key, target },
                Err(error) => ModelResult::PlaceFailed { request_id, error },
            }
        }
        ModelOp::LoadCredits { petal_id } => {
            // Started on the main thread (a DB read); never reaches a worker.
            ModelResult::Credits { petal_id, result: Err("Credits were not requested".into()) }
        }
    };
    BridgeEvent::Done { key, result }
}

/// Folds worker results into the panel state, submits resolved placements and joins credits reads.
pub fn drain_model_results(
    bridge: Option<ResMut<ModelLibraryBridge>>,
    mut state: Option<ResMut<ModelLibraryState>>,
    nav: Res<NavigationManager>,
    db_tx: Option<Res<DbCommandSender>>,
    cameras: Query<&fe_renderer::camera::OrbitCameraController>,
    focus_target: Option<Res<fe_ui::plugin::CameraFocusTarget>>,
    mut db_results: MessageReader<DbResult>,
) {
    let Some(mut bridge) = bridge else { return };
    let bridge = &mut *bridge;

    for result in db_results.read() {
        let DbResult::PetalAssetHashes { petal_id, content_hashes, correlation_id } = result else {
            continue;
        };
        let Some(correlation_id) = correlation_id.as_deref() else { continue };
        let key = OpKey::Credits(petal_id.clone());
        let ours = matches!(
            bridge.in_flight.get(&key),
            Some(Slot::Database { correlation_id: waiting, .. }) if waiting == correlation_id
        );
        if !ours {
            continue;
        }
        bridge.in_flight.insert(key.clone(), Slot::Worker);
        let library = bridge.library.clone();
        let tx = bridge.tx.clone();
        let petal_id = petal_id.clone();
        let hashes = content_hashes.clone();
        fe_runtime::maintenance::spawn_job(move || {
            let by_hash = library.packages_for_hashes(&hashes);
            let result = ModelResult::Credits { petal_id, result: Ok(credits_from(&hashes, &by_hash)) };
            if tx.send(BridgeEvent::Done { key, result }).is_err() {
                tracing::warn!("Model library credits channel closed");
            }
        });
    }

    let now = Instant::now();
    let expired: Vec<OpKey> = bridge
        .in_flight
        .iter()
        .filter(|(_, slot)| matches!(slot, Slot::Database { since, .. } if now.duration_since(*since) > CREDITS_TIMEOUT))
        .map(|(key, _)| key.clone())
        .collect();
    for key in expired {
        bridge.in_flight.remove(&key);
        if let OpKey::Credits(petal_id) = key {
            tracing::warn!(petal_id, "Credits read timed out");
            apply(
                &mut state,
                ModelResult::Credits { petal_id, result: Err("The database did not answer".into()) },
            );
        }
    }

    let orbit_focus = cameras.iter().next().map(|camera| camera.focus.to_array());
    let fallback_focus = focus_target.as_ref().and_then(|focus| focus.target.as_ref().map(|(_, at)| *at));
    while let Ok(event) = bridge.rx.try_recv() {
        match event {
            BridgeEvent::Progress(result) => apply(&mut state, result),
            BridgeEvent::Done { key, result } => {
                bridge.in_flight.remove(&key);
                apply(&mut state, result);
            }
            BridgeEvent::PlaceReady { key, target } => {
                bridge.in_flight.remove(&key);
                let request_id = target.request_id.clone();
                let command = place_command(
                    target,
                    nav.active_verse_id.as_deref(),
                    nav.active_fractal_id.as_deref(),
                    nav.active_petal_id.as_deref(),
                    spawn_position(orbit_focus, fallback_focus),
                );
                let outcome = match (command, db_tx.as_deref()) {
                    (Err(refusal), _) => Some(refusal),
                    (Ok(_), None) => {
                        tracing::warn!(request_id, "Placement dropped: no database channel");
                        place_send_outcome(&request_id, GuiSend::Closed)
                    }
                    (Ok(command), Some(db_tx)) => {
                        let sent = try_submit_scene_write(db_tx, command);
                        if sent.channel_unavailable() {
                            tracing::warn!(request_id, ?sent, "PlaceStoredGlb was not sent");
                        }
                        place_send_outcome(&request_id, sent)
                    }
                };
                if let Some(result) = outcome {
                    apply(&mut state, result);
                }
            }
        }
    }
}

fn apply(state: &mut Option<ResMut<ModelLibraryState>>, result: ModelResult) {
    match state {
        Some(state) => state.apply(result),
        None => tracing::warn!("Model library result dropped: no panel state registered"),
    }
}

// ---- worker jobs (blocking; never on the Bevy main thread) ----

/// Run one future on a private current-thread runtime (the worker owns it start to end).
fn block_on<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the registry client: {error}"))?;
    Ok(runtime.block_on(future))
}

/// Registry search for model packages; rows carry the listing's licence and preview for display only.
fn list_registry(access: &RegistryAccess, query: &str, tags: &[String]) -> Result<Vec<ModelRow>, String> {
    let query = Some(query.trim()).filter(|text| !text.is_empty());
    let tags = search_tags(tags);
    let client = RemoteRegistryClient::new(access.url.clone(), access.token.clone());
    let hits = block_on(client.search(query, tags.as_deref(), Some(REGISTRY_MODEL_TYPE)))?
        .map_err(|error| format!("Registry unreachable: {error:#}"))?;
    Ok(registry_hits(hits, REGISTRY_MODEL_TYPE)
        .iter()
        .map(|hit| {
            let preview = hit
                .preview
                .clone()
                .filter(|path| !path.is_empty() && path.chars().count() <= MAX_PREVIEW_PATH_CHARS);
            ModelRow::new(
                package_row(hit),
                hit.description.as_deref(),
                &hit.tags,
                Licence::new(hit.license_type.as_deref(), hit.spdx_id.as_deref(), hit.attribution.as_deref()),
                preview,
                hit.preview_bytes,
            )
        })
        .collect())
}

/// Comma list for the search `tags` parameter; tags that would split the list are dropped.
fn search_tags(tags: &[String]) -> Option<String> {
    let kept: Vec<&str> = tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty() && !tag.contains(','))
        .take(MAX_SEARCH_TAGS)
        .collect();
    (!kept.is_empty()).then(|| kept.join(","))
}

/// The sealed header of `uri` from the registry, verified under `RequireSealed` and bound to `uri`.
async fn verified_header(
    strategy: &FetchStrategy,
    sources: &[SharedSource],
    uri: &str,
) -> Result<FetchedHeader, String> {
    strategy
        .fetch_header(uri, sources, None)
        .await
        .map(|outcome| outcome.value)
        .map_err(|failure| fetch_failure_text(&failure.error))
}

/// Registry client (header responses capped at `MODEL_HEADER_MAX_BYTES`) as a partial-fetch source, plus the verifying strategy.
fn partial_client(access: &RegistryAccess) -> (Arc<RemoteRegistryClient>, Vec<SharedSource>, FetchStrategy) {
    let client = Arc::new(
        RemoteRegistryClient::new(access.url.clone(), access.token.clone()).with_header_cap(MODEL_HEADER_MAX_BYTES),
    );
    let source: SharedSource = client.clone();
    let sources = vec![source];
    (client, sources, FetchStrategy::new(fe_hexon::p2p::P2pConfig::default()))
}

/// The verified header of `uri`, from the cache or one fetch that is cached only when it verifies.
fn cached_header(headers: &HeaderCache, access: &RegistryAccess, uri: &str) -> Result<Arc<FetchedHeader>, String> {
    headers.get_or_fetch((access.url.clone(), uri.to_owned()), || {
        let (_client, sources, strategy) = partial_client(access);
        block_on(verified_header(&strategy, &sources, uri))?
    })
}

/// The build an install must match: the header's sealing signer and its manifest content hash.
fn header_pin(header: &FetchedHeader) -> Result<InstallPin, String> {
    let signer_did = header.publisher_did();
    if signer_did.is_empty() {
        return Err(VERIFICATION_FAILED.into());
    }
    let content_hash = fe_format::integrity::content_hash(header.doc.manifest.as_bytes())
        .map_err(|_| VERIFICATION_FAILED.to_string())?;
    Ok(InstallPin { content_hash, signer_did: signer_did.to_owned() })
}

/// The downloaded build must be sealed by the pinned signer and carry the pinned content hash.
fn check_pin(pin: &InstallPin, served_signer: Option<&str>, served_content_hash: &str) -> Result<(), String> {
    let same_signer = served_signer == Some(pin.signer_did.as_str());
    let same_build = served_content_hash.eq_ignore_ascii_case(&pin.content_hash);
    if same_signer && same_build { Ok(()) } else { Err(BUILD_CHANGED.into()) }
}

/// Detail from the VERIFIED header only: name, signer, licence, entries and publisher name never come from the listing.
fn detail_from_header(header: &FetchedHeader) -> Result<ModelDetail, String> {
    if header.manifest.hexon_type != HexonType::Model {
        return Err("This package is not a model package".into());
    }
    let pin = header_pin(header)?;
    let licence = match signed_header_member(header, "license.json")? {
        Some(text) => {
            let license: License = serde_json::from_str(text)
                .map_err(|_| format!("{VERIFICATION_FAILED}: the licence is unreadable"))?;
            licence_from(Some(&license))
        }
        None => licence_from(None),
    };
    let models = signed_entries(header)?
        .iter()
        .filter(|entry| entry.kind == EntryKind::Model)
        .map(|entry| ModelEntryRow::new(&entry.entry_id, &entry.label, &entry.tags, entry.is_placeable))
        .collect();
    Ok(ModelDetail::new(
        VerifiedIdentity {
            hexon_id: &header.manifest.hexon_id,
            name: &header.manifest.name,
            signer_did: &pin.signer_did,
            content_hash: &pin.content_hash,
        },
        header.manifest.description.as_deref(),
        header.manifest.publisher_name.as_deref(),
        licence,
        models,
    ))
}

/// A header member's text, re-checked against the signed digest; `None` when the package has none.
fn signed_header_member<'a>(header: &'a FetchedHeader, path: &str) -> Result<Option<&'a str>, String> {
    let Some(text) = header.doc.header_members.get(path) else {
        return Ok(None);
    };
    let coverage = header.verified.coverage().ok_or(VERIFICATION_FAILED)?;
    coverage
        .check_member(path, text.as_bytes())
        .map_err(|_| VERIFICATION_FAILED.to_string())?;
    Ok(Some(text.as_str()))
}

/// The signed entry catalogue; a sealed package without one fails verification.
fn signed_entries(header: &FetchedHeader) -> Result<Vec<fe_hexon::manifest::AssetEntry>, String> {
    let text = signed_header_member(header, fe_format::integrity::ENTRIES_PATH)?
        .ok_or_else(|| format!("{VERIFICATION_FAILED}: the package lists no entries"))?;
    let catalog: fe_format::EntryCatalog = serde_json::from_str(text)
        .map_err(|_| format!("{VERIFICATION_FAILED}: the entry list is unreadable"))?;
    Ok(catalog.entries)
}

/// A preview member only when the verified header names it (an entry's `preview_image` or the icon) and commits its digest.
fn signed_preview_member(header: &FetchedHeader, member: &str) -> Result<(), String> {
    const NOT_SIGNED: &str = "This preview is not part of the signed package";
    let in_preview_area =
        member == fe_hexon::package::ICON_PATH || member.starts_with(fe_hexon::package::PREVIEW_PREFIX);
    if !in_preview_area {
        return Err(NOT_SIGNED.into());
    }
    let named = member == fe_hexon::package::ICON_PATH
        || signed_entries(header)?.iter().any(|entry| {
            entry.kind == EntryKind::Model && entry.preview_image.as_deref() == Some(member)
        });
    let committed = header
        .verified
        .coverage()
        .is_some_and(|coverage| coverage.committed_digest(member).is_some());
    if named && committed { Ok(()) } else { Err(NOT_SIGNED.into()) }
}

/// Verified header, then the named preview member under the thumbnail cap, digest-checked, then decoded.
fn fetch_thumbnail(
    headers: &HeaderCache,
    access: &RegistryAccess,
    uri: &str,
    member: &str,
) -> Result<fe_hexon::thumbnail::Rgba, String> {
    let header = cached_header(headers, access, uri)?;
    signed_preview_member(&header, member)?;
    let (client, _sources, _strategy) = partial_client(access);
    let bytes = block_on(client.fetch_member_capped(&header.hexon_uri(), member, THUMBNAIL_LIMIT_BYTES))?
        .map_err(|error| fetch_failure_text(&error))?;
    let coverage = header.verified.coverage().ok_or(VERIFICATION_FAILED)?;
    coverage
        .check_member(member, &bytes)
        .map_err(|_| VERIFICATION_FAILED.to_string())?;
    fe_hexon::thumbnail::decode(&bytes).map_err(|error| error.to_string())
}

/// Pin (reviewed detail, else a fresh verified header), download (seal, id, version, content hash; 64 MiB cap) with progress, require the pin, then install.
fn install_from_registry(
    library: &ModelLibrary,
    write_lock: &Mutex<()>,
    headers: &HeaderCache,
    access: &RegistryAccess,
    uri: &str,
    pin: Option<InstallPin>,
    tx: &crossbeam::channel::Sender<BridgeEvent>,
) -> Result<InstalledPackage, String> {
    let Some((wanted_id, wanted_version)) = uri.split_once('@') else {
        return Err("Not installed: the package version is missing".into());
    };
    let pin = match pin {
        Some(pin) => pin,
        None => {
            let header = cached_header(headers, access, uri)?;
            if header.manifest.hexon_type != HexonType::Model {
                return Err("This package is not a model package".into());
            }
            header_pin(&header)?
        }
    };
    // The operator-chosen registry is the trust anchor (any valid seal); unsigned content is never admitted.
    let trust = fe_hexon::registry_source::SourceTrust {
        is_trust_root: Arc::new(|_: &str| true),
        index_signer: Arc::new(|_: &str| None),
        is_killed: fe_hexon::registry_source::no_kill_list(),
        allow_unsigned_dev: false,
    };
    let source = fe_hexon::remote::remote_source("model registry", access.url.clone(), access.token.clone(), trust)
        .map_err(|error| format!("could not start the registry client: {error}"))?;
    let listing = fe_hexon::registry_source::RegistrySource::list(&source, Some(wanted_id))
        .map_err(|error| registry_error_text(&error))?;
    let entry = listing
        .entries
        .into_iter()
        .find(|entry| entry.package_id == wanted_id && entry.version == wanted_version)
        .ok_or_else(|| "Not offered by the registry".to_string())?;
    if entry.size > PACKAGE_MAX_BYTES {
        return Err(TOO_LARGE.into());
    }

    let progress = |received_bytes: u64, total_bytes: u64| {
        let result = ModelResult::InstallProgress {
            uri: uri.to_string(),
            received_bytes,
            total_bytes: (total_bytes > 0).then_some(total_bytes),
        };
        if tx.send(BridgeEvent::Progress(result)).is_err() {
            tracing::warn!(uri, "Model install progress channel closed");
        }
    };
    // Repeats and regressions are dropped, as in the map install.
    let mut last = (0u64, entry.size);
    progress(last.0, last.1);
    let fetched = source.fetch_hexon_with_progress(&entry, &mut |received, total| {
        if received < last.0 || (received, total) == last {
            return;
        }
        last = (received, total);
        progress(received, total);
    });
    let bytes = match fetched {
        Ok(fe_hexon::registry_source::FetchedHexon::Content(content)) => {
            check_pin(&pin, content.publisher_did.as_deref(), &content.content_hash).inspect_err(|_| {
                tracing::warn!(uri, "Model install refused: the downloaded build is not the reviewed one");
            })?;
            content.bytes
        }
        Ok(fe_hexon::registry_source::FetchedHexon::Plugin(_)) => {
            return Err("This package is an extension, not a model package".into());
        }
        Err(error) => return Err(registry_error_text(&error)),
    };
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        return Err(TOO_LARGE.into());
    }
    let _write = write_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    library
        .install(&bytes)
        .map(|summary| installed_package(&summary))
        .map_err(|error| library_error_text(&error))
}

/// Resolve an installed entry into a placement: placeable, with its GLB present in the shared store.
fn resolve_place(
    library: &ModelLibrary,
    blobs: &BlobStoreHandle,
    request_id: &str,
    uri: &str,
    entry_id: &str,
    petal_id: &str,
) -> Result<PlaceTarget, String> {
    let entry = library.entry(uri, entry_id).map_err(|error| library_error_text(&error))?;
    if !entry.is_placeable {
        tracing::warn!(uri, entry_id, "Place refused: the entry is not placeable");
        return Err("This model cannot be placed".into());
    }
    const MISSING: &str = "The model's file is missing from this device; reinstall the package";
    let hash = fe_runtime::blob_store::hash_from_hex(&entry.asset_hash).map_err(|_| MISSING.to_string())?;
    let size_bytes = blobs
        .get_blob_path(&hash)
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .ok_or_else(|| MISSING.to_string())?;
    Ok(PlaceTarget {
        request_id: request_id.to_string(),
        petal_id: petal_id.to_string(),
        name: node_name(&entry.label, entry_id),
        content_hash: entry.asset_hash,
        size_bytes,
    })
}

// ---- pure outcome mapping (covered by `tests::op_outcomes`) ----

/// Sanitised node name: the entry label, else the entry id.
fn node_name(label: &str, entry_id: &str) -> String {
    let name = display_text(label, MAX_NODE_NAME_CHARS);
    if name.is_empty() { display_text(entry_id, MAX_NODE_NAME_CHARS) } else { name }
}

/// Licence chip data from a signed or installed `license.json`.
fn licence_from(license: Option<&License>) -> Licence {
    let Some(license) = license else {
        return Licence::new(None, None, None);
    };
    let word = match license.license_type {
        LicenseType::Free => "free",
        LicenseType::Attribution => "attribution",
        LicenseType::Paid => "paid",
    };
    Licence::new(Some(word), license.spdx_id.as_deref(), license.attribution.as_deref())
}

/// An installed package as the panel shows it.
fn installed_package(summary: &ModelPackageSummary) -> InstalledPackage {
    InstalledPackage::new(
        summary.uri.clone(),
        summary.hexon_id.clone(),
        &summary.name,
        &summary.publisher_did,
        summary.publisher_name.as_deref(),
        summary.description.as_deref(),
        licence_from(summary.license.as_ref()),
        summary
            .models
            .iter()
            .map(|entry| ModelEntryRow::new(&entry.entry_id, &entry.label, &entry.tags, entry.is_placeable))
            .collect(),
        summary.size_bytes,
    )
}

/// Credits: each installed package carrying a used hash once; distinct used hashes with no package counted.
fn credits_from(hashes: &[String], by_hash: &HashMap<String, Vec<ModelPackageSummary>>) -> Credits {
    let distinct: HashSet<&str> = hashes.iter().map(String::as_str).collect();
    let sources_not_installed = distinct
        .iter()
        .filter(|hash| by_hash.get(**hash).is_none_or(Vec::is_empty))
        .count();
    let mut seen = HashSet::new();
    let mut packages: Vec<InstalledPackage> = by_hash
        .values()
        .flatten()
        .filter(|summary| seen.insert(summary.uri.clone()))
        .map(installed_package)
        .collect();
    packages.sort_by(|left, right| left.uri.cmp(&right.uri));
    Credits { packages, sources_not_installed }
}

/// Partial-fetch failures in panel words; every integrity failure reads "Failed verification".
fn fetch_failure_text(error: &PartialFetchError) -> String {
    match error {
        PartialFetchError::Integrity(_)
        | PartialFetchError::Substituted { .. }
        | PartialFetchError::OutOfRange => VERIFICATION_FAILED.into(),
        PartialFetchError::NotFound => "Not offered by the registry".into(),
        PartialFetchError::Timeout => "The registry did not answer in time".into(),
        PartialFetchError::Transport(reason) => format!("Registry unreachable: {reason}"),
    }
}

/// Registry download failures in panel words.
fn registry_error_text(error: &RegistryError) -> String {
    match error {
        RegistryError::Transport(reason) => format!("Registry unreachable: {reason}"),
        RegistryError::TooLarge { .. } => TOO_LARGE.into(),
        RegistryError::PackageInvalid(_)
        | RegistryError::PackageUntrusted(_)
        | RegistryError::PublisherMismatch { .. }
        | RegistryError::KindMismatch { .. }
        | RegistryError::VersionMismatch { .. }
        | RegistryError::ContentHashMismatch { .. }
        | RegistryError::Revoked { .. } => VERIFICATION_FAILED.into(),
        RegistryError::NotFound(_) | RegistryError::Yanked(..) => "Not offered by the registry".into(),
        other => format!("Install failed: {other}"),
    }
}

/// Library refusals in panel words, one per `ModelLibraryError` variant.
fn library_error_text(error: &ModelLibraryError) -> String {
    match error {
        ModelLibraryError::Unreadable(_) => format!("{VERIFICATION_FAILED}: the package could not be read"),
        ModelLibraryError::InflatesTooLarge { limit } => {
            format!("Too large to install: the package unpacks to more than {} MiB", limit / (1024 * 1024))
        }
        ModelLibraryError::InvalidModel { entry_id, .. } => {
            format!("Not installed: model {} is not a valid GLB file", display_text(entry_id, MAX_NODE_NAME_CHARS))
        }
        ModelLibraryError::ExternalModelUri(entry_id) => format!(
            "Not installed: model {} depends on files outside the package",
            display_text(entry_id, MAX_NODE_NAME_CHARS)
        ),
        ModelLibraryError::Unsigned => format!("{VERIFICATION_FAILED}: the package is not signed by its publisher"),
        ModelLibraryError::NotModel(_) => "This package is not a model package".into(),
        ModelLibraryError::LicenseMissing => "Not installed: the package has no licence".into(),
        ModelLibraryError::LicensePaid => "Not supported in this build: paid licence".into(),
        ModelLibraryError::AttributionMissing => {
            "Not installed: the attribution licence names no attribution".into()
        }
        ModelLibraryError::UnsafeName(_) => {
            "Not installed: the package id or version is not a safe file name".into()
        }
        ModelLibraryError::NoModels => "Not installed: the package contains no models".into(),
        ModelLibraryError::MissingAsset(entry_id) => {
            format!("{VERIFICATION_FAILED}: model {entry_id} has no file in the package")
        }
        ModelLibraryError::AlreadyInstalled(_) => "Already installed".into(),
        ModelLibraryError::NotInstalled(_) => "Not installed on this device".into(),
        ModelLibraryError::EntryNotFound { .. } => "The package has no such model".into(),
        ModelLibraryError::Blobs(reason) => format!("Could not store the model files: {reason}"),
        ModelLibraryError::Io(error) => format!("Library file error: {error}"),
    }
}

/// The failure result an op answers with when it cannot start.
fn refusal_result(op: &ModelOp, reason: &str) -> ModelResult {
    let error = reason.to_string();
    match op {
        ModelOp::List { generation, .. } => ModelResult::Listed { generation: *generation, result: Err(error) },
        ModelOp::ListInstalled => ModelResult::InstalledListed(Err(error)),
        ModelOp::Detail { uri } => ModelResult::Detail { uri: uri.clone(), result: Err(error) },
        ModelOp::Thumbnail { uri, .. } => ModelResult::Thumbnail { uri: uri.clone(), result: Err(error) },
        ModelOp::Install { uri, .. } => ModelResult::Installed { uri: uri.clone(), result: Err(error) },
        ModelOp::Remove { uri } => ModelResult::Removed { uri: uri.clone(), result: Err(error) },
        ModelOp::Place { request_id, .. } => ModelResult::PlaceFailed { request_id: request_id.clone(), error },
        ModelOp::LoadCredits { petal_id } => ModelResult::Credits { petal_id: petal_id.clone(), result: Err(error) },
    }
}

/// The placement command for the active petal, or `PlaceFailed` when navigation moved or is incomplete.
fn place_command(
    target: PlaceTarget,
    verse_id: Option<&str>,
    fractal_id: Option<&str>,
    active_petal_id: Option<&str>,
    position: [f32; 3],
) -> Result<DbCommand, ModelResult> {
    let fail = |error: &str| ModelResult::PlaceFailed {
        request_id: target.request_id.clone(),
        error: error.to_string(),
    };
    if active_petal_id != Some(target.petal_id.as_str()) {
        return Err(fail("The active petal changed before the model was placed"));
    }
    let (Some(verse_id), Some(fractal_id)) = (verse_id, fractal_id) else {
        return Err(fail("Open a petal to place a model"));
    };
    Ok(DbCommand::PlaceStoredGlb {
        request: Some(NativeImportRequest {
            request_id: target.request_id,
            verse_id: verse_id.to_string(),
            fractal_id: fractal_id.to_string(),
            petal_id: target.petal_id.clone(),
        }),
        petal_id: target.petal_id,
        name: target.name,
        content_hash: target.content_hash,
        size_bytes: target.size_bytes,
        position,
    })
}

/// `PlaceFailed` for a write the door refused or could not take; `None` once it is sent or queued.
fn place_send_outcome(request_id: &str, sent: GuiSend) -> Option<ModelResult> {
    let error = match sent {
        GuiSend::Sent | GuiSend::Waiting => return None,
        GuiSend::Refused(reason) => reason,
        GuiSend::Busy => "Database busy; the model was not placed".to_string(),
        GuiSend::Closed => "Not connected to the database; the model was not placed".to_string(),
    };
    Some(ModelResult::PlaceFailed { request_id: request_id.to_string(), error })
}

/// The camera's orbit focus, else the last focus target, else the petal origin; non-finite points are skipped.
fn spawn_position(orbit_focus: Option<[f32; 3]>, focus_target: Option<[f32; 3]>) -> [f32; 3] {
    [orbit_focus, focus_target]
        .into_iter()
        .flatten()
        .find(|point| point.iter().all(|value| value.is_finite()))
        .unwrap_or([0.0; 3])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(uri: &str) -> ModelPackageSummary {
        let (hexon_id, version) = uri.split_once('@').expect("versioned uri");
        ModelPackageSummary {
            uri: uri.into(),
            hexon_id: hexon_id.into(),
            version: version.into(),
            name: "Chairs".into(),
            description: None,
            publisher_did: "did:key:z6Mk".into(),
            publisher_name: None,
            tags: Vec::new(),
            license: Some(License {
                license_type: LicenseType::Attribution,
                attribution: Some("Model by A. Maker".into()),
                ..License::default()
            }),
            models: Vec::new(),
            size_bytes: 2048,
        }
    }

    fn target(petal_id: &str) -> PlaceTarget {
        PlaceTarget {
            request_id: "model-library-1".into(),
            petal_id: petal_id.into(),
            name: "Chair".into(),
            content_hash: "ab".repeat(32),
            size_bytes: 512,
        }
    }

    /// One line per result so the table compares text.
    fn describe(result: &ModelResult) -> String {
        match result {
            ModelResult::Listed { generation, result } => format!("listed {generation} {:?}", result.as_ref().err()),
            ModelResult::InstalledListed(result) => format!("installed-listed {:?}", result.as_ref().err()),
            ModelResult::Detail { uri, result } => format!("detail {uri} {:?}", result.as_ref().err()),
            ModelResult::Thumbnail { uri, result } => format!("thumbnail {uri} {:?}", result.as_ref().err()),
            ModelResult::InstallProgress { uri, .. } => format!("progress {uri}"),
            ModelResult::Installed { uri, result } => format!("installed {uri} {:?}", result.as_ref().err()),
            ModelResult::Removed { uri, result } => format!("removed {uri} {:?}", result.as_ref().err()),
            ModelResult::Placed { request_id } => format!("placed {request_id}"),
            ModelResult::PlaceFailed { request_id, error } => format!("place-failed {request_id} {error}"),
            ModelResult::Credits { petal_id, result } => match result {
                Ok(credits) => format!(
                    "credits {petal_id} {} packages, {} not installed",
                    credits.packages.len(),
                    credits.sources_not_installed
                ),
                Err(error) => format!("credits {petal_id} error {error}"),
            },
        }
    }

    fn describe_command(command: Result<DbCommand, ModelResult>) -> String {
        match command {
            Ok(DbCommand::PlaceStoredGlb { request: Some(request), petal_id, name, size_bytes, position, .. }) => {
                format!(
                    "place {} in {}/{}/{petal_id} as {name} ({size_bytes} B) at {position:?}",
                    request.request_id, request.verse_id, request.fractal_id
                )
            }
            Ok(_) => "unexpected command".into(),
            Err(result) => describe(&result),
        }
    }

    #[test]
    fn op_outcomes() {
        let mut by_hash = HashMap::new();
        by_hash.insert("a".repeat(64), vec![summary("chairs@1.0.0")]);
        by_hash.insert("b".repeat(64), vec![summary("chairs@1.0.0")]);
        let used = vec!["a".repeat(64), "b".repeat(64), "a".repeat(64), "c".repeat(64)];
        let credits = credits_from(&used, &by_hash);
        let installed = installed_package(&summary("chairs@1.0.0"));
        let place = |petal: Option<&str>, verse: Option<&str>| {
            describe_command(place_command(target("p1"), verse, Some("f1"), petal, [1.0, 2.0, 3.0]))
        };
        let reviewed = InstallPin { content_hash: "ab".repeat(32), signer_did: "did:key:z6MkReviewed".into() };
        let pin_outcome = |signer: Option<&str>, hash: &str| check_pin(&reviewed, signer, hash).err().unwrap_or_else(|| "ok".into());
        // Header cache: a failed fetch is never stored (the next op fetches again); slots stay bounded.
        let headers = HeaderCache::default();
        let mut fetches = 0;
        for index in 0..(HEADER_CACHE_CAPACITY + 8) {
            let uri = if index < 2 { "chairs@1.0.0".to_string() } else { format!("pkg{index}@1.0.0") };
            let _ = headers.get_or_fetch(("https://registry".into(), uri), || {
                fetches += 1;
                Err("offline".into())
            });
        }
        let cached_slots = headers.slots.lock().unwrap().len();

        let cases: Vec<(&str, String, String)> = vec![
            // Detail / thumbnail fetch failures: every integrity failure reads the same (U2).
            ("tampered header", fetch_failure_text(&PartialFetchError::Integrity(fe_format::IntegrityError::NotSealed)), VERIFICATION_FAILED.into()),
            (
                "substituted header",
                fetch_failure_text(&PartialFetchError::Substituted { requested: "a@1.0.0".into(), signed: "b@1.0.0".into() }),
                VERIFICATION_FAILED.into(),
            ),
            ("registry offline", fetch_failure_text(&PartialFetchError::Transport("refused".into())), "Registry unreachable: refused".into()),
            ("registry slow", fetch_failure_text(&PartialFetchError::Timeout), "The registry did not answer in time".into()),
            // Install download failures.
            (
                "content hash mismatch",
                registry_error_text(&RegistryError::ContentHashMismatch { listed: "x".into(), served: "y".into() }),
                VERIFICATION_FAILED.into(),
            ),
            ("over the cap", registry_error_text(&RegistryError::TooLarge { what: "package".into(), max: 1 }), TOO_LARGE.into()),
            ("not listed", registry_error_text(&RegistryError::NotFound("chairs".into())), "Not offered by the registry".into()),
            // Library refusals.
            (
                "unsigned package",
                library_error_text(&ModelLibraryError::Unsigned),
                format!("{VERIFICATION_FAILED}: the package is not signed by its publisher"),
            ),
            ("paid licence", library_error_text(&ModelLibraryError::LicensePaid), "Not supported in this build: paid licence".into()),
            (
                "inflates past the library budget",
                library_error_text(&ModelLibraryError::InflatesTooLarge { limit: 256 * 1024 * 1024 }),
                "Too large to install: the package unpacks to more than 256 MiB".into(),
            ),
            (
                "malformed glb",
                library_error_text(&ModelLibraryError::InvalidModel { entry_id: "chair\u{202E}".into(), reason: "x".into() }),
                "Not installed: model chair is not a valid GLB file".into(),
            ),
            // Install pin: the download must be the reviewed build (signer and content hash), else nothing is stored.
            ("pin: the reviewed build", pin_outcome(Some("did:key:z6MkReviewed"), &"AB".repeat(32)), "ok".into()),
            ("pin: another build", pin_outcome(Some("did:key:z6MkReviewed"), &"cd".repeat(32)), BUILD_CHANGED.into()),
            ("pin: another signer", pin_outcome(Some("did:key:z6MkOther"), &"ab".repeat(32)), BUILD_CHANGED.into()),
            ("pin: unsigned download", pin_outcome(None, &"ab".repeat(32)), BUILD_CHANGED.into()),
            ("failed header fetches are retried, never cached", fetches.to_string(), (HEADER_CACHE_CAPACITY + 8).to_string()),
            ("header cache bounded", cached_slots.to_string(), HEADER_CACHE_CAPACITY.to_string()),
            ("already installed", library_error_text(&ModelLibraryError::AlreadyInstalled("chairs@1.0.0".into())), "Already installed".into()),
            // Placement: navigation must still be on the requested petal.
            ("place in the active petal", place(Some("p1"), Some("v1")), "place model-library-1 in v1/f1/p1 as Chair (512 B) at [1.0, 2.0, 3.0]".into()),
            (
                "petal changed before placing",
                place(Some("p2"), Some("v1")),
                "place-failed model-library-1 The active petal changed before the model was placed".into(),
            ),
            ("no verse open", place(Some("p1"), None), "place-failed model-library-1 Open a petal to place a model".into()),
            // Door outcomes.
            ("door took the write", format!("{:?}", place_send_outcome("r1", GuiSend::Sent).map(|r| describe(&r))), "None".into()),
            ("write queued", format!("{:?}", place_send_outcome("r1", GuiSend::Waiting).map(|r| describe(&r))), "None".into()),
            (
                "hook refused",
                describe(&place_send_outcome("r1", GuiSend::Refused("Blocked by an extension".into())).expect("refusal")),
                "place-failed r1 Blocked by an extension".into(),
            ),
            (
                "channel full",
                describe(&place_send_outcome("r1", GuiSend::Busy).expect("busy")),
                "place-failed r1 Database busy; the model was not placed".into(),
            ),
            // Credits: one package for two hashes; one distinct hash with no source.
            ("credits join", describe(&ModelResult::Credits { petal_id: "p1".into(), result: Ok(credits) }), "credits p1 1 packages, 1 not installed".into()),
            ("installed licence", installed.licence.attribution.clone().unwrap_or_default(), "Model by A. Maker".into()),
            // Ops that cannot start answer with their own failure shape.
            (
                "refused listing keeps its generation",
                describe(&refusal_result(&ModelOp::List { generation: 7, query: String::new(), tags: Vec::new() }, "offline")),
                "listed 7 Some(\"offline\")".into(),
            ),
            (
                "refused placement",
                describe(&refusal_result(
                    &ModelOp::Place { request_id: "r2".into(), uri: "a@1.0.0".into(), entry_id: "e".into(), petal_id: "p".into() },
                    "unavailable",
                )),
                "place-failed r2 unavailable".into(),
            ),
            // Spawn point: orbit focus, else focus target, else origin; non-finite skipped.
            ("orbit focus wins", format!("{:?}", spawn_position(Some([1.0, 0.0, 2.0]), Some([9.0; 3]))), "[1.0, 0.0, 2.0]".into()),
            ("non-finite orbit falls back", format!("{:?}", spawn_position(Some([f32::NAN, 0.0, 0.0]), Some([9.0; 3]))), "[9.0, 9.0, 9.0]".into()),
            ("nothing focused", format!("{:?}", spawn_position(None, None)), "[0.0, 0.0, 0.0]".into()),
            // Search tags and node names.
            ("search tags", format!("{:?}", search_tags(&["chair".into(), " ".into(), "a,b".into(), "wood".into()])), "Some(\"chair,wood\")".into()),
            ("blank label uses the entry id", node_name("  ", "chair_01"), "chair_01".into()),
        ];
        for (case, got, want) in cases {
            assert_eq!(got, want, "{case}");
        }
    }
}
