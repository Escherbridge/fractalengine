//! Mount table, bundle resolution and the open/close/revoke lifecycle (protocol.md §2, admission.md §3-§5); see `portal_bundle/AGENTS.md` §mounts, §admission.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use fe_format::integrity::IntegrityStatus;
use fe_format::portal_bundle::{
    validate_portal_bundle, BundleFacts, BundleMember, PortalSection, RefusalClass,
    DEFAULT_MAX_BUNDLE_BYTES, EXTENSION_MANIFEST_PATH,
};
use fe_format::HexonArchiveIndex;

use super::admission::{self, AdmissionSignal, GovernanceView, KillListStatus, MountedBuild, Rejudge};
use super::feeds::{self, FeedSource};
use super::ipc::PortalBridge;
use super::navigation::{normalize_origin, GateNotice, NavigationGate, UNLOAD_URL};
use super::origin::{admitted_label, static_label, Label};
use crate::ipc::{BlockedReason, BrowserEvent, BundleState, RevokedReason};

/// Env var overriding the per-node bundle size cap in bytes (README §8.4, OD-4; same name fe-api reads).
pub const BUNDLE_MAX_BYTES_ENV: &str = "FE_PORTAL_BUNDLE_MAX_BYTES";

/// Process-wide mount counter; ids never repeat.
static NEXT_MOUNT_ID: AtomicU64 = AtomicU64::new(1);

/// Per-node bundle size cap: [`BUNDLE_MAX_BYTES_ENV`] or the 32 MiB default.
pub fn bundle_max_bytes() -> u64 {
    std::env::var(BUNDLE_MAX_BYTES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|cap| *cap > 0)
        .unwrap_or(DEFAULT_MAX_BUNDLE_BYTES)
}

// ---------------------------------------------------------------------------
// Resolution seam (binary-injected; governance-backed when it returns a view)
// ---------------------------------------------------------------------------

/// Where a repository node's bundle lives on this peer.
#[derive(Debug, Clone)]
pub struct BundleSource {
    pub asset_id: String,
    /// Local archive path; `None` while the blob has not arrived (Waiting).
    pub blob_path: Option<PathBuf>,
    /// Set when governance refuses this build before any byte is read (admission.md §2 rules 3-5).
    pub revoked: Option<RevokedReason>,
}

/// Identity of one opened build, as governance keys it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleIdentity {
    pub package_id: String,
    /// Lowercase-hex BLAKE3 (README §2.3).
    pub content_hash: String,
    /// BLAKE3 of the whole archive (the blob hash); equals `content_hash` when unsealed.
    pub archive_digest: String,
    /// The verified sealing DID; `None` for an unsealed build.
    pub publisher_did: Option<String>,
    pub capability_digest: String,
}

impl BundleIdentity {
    /// The digests a kill-list entry may name for this build, under any package id.
    pub fn digests(&self) -> [&str; 2] {
        [self.content_hash.as_str(), self.archive_digest.as_str()]
    }
}

/// Governance's answer for one build on this peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionVerdict {
    /// No admission record: static sandbox.
    Unadmitted { can_admit: bool },
    /// An admission record for this exact pair, by this publisher.
    Admitted { publisher_did: String },
    Revoked(RevokedReason),
    /// A verse-policy ceiling (admission.md §2 rule 10).
    Blocked(BlockedReason),
}

/// Resolves repository nodes to local bundle bytes; injected by the binary, called on a worker thread.
pub trait BundleResolver: Send + Sync + 'static {
    /// Resolve `node_id`'s `portal.bundle_asset`; `Err` carries the state to show (Waiting, Blocked, Revoked).
    fn resolve(&self, node_id: &str) -> Result<BundleSource, BundleState>;

    /// Verdict for an opened, validated build when [`governance`](Self::governance) is `None`; admits nothing by default.
    fn verdict(&self, _identity: &BundleIdentity) -> AdmissionVerdict {
        AdmissionVerdict::Unadmitted { can_admit: false }
    }

    /// Governance's synchronous view (A-15); when present it decides admission, at open and on every re-judge.
    fn governance(&self) -> Option<&dyn GovernanceView> {
        None
    }

    /// The node's static feeds and this peer's fe-api origin (admission.md §7); none by default.
    fn feeds(&self, _node_id: &str) -> FeedSource {
        FeedSource::default()
    }
}

// ---------------------------------------------------------------------------
// Mounts
// ---------------------------------------------------------------------------

/// Whether a mount gets the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    Admitted,
    Static,
}

/// One mounted bundle build: immutable once installed, except its fault flag.
#[derive(Debug)]
pub struct Mount {
    pub mount_id: u64,
    pub label: Label,
    pub mode: MountMode,
    pub can_admit: bool,
    pub node_id: String,
    pub asset_id: String,
    pub package_id: String,
    pub content_hash: String,
    /// BLAKE3 of the whole archive: a kill-list key beside `content_hash`.
    pub archive_digest: String,
    pub publisher_did: Option<String>,
    pub capability_digest: String,
    pub portal: PortalSection,
    /// The document URL the host loads to open this mount.
    pub entry_url: String,
    pub(crate) archive: HexonArchiveIndex,
    pub(crate) csp: http::HeaderValue,
    /// Served `web/` member → Content-Type.
    pub(crate) members: HashMap<String, &'static str>,
    pub(crate) navigation_origins: Vec<String>,
    pub(crate) frame_origins: Vec<String>,
    /// The `/__fe/feeds.json` body; `None` on an Admitted mount, whose data path is IPC.
    pub(crate) feeds_document: Option<Vec<u8>>,
    faulted: AtomicBool,
}

impl Mount {
    /// The chrome state this mount renders in.
    pub fn state(&self) -> BundleState {
        match (self.mode, &self.publisher_did) {
            (MountMode::Admitted, Some(publisher)) => BundleState::Admitted {
                publisher: publisher.clone(),
            },
            _ => BundleState::StaticSandbox {
                can_admit: self.can_admit,
            },
        }
    }

    /// True once a member failed its integrity check; the mount serves nothing more.
    pub fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::SeqCst)
    }

    /// The composed Content-Security-Policy header value.
    pub fn csp(&self) -> &http::HeaderValue {
        &self.csp
    }

    /// The `/__fe/feeds.json` body; `None` on an Admitted mount.
    pub fn feeds_document(&self) -> Option<&[u8]> {
        self.feeds_document.as_deref()
    }

    /// Governance's key for this build.
    pub fn identity(&self) -> BundleIdentity {
        BundleIdentity {
            package_id: self.package_id.clone(),
            content_hash: self.content_hash.clone(),
            archive_digest: self.archive_digest.clone(),
            publisher_did: self.publisher_did.clone(),
            capability_digest: self.capability_digest.clone(),
        }
    }

    /// What `host/admission` events are matched against.
    pub fn build(&self) -> MountedBuild<'_> {
        MountedBuild {
            package_id: &self.package_id,
            content_hash: &self.content_hash,
            archive_digest: &self.archive_digest,
            mode: self.mode,
        }
    }

    /// Origins whose storage a revocation clears: this mount's label, plus the build's static label when admitted.
    pub fn storage_labels(&self) -> Vec<Label> {
        let mut labels = vec![self.label.clone()];
        if let Some(static_origin) = static_label(&self.content_hash).filter(|label| *label != self.label) {
            labels.push(static_origin);
        }
        labels
    }

    fn pair(&self) -> (String, String) {
        (self.package_id.clone(), self.content_hash.clone())
    }
}

#[derive(Debug, Default)]
struct Table {
    /// One webview, one live mount (A-7).
    live: Option<Arc<Mount>>,
    /// Document generation: bumps on every admitted top-frame navigation and every unmount (ipc-v1.md §1.4).
    generation: u64,
}

#[derive(Debug, Default)]
struct Shared {
    table: RwLock<Table>,
    faults: Mutex<Vec<u64>>,
}

/// The shared mount table: protocol workers read it, only the main thread writes it.
#[derive(Debug, Clone, Default)]
pub struct PortalMounts {
    shared: Arc<Shared>,
}

impl PortalMounts {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> RwLockReadGuard<'_, Table> {
        self.shared.table.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Table> {
        self.shared.table.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Make `mount` the live mount (main thread).
    pub fn install(&self, mount: Mount) -> Arc<Mount> {
        let mount = Arc::new(mount);
        self.write().live = Some(Arc::clone(&mount));
        mount
    }

    /// Remove the live mount, then bump the generation (protocol.md §2.5 steps 1-2).
    pub fn unmount(&self) -> Option<Arc<Mount>> {
        let mut table = self.write();
        let removed = table.live.take();
        if removed.is_some() {
            table.generation = table.generation.wrapping_add(1);
        }
        removed
    }

    /// [`unmount`](Self::unmount) only when `mount_id` is still the live mount.
    pub fn unmount_if(&self, mount_id: u64) -> Option<Arc<Mount>> {
        let mut table = self.write();
        if table.live.as_ref().is_some_and(|live| live.mount_id == mount_id) {
            table.generation = table.generation.wrapping_add(1);
            return table.live.take();
        }
        None
    }

    /// The live mount, faulted or not.
    pub fn current(&self) -> Option<Arc<Mount>> {
        self.read().live.clone()
    }

    /// The live, unfaulted mount for `label`.
    pub fn lookup(&self, label: &Label) -> Option<Arc<Mount>> {
        self.read()
            .live
            .as_ref()
            .filter(|mount| &mount.label == label && !mount.is_faulted())
            .cloned()
    }

    /// True while `mount_id` is live and unfaulted (protocol.md §3.2 step 9).
    pub fn is_live(&self, mount_id: u64) -> bool {
        self.read()
            .live
            .as_ref()
            .is_some_and(|mount| mount.mount_id == mount_id && !mount.is_faulted())
    }

    /// Current document generation.
    pub fn generation(&self) -> u64 {
        self.read().generation
    }

    /// A new top-frame document (or none) replaced the last one.
    pub fn bump_generation(&self) {
        let mut table = self.write();
        table.generation = table.generation.wrapping_add(1);
    }

    /// A worker saw a tampered or changed member: stop serving now, Block on the next frame.
    pub(crate) fn report_fault(&self, mount: &Mount) {
        mount.faulted.store(true, Ordering::SeqCst);
        self.shared
            .faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mount.mount_id);
    }

    /// Mount ids reported faulted since the last drain.
    pub fn drain_faults(&self) -> Vec<u64> {
        std::mem::take(&mut *self.shared.faults.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

// ---------------------------------------------------------------------------
// Preparation (worker thread): resolve, open, validate, identify, decide the mode
// ---------------------------------------------------------------------------

fn blocked(reason: BlockedReason) -> BundleState {
    BundleState::Blocked(reason)
}

/// README §6 refusal class → chrome reason (admission.md §2 rules 7-9).
pub fn blocked_reason(class: RefusalClass) -> BlockedReason {
    match class {
        RefusalClass::TooLarge | RefusalClass::ArchiveLimit => BlockedReason::TooLarge,
        RefusalClass::Integrity => BlockedReason::Integrity,
        _ => BlockedReason::MalformedManifest,
    }
}

/// Resolve and open `node_id`'s bundle into a mount, or the state to show instead (admission.md §2 order).
pub fn prepare(resolver: &dyn BundleResolver, node_id: &str, max_bytes: u64) -> Result<Mount, BundleState> {
    let source = resolver.resolve(node_id)?;
    if let Some(reason) = source.revoked {
        return Err(BundleState::Revoked(reason));
    }
    let governance = resolver.governance();
    // Rule 3 before any byte: a host-wide disable renders nothing, the static sandbox included.
    if governance.is_some_and(|view| view.host_disabled()) {
        return Err(BundleState::Revoked(RevokedReason::KillSwitch));
    }
    let Some(path) = source.blob_path.as_deref() else {
        return Err(BundleState::WaitingForBytes);
    };
    let archive_bytes = std::fs::metadata(path)
        .map_err(|_| BundleState::WaitingForBytes)?
        .len();
    if archive_bytes > max_bytes {
        return Err(blocked(BlockedReason::TooLarge));
    }
    // Rule 4 before any byte is parsed: a list entry may name the archive itself, under any package id.
    let archive_digest = file_blake3_hex(path).map_err(|_| blocked(BlockedReason::Integrity))?;
    if let Some(view) = governance {
        if admission::kill_list_status(view, &[archive_digest.as_str()]) == KillListStatus::Listed {
            return Err(BundleState::Revoked(RevokedReason::RevocationList));
        }
    }
    let archive = HexonArchiveIndex::open(path).map_err(|error| {
        tracing::warn!(node_id, "portal bundle failed header verification: {error:#}");
        blocked(BlockedReason::Integrity)
    })?;
    let members = central_directory(path).map_err(|error| {
        tracing::warn!(node_id, "portal bundle directory unreadable: {error:#}");
        blocked(BlockedReason::Integrity)
    })?;
    let publisher_did = match archive.integrity_status() {
        IntegrityStatus::Sealed { publisher_did } => Some(publisher_did),
        IntegrityStatus::Unsealed => None,
    };
    let extension_json = archive
        .read_member(EXTENSION_MANIFEST_PATH)
        .map_err(|error| blocked(member_read_reason(&error)))?;
    let manifest = archive.manifest();
    let facts = BundleFacts {
        archive_bytes,
        max_bytes,
        members: &members,
        integrity_failure: None,
        extension_json: extension_json.as_deref(),
        hexon_id: &manifest.hexon_id,
        hexon_version: &manifest.version,
    };
    let validated = validate_portal_bundle(&facts, &origin_allowed).map_err(|refusal| {
        tracing::warn!(node_id, "portal bundle refused: {refusal}");
        blocked(blocked_reason(refusal.class))
    })?;
    let content_hash = match publisher_did {
        Some(_) => fe_format::integrity::content_hash(archive.header_doc().manifest.as_bytes())
            .map_err(|_| blocked(BlockedReason::Integrity))?,
        None => archive_digest.clone(),
    };
    let identity = BundleIdentity {
        package_id: validated.package_id.clone(),
        content_hash: content_hash.clone(),
        archive_digest: archive_digest.clone(),
        publisher_did: publisher_did.clone(),
        capability_digest: validated.capability_digest.clone(),
    };
    // Admitted needs the exact pair, a seal by the admitting publisher and consent for these capabilities (rule 11).
    let verdict = match governance {
        Some(view) => admission::verdict(view, node_id, &identity),
        None => resolver.verdict(&identity),
    };
    let (mode, can_admit) = admission::mount_mode(&verdict, publisher_did.as_deref())?;
    let label = match (mode, &publisher_did) {
        (MountMode::Admitted, Some(publisher)) => admitted_label(publisher, &validated.package_id),
        _ => static_label(&content_hash).ok_or_else(|| blocked(BlockedReason::Integrity))?,
    };
    // Only a Static mount reads feeds; its `connect-src` gains exactly the kept feed origins.
    let kept_feeds = (mode == MountMode::Static).then(|| feeds::admit(&resolver.feeds(node_id)));
    let feed_origins = kept_feeds.as_ref().map_or(&[][..], |kept| kept.origins.as_slice());
    let policy = super::csp::compose(&validated.portal.csp, mode, feed_origins)
        .map_err(|_| blocked(BlockedReason::MalformedManifest))?;
    let csp = http::HeaderValue::from_str(&policy).map_err(|_| blocked(BlockedReason::MalformedManifest))?;
    let entry_url = label
        .document_url(&validated.portal.entry)
        .ok_or_else(|| blocked(BlockedReason::MalformedManifest))?;
    let navigation_origins = validated.portal.navigation.iter().filter_map(|o| normalize_origin(o)).collect();
    let frame_origins = validated.portal.csp.frame.iter().filter_map(|o| normalize_origin(o)).collect();
    Ok(Mount {
        mount_id: NEXT_MOUNT_ID.fetch_add(1, Ordering::Relaxed),
        label,
        mode,
        can_admit,
        node_id: node_id.to_owned(),
        asset_id: source.asset_id,
        package_id: validated.package_id,
        content_hash,
        archive_digest,
        publisher_did,
        capability_digest: validated.capability_digest,
        entry_url,
        members: validated.web_members.into_iter().collect(),
        portal: validated.portal,
        archive,
        csp,
        navigation_origins,
        frame_origins,
        feeds_document: kept_feeds.map(|kept| kept.document),
        faulted: AtomicBool::new(false),
    })
}

/// The raw URL policy over a declared origin (README §5; never the QA-widened `embedded_allowed`).
fn origin_allowed(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|url| fe_runtime::portal_url::is_url_allowed(&url))
}

/// Every central-directory entry with its declared uncompressed size; no member is inflated.
fn central_directory(path: &Path) -> anyhow::Result<Vec<BundleMember>> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut zip = zip::ZipArchive::new(file)?;
    let mut members = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index)?;
        members.push(BundleMember {
            path: entry.name().to_owned(),
            uncompressed_size: entry.size(),
        });
    }
    Ok(members)
}

/// A lazy-read failure: a cap breach is "too large", anything else an integrity failure.
fn member_read_reason(error: &anyhow::Error) -> BlockedReason {
    if format!("{error:#}").to_ascii_lowercase().contains("limit") {
        BlockedReason::TooLarge
    } else {
        BlockedReason::Integrity
    }
}

/// BLAKE3 hex of a file, streamed (the unsealed `content_hash`, README §2.3).
fn file_blake3_hex(path: &Path) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut file = std::fs::File::open(path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

// ---------------------------------------------------------------------------
// Lifecycle (main thread)
// ---------------------------------------------------------------------------

/// What a backend shares with the bundle host: the table, the guard state and platform support.
#[derive(Debug, Clone)]
pub struct BundleParts {
    pub mounts: PortalMounts,
    pub gate: Rc<NavigationGate>,
    /// Windows with `ICoreWebView2_22` (OD-10, spike P-1); otherwise every Open is Blocked.
    pub supported: bool,
}

/// The webview operations the bundle lifecycle needs; backends without bundles keep the defaults.
pub trait BundleWebview {
    /// Shared pieces for a [`BundleHost`]; `None` when the backend has no embedded browser.
    fn bundle_parts(&self) -> Option<BundleParts> {
        None
    }
    /// Load a host-minted URL (a bundle entry or [`UNLOAD_URL`]); the gate admits it by token.
    fn bundle_load(&mut self, _url: &str) -> anyhow::Result<()> {
        anyhow::bail!("this backend cannot render portal bundles")
    }
    /// Show the portal surface.
    fn bundle_show(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("this backend cannot render portal bundles")
    }
    /// Hide the portal surface.
    fn bundle_hide(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("this backend cannot render portal bundles")
    }
    /// Queue a storage clear for `label`'s origin, run once a document of another origin commits (SP-6); a no-op by default.
    fn bundle_clear_storage(&mut self, _label: &Label) {}
}

struct OpenOutcome {
    request: u64,
    node_id: String,
    result: Result<Mount, BundleState>,
}

/// The explicit local Open in force (admission.md §3: only an Open ever mounts).
struct Shown {
    node_id: String,
    /// `(package_id, content_hash)` of the build it last mounted, or refused as revoked at install.
    pair: Option<(String, String)>,
    /// A revocation unloaded it: only consent for `pair`, or a new Open, renders again (ux.md §4.4).
    revoked: bool,
}

/// Open/close/leave/revoke orchestration over one webview; resolution runs off the main thread.
pub struct BundleHost {
    mounts: PortalMounts,
    gate: Rc<NavigationGate>,
    supported: bool,
    latest_request: u64,
    /// Node of the in-flight open (request = `latest_request`).
    pending: Option<String>,
    outcomes_tx: mpsc::Sender<OpenOutcome>,
    outcomes_rx: mpsc::Receiver<OpenOutcome>,
    /// The latest Open's resolver: its governance view re-judges at install, on signals and on refresh.
    resolver: Option<Arc<dyn BundleResolver>>,
    shown: Option<Shown>,
    /// Signals that arrived while an open was in flight; checked before its mount is installed.
    signals_since_open: Vec<AdmissionSignal>,
}

/// One `BundleState` event.
fn state_event(node_id: String, label: Option<&Label>, state: BundleState) -> BrowserEvent {
    BrowserEvent::BundleState {
        node_id,
        label: label.map(|label| label.as_str().to_owned()),
        state,
    }
}

impl BundleHost {
    /// A host over a backend's shared parts.
    pub fn new(parts: BundleParts) -> Self {
        let (outcomes_tx, outcomes_rx) = mpsc::channel();
        Self {
            mounts: parts.mounts,
            gate: parts.gate,
            supported: parts.supported,
            latest_request: 0,
            pending: None,
            outcomes_tx,
            outcomes_rx,
            resolver: None,
            shown: None,
            signals_since_open: Vec::new(),
        }
    }

    /// True while a bundle is mounted or an open is in flight.
    pub fn is_active(&self) -> bool {
        self.pending.is_some() || self.mounts.current().is_some()
    }

    /// The shared mount table.
    pub fn mounts(&self) -> &PortalMounts {
        &self.mounts
    }

    /// Explicit local Open: closes any open bundle, then resolves `node_id` on a worker thread.
    pub fn open(
        &mut self,
        node_id: &str,
        resolver: Option<Arc<dyn BundleResolver>>,
        webview: &mut dyn BundleWebview,
    ) -> Vec<BrowserEvent> {
        let mut events = self.close(webview);
        self.resolver = resolver;
        self.shown = Some(Shown {
            node_id: node_id.to_owned(),
            pair: None,
            revoked: false,
        });
        events.extend(self.start(node_id));
        events
    }

    /// Resolve `node_id` with the stored resolver on a worker thread; tears nothing down.
    fn start(&mut self, node_id: &str) -> Vec<BrowserEvent> {
        self.signals_since_open.clear();
        if !self.supported {
            return vec![state_event(node_id.to_owned(), None, blocked(BlockedReason::UnsupportedPlatform))];
        }
        let Some(resolver) = self.resolver.clone() else {
            return vec![state_event(node_id.to_owned(), None, BundleState::Unavailable)];
        };
        self.latest_request = self.latest_request.wrapping_add(1);
        let request = self.latest_request;
        let tx = self.outcomes_tx.clone();
        let node = node_id.to_owned();
        let max_bytes = bundle_max_bytes();
        let spawned = std::thread::Builder::new()
            .name("fe-portal-open".into())
            .spawn(move || {
                let result = prepare(resolver.as_ref(), &node, max_bytes);
                let _ = tx.send(OpenOutcome { request, node_id: node, result });
            });
        match spawned {
            Ok(_) => {
                self.pending = Some(node_id.to_owned());
                Vec::new()
            }
            Err(error) => {
                tracing::error!("portal bundle open worker failed to start: {error}");
                vec![state_event(node_id.to_owned(), None, BundleState::Unavailable)]
            }
        }
    }

    /// Close: void any pending open, then unmount, unload and hide (protocol.md §2.5).
    pub fn close(&mut self, webview: &mut dyn BundleWebview) -> Vec<BrowserEvent> {
        let mut events = Vec::new();
        self.latest_request = self.latest_request.wrapping_add(1);
        self.shown = None;
        if let Some(node_id) = self.pending.take() {
            events.push(state_event(node_id, None, BundleState::Closed));
        }
        if let Some(mount) = self.teardown(webview) {
            events.push(state_event(mount.node_id.clone(), Some(&mount.label), BundleState::Closed));
        }
        events
    }

    /// A host `Navigate` to an external URL: unmount without unloading; that load replaces the document.
    pub fn detach_for_navigation(&mut self, url: &str) -> Vec<BrowserEvent> {
        let mut events = Vec::new();
        self.latest_request = self.latest_request.wrapping_add(1);
        self.shown = None;
        if let Some(node_id) = self.pending.take() {
            events.push(state_event(node_id, None, BundleState::Closed));
        }
        let removed = self.mounts.unmount();
        self.gate.expect_navigation(url.to_owned());
        if let Some(mount) = removed {
            events.push(state_event(mount.node_id.clone(), Some(&mount.label), BundleState::Closed));
        }
        events
    }

    /// Per frame: leaves, integrity faults and finished opens become events.
    pub fn poll(&mut self, webview: &mut dyn BundleWebview) -> Vec<BrowserEvent> {
        let mut events = Vec::new();
        for notice in self.gate.take_notices() {
            match notice {
                GateNotice::Left { node_id, label } => {
                    if self.shown.as_ref().is_some_and(|shown| shown.node_id == node_id) {
                        self.shown = None;
                    }
                    events.push(state_event(node_id, Some(&label), BundleState::Closed));
                }
            }
        }
        let faults = self.mounts.drain_faults();
        let faulted_live = self
            .mounts
            .current()
            .is_some_and(|live| faults.contains(&live.mount_id));
        if faulted_live {
            if let Some(mount) = self.teardown(webview) {
                events.push(state_event(
                    mount.node_id.clone(),
                    Some(&mount.label),
                    blocked(BlockedReason::Integrity),
                ));
            }
        }
        while let Ok(outcome) = self.outcomes_rx.try_recv() {
            let current = outcome.request == self.latest_request
                && self.pending.as_deref() == Some(outcome.node_id.as_str());
            if !current {
                continue;
            }
            self.pending = None;
            match outcome.result {
                Ok(mount) => events.extend(self.checked_install(mount, webview)),
                Err(state) => {
                    if matches!(state, BundleState::Revoked(_)) {
                        self.mark_revoked(None);
                    }
                    events.push(state_event(outcome.node_id, None, state));
                }
            }
        }
        events
    }

    /// Revoke listener (admission.md §5): re-judge the live mount, and any open in flight, after a governance signal.
    pub fn apply_admission(
        &mut self,
        signal: &AdmissionSignal,
        webview: &mut dyn BundleWebview,
        bridge: Option<&PortalBridge>,
    ) -> Vec<BrowserEvent> {
        if self.pending.is_some() {
            self.signals_since_open.push(signal.clone());
        }
        let event = match signal {
            AdmissionSignal::Event(event) => Some(event),
            AdmissionSignal::ListAccepted => None,
        };
        let Some(mount) = self.mounts.current() else {
            // A revoked portal renders again only on consent for the build it showed.
            let consented = match (&self.shown, event) {
                (Some(shown), Some(event)) if shown.revoked => shown
                    .pair
                    .as_ref()
                    .is_some_and(|(package_id, content_hash)| admission::consents(event, package_id, content_hash)),
                _ => false,
            };
            let node_id = self.shown.as_ref().map(|shown| shown.node_id.clone());
            return match node_id {
                Some(node_id) if consented => self.reopen(&node_id, webview),
                _ => Vec::new(),
            };
        };
        // An event naming the build revokes it even when the view lags the bus.
        let named = event.and_then(|event| admission::revocation_for(event, mount.build()));
        let rejudged = self.rejudge(&mount);
        let refused = named.or_else(|| match &rejudged {
            Some(Rejudge::Refuse(state)) => Some(state.clone()),
            _ => None,
        });
        if let Some(state) = refused {
            return self.revoke(&mount, state, webview, bridge);
        }
        let consented = mount.mode == MountMode::Static
            && event.is_some_and(|event| admission::consents(event, &mount.package_id, &mount.content_hash));
        match rejudged {
            Some(Rejudge::Promote) => self.reopen(&mount.node_id, webview),
            _ if consented => self.reopen(&mount.node_id, webview),
            Some(Rejudge::Restate(state)) => vec![state_event(mount.node_id.clone(), Some(&mount.label), state)],
            _ => Vec::new(),
        }
    }

    /// The node's bytes arrived or its `portal.bundle_asset` changed: an open, unrevoked portal reloads under the new state.
    pub fn refresh(&mut self, node_id: &str, webview: &mut dyn BundleWebview) -> Vec<BrowserEvent> {
        let in_force = self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.node_id == node_id && !shown.revoked);
        if !in_force {
            return Vec::new();
        }
        self.reopen(node_id, webview)
    }

    /// A fresh governance verdict for `mount`, when the resolver carries a view.
    fn rejudge(&self, mount: &Mount) -> Option<Rejudge> {
        let view = self.resolver.as_deref()?.governance()?;
        let verdict = admission::verdict(view, &mount.node_id, &mount.identity());
        Some(admission::rejudge(
            &verdict,
            mount.mode,
            mount.publisher_did.as_deref(),
            mount.can_admit,
        ))
    }

    /// Install a prepared build unless governance changed while it was prepared.
    fn checked_install(&mut self, mut mount: Mount, webview: &mut dyn BundleWebview) -> Vec<BrowserEvent> {
        let signals = std::mem::take(&mut self.signals_since_open);
        let raced = signals.iter().find_map(|signal| match signal {
            AdmissionSignal::Event(event) => admission::revocation_for(event, mount.build()),
            AdmissionSignal::ListAccepted => None,
        });
        let rejudged = self.rejudge(&mount);
        let refused = raced.or_else(|| match &rejudged {
            Some(Rejudge::Refuse(state)) => Some(state.clone()),
            _ => None,
        });
        if let Some(state) = refused {
            if matches!(state, BundleState::Revoked(_)) {
                self.mark_revoked(Some(mount.pair()));
            }
            return vec![state_event(mount.node_id.clone(), None, state)];
        }
        match rejudged {
            // Admitted while it was prepared as Static: prepare again on the admitted label.
            Some(Rejudge::Promote) => {
                let node_id = mount.node_id.clone();
                self.start(&node_id)
            }
            Some(Rejudge::Restate(BundleState::StaticSandbox { can_admit })) => {
                mount.can_admit = can_admit;
                vec![self.install(mount, webview)]
            }
            _ => vec![self.install(mount, webview)],
        }
    }

    /// Insert the mount, mint the entry token, load the entry and show (protocol.md §2.4 steps 4-6).
    fn install(&mut self, mount: Mount, webview: &mut dyn BundleWebview) -> BrowserEvent {
        if self.mounts.unmount().is_some() {
            self.gate.detach();
        }
        let state = mount.state();
        let node_id = mount.node_id.clone();
        if let Some(shown) = self.shown.as_mut().filter(|shown| shown.node_id == node_id) {
            shown.pair = Some(mount.pair());
            shown.revoked = false;
        }
        let mount = self.mounts.install(mount);
        self.gate.expect_entry(mount.entry_url.clone());
        if let Err(error) = webview.bundle_load(&mount.entry_url).and_then(|()| webview.bundle_show()) {
            tracing::warn!(node_id = %node_id, "portal bundle entry load failed: {error:#}");
            self.teardown(webview);
            return state_event(node_id, None, BundleState::Unavailable);
        }
        state_event(node_id, Some(&mount.label), state)
    }

    /// admission.md §5 for the live mount, in order, inside this frame.
    fn revoke(
        &mut self,
        mount: &Arc<Mount>,
        state: BundleState,
        webview: &mut dyn BundleWebview,
        bridge: Option<&PortalBridge>,
    ) -> Vec<BrowserEvent> {
        // 1-2. Remove the mount and bump the generation: no byte is served after this (410 in flight, 404 new).
        if self.mounts.unmount_if(mount.mount_id).is_none() {
            return Vec::new();
        }
        // 3. Close the broker client: subscriptions end; acts and invokes not yet handed over are dropped.
        if let Some(bridge) = bridge {
            bridge.shutdown();
        }
        // 4. Unload under the one-shot token, then hide.
        self.unload(webview, &mount.node_id);
        if matches!(state, BundleState::Revoked(_)) {
            // 6. Queue the per-origin storage clear; the backend runs it after the unload commits (SP-6).
            for label in mount.storage_labels() {
                webview.bundle_clear_storage(&label);
            }
            self.mark_revoked(Some(mount.pair()));
        }
        // 5. The badge changes in place.
        vec![state_event(mount.node_id.clone(), None, state)]
    }

    /// Reload `node_id` under its current state without closing the portal (no Closed; the badge changes when it lands).
    fn reopen(&mut self, node_id: &str, webview: &mut dyn BundleWebview) -> Vec<BrowserEvent> {
        if self.resolver.is_none() {
            return Vec::new();
        }
        self.latest_request = self.latest_request.wrapping_add(1);
        self.pending = None;
        self.teardown(webview);
        match self.shown.as_mut().filter(|shown| shown.node_id == node_id) {
            Some(shown) => shown.revoked = false,
            None => {
                self.shown = Some(Shown {
                    node_id: node_id.to_owned(),
                    pair: None,
                    revoked: false,
                })
            }
        }
        self.start(node_id)
    }

    fn mark_revoked(&mut self, pair: Option<(String, String)>) {
        if let Some(shown) = self.shown.as_mut() {
            shown.revoked = true;
            if pair.is_some() {
                shown.pair = pair;
            }
        }
    }

    /// Mint the unload token, load the unload document, hide (protocol.md §2.5 steps 3-4).
    fn unload(&self, webview: &mut dyn BundleWebview, node_id: &str) {
        self.gate.expect_unload();
        if let Err(error) = webview.bundle_load(UNLOAD_URL) {
            tracing::warn!(node_id = %node_id, "portal bundle unload failed: {error:#}");
        }
        if let Err(error) = webview.bundle_hide() {
            tracing::warn!(node_id = %node_id, "portal hide after unload failed: {error:#}");
        }
    }

    /// Unmount (bumping the generation), then unload; the removed mount, if any.
    fn teardown(&mut self, webview: &mut dyn BundleWebview) -> Option<Arc<Mount>> {
        let mount = self.mounts.unmount()?;
        self.unload(webview, &mount.node_id);
        Some(mount)
    }
}
