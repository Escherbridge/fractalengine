//! Portal bundle resolution (P-S1, P-S4): repository node -> `portal.bundle_asset` -> asset row -> local blob, the
//! portal's synchronous admission view over extension governance, and fe-api's local admission read.
//! Rationale: src/AGENTS.md §portal-resolver.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::hash::Hash;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use bevy::prelude::{App, DetectChanges, IntoScheduleConfigs, MessageReader, Res, Resource, SystemSet, Update};
use fe_api::portal_bundles::{AdmissionQuery, AdmissionView, LocalAdmissionSource};
use fe_database::repo::Db;
use fe_format::integrity::IntegrityStatus;
use fe_format::portal_bundle::PORTAL_BUNDLE_MIME;
use fe_format::HexonArchiveIndex;
use fe_plugin::lifecycle::{GovernanceGates, PackageFacts, TrustTier};
use fe_runtime::app::RuntimeApiEndpoint;
use fe_runtime::blob_store::{hash_from_hex, hash_to_hex, verify_blob_file, BlobStoreHandle};
use fe_webview::ipc::{BlockedReason, BundleState};
use fe_webview::plugin::{apply_portal_admission, PortalBundleResolver};
use fe_webview::portal_bundle::admission::{
    self, AdmissionEvent, AdmissionRecord, AdmissionSignal, GovernanceView, RecordReason, RecordState,
};
use fe_webview::portal_bundle::feeds::FeedSource;
use fe_webview::portal_bundle::{
    bundle_max_bytes, AdmissionVerdict, BundleIdentity, BundleResolver, BundleSource, BundleTarget,
};
use serde_json::Value;

use crate::extension_host::governance::ExtensionGovernance;

/// Bound on one resolver DB read.
const DB_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Repository nodes, petals and builds remembered for main-thread re-judging (oldest evicted first).
const CACHE_ENTRIES: usize = 1024;
/// Admission records mirrored (oldest evicted first; losing one only loses admission).
const RECORD_ENTRIES: usize = 1024;
/// `portal.feeds` entries copied per node (fe-webview keeps at most 16 of them).
const MAX_FEED_ENTRIES: usize = 64;

const NODE_SQL: &str = "SELECT petal_id, properties FROM node WHERE node_id = $node AND tombstone = NONE LIMIT 1";
const PETAL_SQL: &str = "SELECT fractal_id FROM petal WHERE petal_id = $petal AND tombstone = NONE LIMIT 1";
const FRACTAL_SQL: &str = "SELECT verse_id FROM fractal WHERE fractal_id = $fractal AND tombstone = NONE LIMIT 1";
const VERSE_SQL: &str = "SELECT verse_id FROM verse WHERE verse_id = $verse AND tombstone = NONE LIMIT 1";
const ASSET_SQL: &str = "SELECT content_type, content_hash FROM asset WHERE asset_id = $asset LIMIT 1";

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Bounded caches
// ---------------------------------------------------------------------------

/// A map of at most `cap` entries; a new key past the cap evicts the oldest insertion.
struct Bounded<K, V> {
    cap: usize,
    next: u64,
    entries: HashMap<K, (u64, V)>,
}

impl<K: Eq + Hash + Clone, V> Bounded<K, V> {
    fn new(cap: usize) -> Self {
        Self { cap, next: 0, entries: HashMap::new() }
    }

    fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.entries.get(key).map(|(_, value)| value)
    }

    fn get_mut<Q: Eq + Hash + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        self.entries.get_mut(key).map(|(_, value)| value)
    }

    /// Insert or replace `key`; it becomes the newest entry.
    fn insert(&mut self, key: K, value: V) {
        if !self.entries.contains_key(&key) && self.entries.len() >= self.cap {
            let oldest = self.entries.iter().min_by_key(|(_, (seq, _))| *seq).map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.next = self.next.wrapping_add(1);
        self.entries.insert(key, (self.next, value));
    }
}

// ---------------------------------------------------------------------------
// What governance is read through
// ---------------------------------------------------------------------------

/// What the portal reads from extension governance (governance.md §Activation preconditions rows 4-8).
pub(crate) trait GovernanceSource: Send + Sync + 'static {
    /// Row 5: an accepted revocation entry names any of `digests`, under any package id.
    fn listed(&self, digests: &[&str]) -> bool;
    /// The verse Owner's audited kill-list override for `content_hash` in `verse_id` on this host.
    fn overridden_in(&self, verse_id: &str, content_hash: &str) -> bool;
    /// Row 4: the tier of a build sealed by `publisher_did`; `None` when that key is no loaded root.
    fn tier_of(&self, publisher_did: &str) -> Option<TrustTier>;
    /// Row 7: the verse policy and `extensions.toml` admit the build (no manifest, so `ceilings.kinds` never applies).
    fn policy_admits(&self, verse_id: &str, facts: &PackageFacts) -> Result<(), String>;
    /// Row 8 for a build that does not write as an extension: the node DID holds Viewer+ at the petal.
    fn may_read(&self, petal_id: &str) -> Result<(), String>;
    /// Governance's change generation (kill list, policy, overrides); `None` until the source exposes one, when callers
    /// keep their timed re-judge.
    fn generation(&self) -> Option<u64> {
        None
    }
}

impl GovernanceSource for ExtensionGovernance {
    fn listed(&self, digests: &[&str]) -> bool {
        self.is_revoked_any(digests)
    }

    fn overridden_in(&self, verse_id: &str, content_hash: &str) -> bool {
        self.is_overridden(verse_id, content_hash)
    }

    fn tier_of(&self, publisher_did: &str) -> Option<TrustTier> {
        self.trust().tier_for(Some(publisher_did))
    }

    fn policy_admits(&self, verse_id: &str, facts: &PackageFacts) -> Result<(), String> {
        self.evaluate(verse_id, facts)
    }

    fn may_read(&self, petal_id: &str) -> Result<(), String> {
        GovernanceGates::role(self, petal_id, false)
    }

    fn generation(&self) -> Option<u64> {
        Some(ExtensionGovernance::generation(self))
    }
}

/// Host switches the view starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PortalGovernanceConfig {
    /// The plugin host switch (`FE_PLUGIN_HOST_ALLOW=1`, or the binary's `PackageAdmission`): off admits nothing.
    pub admission_enabled: bool,
    /// `FE_EXT_DISABLE_ALL=1` at startup (layer b).
    pub disable_all: bool,
}

impl PortalGovernanceConfig {
    /// Both switches from the environment (exactly `"1"` turns each on).
    pub(crate) fn from_env() -> Self {
        Self {
            admission_enabled: fe_plugin::plugin_host_allowed(std::env::var(fe_plugin::PLUGIN_HOST_ENV).ok().as_deref()),
            disable_all: fe_plugin::lifecycle::ext_disable_all(
                std::env::var(fe_plugin::lifecycle::EXT_DISABLE_ALL_ENV).ok().as_deref(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The portal's admission view (admission.md A-15)
// ---------------------------------------------------------------------------

/// Where an opened repository node lives, and its static feeds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NodeScope {
    petal_id: String,
    verse_id: String,
    /// `properties.portal.feeds` as written: name -> share URL.
    feeds: BTreeMap<String, String>,
}

/// What a sealed build's verified header says; the `content_hash` binds both values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildFacts {
    publisher_did: String,
    /// Empty when only the seal is known (a summary read); a SemVer allow entry then never matches.
    version: String,
}

/// One mirrored admission record (governance.md §Admission record, web bundles).
#[derive(Debug, Clone)]
struct RecordEntry {
    state: RecordState,
    reason: RecordReason,
    /// The consented capability digest; empty for a record learned only from a refusal event.
    capability_digest: String,
}

fn record_key(package_id: &str, content_hash: &str) -> (String, String) {
    (package_id.to_owned(), content_hash.to_ascii_lowercase())
}

/// Governance's cached, synchronous view for the portal (A-15): every call reads memory only.
pub(crate) struct PortalGovernance {
    source: Arc<dyn GovernanceSource>,
    admission_enabled: bool,
    /// Shared with `plugin_bridge` so Disable all / Enable all reach the portal (R-X-33).
    disabled_all: Arc<AtomicBool>,
    /// The QA lane's native kill switch: read through the same host-disabled check, cleared only by its own disengage.
    kill_switch: AtomicBool,
    records: RwLock<Bounded<(String, String), RecordEntry>>,
    nodes: RwLock<Bounded<String, NodeScope>>,
    /// petal id -> verse id.
    petals: RwLock<Bounded<String, String>>,
    /// content_hash -> facts of a sealed build.
    builds: RwLock<Bounded<String, BuildFacts>>,
}

impl PortalGovernance {
    /// A view over `source`; no record exists until a consent or event arrives.
    pub(crate) fn new(source: Arc<dyn GovernanceSource>, config: PortalGovernanceConfig) -> Self {
        Self {
            source,
            admission_enabled: config.admission_enabled,
            disabled_all: Arc::new(AtomicBool::new(config.disable_all)),
            kill_switch: AtomicBool::new(false),
            records: RwLock::new(Bounded::new(RECORD_ENTRIES)),
            nodes: RwLock::new(Bounded::new(CACHE_ENTRIES)),
            petals: RwLock::new(Bounded::new(CACHE_ENTRIES)),
            builds: RwLock::new(Bounded::new(CACHE_ENTRIES)),
        }
    }

    /// A local consent for a web bundle (startup hydration or a fresh Admit): the pair is admitted for exactly this digest.
    pub(crate) fn learn_consent(&self, package_id: &str, content_hash: &str, capability_digest: &str) {
        if package_id.is_empty() || content_hash.is_empty() || capability_digest.is_empty() {
            return;
        }
        let entry = RecordEntry {
            state: RecordState::Admitted,
            reason: RecordReason::Consented,
            capability_digest: capability_digest.to_owned(),
        };
        write_lock(&self.records).insert(record_key(package_id, content_hash), entry);
    }

    /// Forget a consent (the pair's record), so an Admit is needed again.
    pub(crate) fn forget_consent(&self, package_id: &str, content_hash: &str) {
        write_lock(&self.records).entries.remove(&record_key(package_id, content_hash));
    }

    /// Every `(package_id, content_hash)` consented and admitted on this host (feeds `LocalFacts.admitted_bundles`, R-X-04).
    /// Empty while the host switch is off or Disable all is on; a pair the kill list names under any digest is left out.
    pub(crate) fn admitted_set(&self) -> BTreeSet<(String, String)> {
        if !self.admission_enabled || self.host_disabled() {
            return BTreeSet::new();
        }
        let admitted: Vec<(String, String)> = read_lock(&self.records)
            .entries
            .iter()
            .filter(|(_, (_, entry))| entry.state == RecordState::Admitted)
            .map(|(key, _)| key.clone())
            .collect();
        admitted.into_iter().filter(|(_, content_hash)| !self.source.listed(&[content_hash.as_str()])).collect()
    }

    /// The shared Disable all flag: `plugin_bridge` stores into it (R-X-33), the portal view reads it.
    pub(crate) fn host_disabled_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.disabled_all)
    }

    /// Governance's change generation, when the source exposes one; a change means re-judge open portals now.
    pub(crate) fn generation(&self) -> Option<u64> {
        self.source.generation()
    }

    /// `true` once when the generation moved past `seen` (which is then updated); always `false` without a generation.
    pub(crate) fn changed_since(&self, seen: &mut Option<u64>) -> bool {
        let Some(now) = self.generation() else {
            return false;
        };
        let changed = seen.is_some_and(|before| before != now);
        *seen = Some(now);
        changed
    }

    /// One `host/admission` delta; an `admitted` event alone never creates a record (it carries no capability digest).
    pub(crate) fn observe(&self, event: &AdmissionEvent) {
        if event.state == RecordState::Disabled && event.reason == RecordReason::DisableAll {
            self.set_host_disabled(true);
            return;
        }
        if event.package_id.is_empty() || event.content_hash.is_empty() {
            return;
        }
        let key = record_key(&event.package_id, &event.content_hash);
        let mut records = write_lock(&self.records);
        if let Some(entry) = records.get_mut(&key) {
            if event.state != RecordState::Admitted || !entry.capability_digest.is_empty() {
                entry.state = event.state;
                entry.reason = event.reason;
            }
            return;
        }
        if event.state != RecordState::Admitted {
            let entry = RecordEntry { state: event.state, reason: event.reason, capability_digest: String::new() };
            records.insert(key, entry);
        }
    }

    /// Layer (b) at runtime: Disable all sets it, Enable all clears it.
    pub(crate) fn set_host_disabled(&self, disabled: bool) {
        self.disabled_all.store(disabled, Ordering::Release);
    }

    /// The native kill switch (QA lane): engaged, the view is host-disabled exactly as under Disable all; disengaging
    /// never clears an operator's Disable all.
    pub(crate) fn set_kill_switch(&self, engaged: bool) {
        self.kill_switch.store(engaged, Ordering::Release);
    }

    fn remember_node(&self, node_id: &str, scope: NodeScope) {
        write_lock(&self.petals).insert(scope.petal_id.clone(), scope.verse_id.clone());
        write_lock(&self.nodes).insert(node_id.to_owned(), scope);
    }

    fn remember_build(&self, content_hash: &str, facts: BuildFacts) {
        write_lock(&self.builds).insert(content_hash.to_ascii_lowercase(), facts);
    }

    /// The sealing DID of a build a verified summary read reported; never replaces facts read from a header.
    fn remember_publisher(&self, content_hash: &str, publisher_did: &str) {
        if content_hash.is_empty() || publisher_did.is_empty() {
            return;
        }
        let key = content_hash.to_ascii_lowercase();
        let mut builds = write_lock(&self.builds);
        if builds.get(key.as_str()).is_none() {
            builds.insert(key, BuildFacts { publisher_did: publisher_did.to_owned(), version: String::new() });
        }
    }

    fn node_feeds(&self, node_id: &str) -> BTreeMap<String, String> {
        read_lock(&self.nodes).get(node_id).map(|scope| scope.feeds.clone()).unwrap_or_default()
    }

    /// `(petal_id, verse_id)` of a target this peer resolved; `None` fails every scoped check closed.
    fn scope_of(&self, target: &BundleTarget) -> Option<(String, String)> {
        match target {
            BundleTarget::Node(node_id) if !node_id.is_empty() => read_lock(&self.nodes)
                .get(node_id.as_str())
                .map(|scope| (scope.petal_id.clone(), scope.verse_id.clone())),
            BundleTarget::Node(_) => None,
            BundleTarget::Bound { petal_id, .. } => {
                read_lock(&self.petals).get(petal_id.as_str()).map(|verse_id| (petal_id.clone(), verse_id.clone()))
            }
        }
    }

    /// The scope-free half of may-admit: the host switch on, Disable all off, sealed by a loaded trust root. Verse
    /// policy and role need a target, so a `true` here is not a promise; the consent dialog judges those.
    fn may_admit_on_host(&self, identity: &BundleIdentity) -> bool {
        self.admission_enabled
            && !self.host_disabled()
            && identity
                .publisher_did
                .as_deref()
                .is_some_and(|publisher| self.source.tier_of(publisher).is_some())
    }

    fn is_admitted(&self, package_id: &str, content_hash: &str) -> bool {
        self.admission_enabled
            && read_lock(&self.records)
                .get(&record_key(package_id, content_hash))
                .is_some_and(|entry| entry.state == RecordState::Admitted)
    }

    /// Rows 4 and 7 in `verse_id`: a loaded root sealed the build and both policies admit it.
    fn admissible(&self, verse_id: &str, identity: &BundleIdentity) -> Result<(), String> {
        let publisher = identity
            .publisher_did
            .as_deref()
            .ok_or_else(|| "an unsealed build is never admitted".to_owned())?;
        let trust = self
            .source
            .tier_of(publisher)
            .ok_or_else(|| format!("{publisher} is not a trust root on this host"))?;
        let version = read_lock(&self.builds)
            .get(identity.content_hash.to_ascii_lowercase().as_str())
            .map(|build| build.version.clone())
            .unwrap_or_default();
        let facts = PackageFacts {
            package_id: identity.package_id.clone(),
            version,
            content_hash: identity.content_hash.clone(),
            component_digest: None,
            plaintext_digest: None,
            capability_digest: identity.capability_digest.clone(),
            trust,
            writes: false,
            licensed: false,
            manifest: None,
        };
        self.source.policy_admits(verse_id, &facts)
    }
}

impl GovernanceView for PortalGovernance {
    fn host_disabled(&self) -> bool {
        self.disabled_all.load(Ordering::Acquire) || self.kill_switch.load(Ordering::Acquire)
    }

    fn kill_listed(&self, digests: &[&str]) -> bool {
        let named: Vec<&str> = digests.iter().copied().filter(|digest| !digest.is_empty()).collect();
        !named.is_empty() && self.source.listed(&named)
    }

    fn kill_list_override(&self, target: &BundleTarget, content_hash: &str) -> bool {
        !content_hash.is_empty()
            && self
                .scope_of(target)
                .is_some_and(|(_, verse_id)| self.source.overridden_in(&verse_id, content_hash))
    }

    fn record(&self, package_id: &str, content_hash: &str) -> Option<AdmissionRecord> {
        if package_id.is_empty() || content_hash.is_empty() {
            return None;
        }
        let key = record_key(package_id, content_hash);
        let entry = read_lock(&self.records).get(&key).cloned()?;
        // With the host switch off nothing is admitted; refusals still apply.
        if entry.state == RecordState::Admitted && !self.admission_enabled {
            return None;
        }
        let publisher_did = read_lock(&self.builds)
            .get(key.1.as_str())
            .map(|build| build.publisher_did.clone())
            .unwrap_or_default();
        Some(AdmissionRecord {
            state: entry.state,
            reason: entry.reason,
            publisher_did,
            capability_digest: entry.capability_digest,
        })
    }

    fn can_admit(&self, target: &BundleTarget, identity: &BundleIdentity) -> bool {
        if !self.admission_enabled || self.host_disabled() || identity.publisher_did.is_none() {
            return false;
        }
        let Some((petal_id, verse_id)) = self.scope_of(target) else {
            return false;
        };
        self.admissible(&verse_id, identity).is_ok() && self.source.may_read(&petal_id).is_ok()
    }

    /// An admitted build stays off a target whose verse policy (or trust) no longer admits it; static rendering is never
    /// policy-gated (A-12).
    fn render_ceiling(&self, target: &BundleTarget, identity: &BundleIdentity) -> bool {
        if !self.is_admitted(&identity.package_id, &identity.content_hash) {
            return false;
        }
        match self.scope_of(target) {
            Some((_, verse_id)) => self.admissible(&verse_id, identity).is_err(),
            None => true,
        }
    }
}

// ---------------------------------------------------------------------------
// The resolver (BundleResolver)
// ---------------------------------------------------------------------------

/// A repository node's links, read from the DB.
struct RepositoryRow {
    petal_id: String,
    verse_id: String,
    asset_id: String,
    blob_hex: String,
    feeds: BTreeMap<String, String>,
}

/// The binary's [`BundleResolver`]: node -> `portal.bundle_asset` -> asset row -> local blob, judged by [`PortalGovernance`].
pub(crate) struct PortalResolver {
    /// Clone of the DB thread's handle, read only.
    db: Db,
    blobs: BlobStoreHandle,
    governance: Arc<PortalGovernance>,
    /// This peer's fe-api origin once the listener is bound.
    api_origin: RwLock<Option<String>>,
}

impl PortalResolver {
    /// A resolver over the shared DB handle and blob store, judged through `source`.
    pub(crate) fn new(
        db: Db,
        blobs: BlobStoreHandle,
        source: Arc<dyn GovernanceSource>,
        config: PortalGovernanceConfig,
    ) -> Self {
        Self {
            db,
            blobs,
            governance: Arc::new(PortalGovernance::new(source, config)),
            api_origin: RwLock::new(None),
        }
    }

    /// The admission view (consent hydration, Disable all / Enable all).
    pub(crate) fn view(&self) -> &Arc<PortalGovernance> {
        &self.governance
    }

    /// fe-api's local admission read; `client_did` is the client DID of this host's own API tokens.
    pub(crate) fn local_admission(&self, client_did: Option<String>) -> LocalAdmission {
        LocalAdmission { governance: Arc::clone(&self.governance), client_did: client_did.unwrap_or_default() }
    }

    /// The consented, admitted web-bundle pairs (see [`PortalGovernance::admitted_set`]).
    pub(crate) fn admitted_set(&self) -> BTreeSet<(String, String)> {
        self.governance.admitted_set()
    }

    /// The shared Disable all flag (see [`PortalGovernance::host_disabled_handle`]).
    pub(crate) fn host_disabled_handle(&self) -> Arc<AtomicBool> {
        self.governance.host_disabled_handle()
    }

    fn set_api_origin(&self, origin: Option<String>) {
        *write_lock(&self.api_origin) = origin;
    }
}

impl BundleResolver for PortalResolver {
    fn resolve(&self, node_id: &str) -> Result<BundleSource, BundleState> {
        let Some(row) = block_on(load_repository(&self.db, node_id)) else {
            tracing::debug!(node_id, "portal open: no repository bundle on this node");
            return Err(BundleState::Unavailable);
        };
        let hash = hash_from_hex(&row.blob_hex).map_err(|_| BundleState::Blocked(BlockedReason::Integrity))?;
        let blob_hex = hash_to_hex(&hash);
        let scope = NodeScope { petal_id: row.petal_id, verse_id: row.verse_id, feeds: row.feeds };
        self.governance.remember_node(node_id, scope);
        let waiting = BundleSource { asset_id: row.asset_id, blob_path: None, revoked: None };
        // Rules 3-4 before any byte: Disable all, or the blob hash listed under any package id.
        let target = BundleTarget::Node(node_id.to_owned());
        let view: &dyn GovernanceView = &*self.governance;
        if let Some(reason) = admission::revoked_before_bytes(view, &target, Some(blob_hex.as_str())) {
            return Ok(BundleSource { revoked: Some(reason), ..waiting });
        }
        let Some(path) = self.blobs.get_blob_path(&hash) else {
            return Ok(waiting);
        };
        let Ok(size) = std::fs::metadata(&path).map(|metadata| metadata.len()) else {
            return Ok(waiting);
        };
        if size > bundle_max_bytes() {
            return Err(BundleState::Blocked(BlockedReason::TooLarge));
        }
        // The header is read only from bytes whose hash is the one the kill list just cleared.
        match verify_blob_file(&path, &hash, size) {
            Ok(true) => {}
            Ok(false) => return Err(BundleState::Blocked(BlockedReason::Integrity)),
            Err(_) => return Ok(waiting),
        }
        if let Some((content_hash, facts)) = sealed_build_facts(&path) {
            self.governance.remember_build(&content_hash, facts);
        }
        Ok(BundleSource { blob_path: Some(path), ..waiting })
    }

    fn governance(&self) -> Option<&dyn GovernanceView> {
        let view: &dyn GovernanceView = &*self.governance;
        Some(view)
    }

    fn feeds(&self, node_id: &str) -> FeedSource {
        FeedSource {
            feeds: self.governance.node_feeds(node_id),
            local_api_origin: read_lock(&self.api_origin).clone(),
        }
    }
}

/// `(content_hash, facts)` of a sealed build from its verified header; `None` when unsealed or unreadable.
fn sealed_build_facts(path: &Path) -> Option<(String, BuildFacts)> {
    let archive = HexonArchiveIndex::open(path).ok()?;
    let IntegrityStatus::Sealed { publisher_did } = archive.integrity_status() else {
        return None;
    };
    let content_hash = fe_format::integrity::content_hash(archive.header_doc().manifest.as_bytes()).ok()?;
    let version = archive.manifest().version.clone();
    Some((content_hash, BuildFacts { publisher_did, version }))
}

/// Run DB reads on a private current-thread runtime; `None` on a tokio worker, which must never block.
fn block_on<T>(work: impl Future<Output = Option<T>>) -> Option<T> {
    if tokio::runtime::Handle::try_current().is_ok() {
        tracing::warn!("portal resolver called on a tokio worker; resolution refused");
        return None;
    }
    match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(work),
        Err(error) => {
            tracing::warn!(%error, "portal resolver runtime did not start");
            None
        }
    }
}

/// The rows of one parameterised SELECT; `None` (logged) on a DB error or timeout.
async fn select(db: &Db, sql: &'static str, name: &'static str, value: &str) -> Option<Vec<Value>> {
    let query = db.query(sql).bind((name, value.to_owned()));
    let response = match tokio::time::timeout(DB_READ_TIMEOUT, async move { query.await }).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            tracing::warn!(%error, sql, "portal resolver read failed");
            return None;
        }
        Err(_) => {
            tracing::warn!(sql, "portal resolver read timed out");
            return None;
        }
    };
    match response.check().and_then(|mut checked| checked.take::<Vec<Value>>(0)) {
        Ok(rows) => Some(rows),
        Err(error) => {
            tracing::warn!(%error, sql, "portal resolver read failed");
            None
        }
    }
}

fn first(rows: Vec<Value>) -> Option<Value> {
    rows.into_iter().next()
}

fn text(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A live repository node's petal, verse, bundle asset and feeds; `None` when any link is absent or unreadable.
async fn load_repository(db: &Db, node_id: &str) -> Option<RepositoryRow> {
    let node = first(select(db, NODE_SQL, "node", node_id).await?)?;
    let properties = node.get("properties").and_then(Value::as_object)?;
    if properties.get("node_kind").and_then(Value::as_str) != Some("repository") {
        return None;
    }
    let asset_id = properties
        .get("portal.bundle_asset")
        .and_then(Value::as_str)
        .filter(|id| id.parse::<ulid::Ulid>().is_ok())?
        .to_owned();
    let feeds: BTreeMap<String, String> = properties
        .get("portal.feeds")
        .and_then(Value::as_object)
        .map(|declared| {
            declared
                .iter()
                .take(MAX_FEED_ENTRIES)
                .filter_map(|(name, url)| Some((name.clone(), url.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let petal_id = text(&node, "petal_id")?;
    let fractal_id = text(&first(select(db, PETAL_SQL, "petal", &petal_id).await?)?, "fractal_id")?;
    let verse_id = text(&first(select(db, FRACTAL_SQL, "fractal", &fractal_id).await?)?, "verse_id")?;
    first(select(db, VERSE_SQL, "verse", &verse_id).await?)?;
    let asset = first(select(db, ASSET_SQL, "asset", &asset_id).await?)?;
    if text(&asset, "content_type").as_deref() != Some(PORTAL_BUNDLE_MIME) {
        return None;
    }
    let blob_hex = text(&asset, "content_hash")?;
    Some(RepositoryRow { petal_id, verse_id, asset_id, blob_hex, feeds })
}

// ---------------------------------------------------------------------------
// fe-api's local admission read (README §8.5)
// ---------------------------------------------------------------------------

/// The summary read's admission block and the export's revocation check: the scope-free record verdict.
pub(crate) struct LocalAdmission {
    governance: Arc<PortalGovernance>,
    /// Empty when this host mints no client tokens; fe-identity never accepts an empty client DID, so nothing matches.
    client_did: String,
}

impl LocalAdmissionSource for LocalAdmission {
    fn local_did(&self) -> String {
        self.client_did.clone()
    }

    fn admission(&self, bundle: &AdmissionQuery<'_>) -> Option<AdmissionView> {
        if let Some(publisher) = bundle.sealed_by {
            self.governance.remember_publisher(bundle.content_hash, publisher);
        }
        // Every committed member digest is a kill-list key too (admission.md §4.2, hash under any id).
        let identity = BundleIdentity {
            package_id: bundle.package_id.to_owned(),
            content_hash: bundle.content_hash.to_owned(),
            archive_digest: bundle.archive_digest.to_owned(),
            publisher_did: bundle.sealed_by.map(str::to_owned),
            capability_digest: bundle.capability_digest.to_owned(),
            member_digests: bundle.member_digests.to_vec(),
        };
        // No repository node names a verse here, so no Owner override applies (fails closed).
        let view: &dyn GovernanceView = &*self.governance;
        let verdict = admission::record_verdict(view, &BundleTarget::Node(String::new()), &identity);
        let (state, mut reason) = admission::summary_codes(&verdict);
        // A read build (not the archive-only pre-check) the local identity can never admit on this host.
        let read = !bundle.package_id.is_empty() && !bundle.content_hash.is_empty();
        if read && matches!(verdict, AdmissionVerdict::Unadmitted { .. }) && !self.governance.may_admit_on_host(&identity) {
            reason = Some("cannot_admit");
        }
        Some(AdmissionView { state: state.to_owned(), reason: reason.map(str::to_owned) })
    }
}

// ---------------------------------------------------------------------------
// Bevy wiring
// ---------------------------------------------------------------------------

/// The resolver for the binary's own systems (the webview reads it as [`PortalBundleResolver`]).
#[derive(Resource, Clone)]
pub(crate) struct PortalResolverRes(pub(crate) Arc<PortalResolver>);

/// Systems writing [`AdmissionSignal`]: they run before the view observes them and before the revoke listener.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AdmissionWriters;

/// Insert the resolver for the webview, register fe-api's local admission read, and keep the view current.
pub(crate) fn install(app: &mut App, resolver: Arc<PortalResolver>, local_client_did: Option<String>) {
    fe_api::portal_bundles::set_local_admission_source(Arc::new(resolver.local_admission(local_client_did)));
    let webview_resolver: Arc<dyn BundleResolver> = resolver.clone();
    app.insert_resource(PortalBundleResolver(webview_resolver));
    app.insert_resource(PortalResolverRes(resolver));
    app.configure_sets(Update, AdmissionWriters.before(apply_portal_admission));
    app.add_systems(
        Update,
        (
            follow_api_endpoint,
            observe_admission.after(AdmissionWriters).before(apply_portal_admission),
        ),
    );
}

/// Mirror the bound fe-api address into the resolver: a static feed may name only this peer's own origin.
fn follow_api_endpoint(endpoint: Option<Res<RuntimeApiEndpoint>>, resolver: Res<PortalResolverRes>) {
    let Some(endpoint) = endpoint else {
        return;
    };
    if endpoint.is_changed() {
        resolver.0.set_api_origin(endpoint.0.map(|address| format!("http://{address}")));
    }
}

/// Feed each `host/admission` delta into the view before the revoke listener re-judges with it.
fn observe_admission(mut signals: MessageReader<AdmissionSignal>, resolver: Res<PortalResolverRes>) {
    for signal in signals.read() {
        if let AdmissionSignal::Event(event) = signal {
            resolver.0.governance.observe(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use fe_webview::ipc::RevokedReason;

    use super::*;

    const PUBLISHER: &str = "did:key:z6Mkpublisher";
    const PACKAGE: &str = "org.example.dashboard";

    /// Governance as the binary adapts it: a mutable kill list, one Owner override, one verse whose policy refuses.
    #[derive(Default)]
    struct FakeGovernance {
        listed: Mutex<Vec<String>>,
        overrides: Vec<(String, String)>,
        refusing_verses: Vec<String>,
        generation: std::sync::atomic::AtomicU64,
    }

    impl FakeGovernance {
        fn list(&self, digests: &[&str]) {
            self.generation.fetch_add(1, Ordering::SeqCst);
            *self.listed.lock().unwrap_or_else(PoisonError::into_inner) =
                digests.iter().map(|digest| (*digest).to_owned()).collect();
        }
    }

    impl GovernanceSource for FakeGovernance {
        fn listed(&self, digests: &[&str]) -> bool {
            let listed = self.listed.lock().unwrap_or_else(PoisonError::into_inner);
            digests.iter().any(|digest| listed.iter().any(|entry| entry == digest))
        }
        fn overridden_in(&self, verse_id: &str, content_hash: &str) -> bool {
            self.overrides.iter().any(|(verse, hash)| verse == verse_id && hash == content_hash)
        }
        fn tier_of(&self, publisher_did: &str) -> Option<TrustTier> {
            (publisher_did == PUBLISHER).then_some(TrustTier::Publisher)
        }
        fn policy_admits(&self, verse_id: &str, facts: &PackageFacts) -> Result<(), String> {
            assert!(facts.manifest.is_none(), "a web bundle carries no extension manifest (no kinds ceiling)");
            if self.refusing_verses.iter().any(|verse| verse == verse_id) {
                Err("verse policy: no allow entry admits this build".into())
            } else {
                Ok(())
            }
        }
        fn may_read(&self, _petal_id: &str) -> Result<(), String> {
            Ok(())
        }
        fn generation(&self) -> Option<u64> {
            Some(self.generation.load(Ordering::SeqCst))
        }
    }

    fn identity(content_hash: &str, publisher: Option<&str>, capability_digest: &str) -> BundleIdentity {
        BundleIdentity {
            package_id: PACKAGE.into(),
            content_hash: content_hash.into(),
            archive_digest: "z1".into(),
            publisher_did: publisher.map(str::to_owned),
            capability_digest: capability_digest.into(),
            member_digests: vec!["m1".into()],
        }
    }

    fn event(package_id: &str, content_hash: &str, state: RecordState, reason: RecordReason) -> AdmissionEvent {
        AdmissionEvent { package_id: package_id.into(), content_hash: content_hash.into(), state, reason }
    }

    fn view_over(fake: &Arc<FakeGovernance>, admission_enabled: bool) -> Arc<PortalGovernance> {
        let source: Arc<dyn GovernanceSource> = fake.clone();
        let view = Arc::new(PortalGovernance::new(source, PortalGovernanceConfig { admission_enabled, disable_all: false }));
        for (node, petal, verse) in [("node-a", "petal-a", "verse-a"), ("node-b", "petal-b", "verse-b")] {
            let scope = NodeScope { petal_id: petal.into(), verse_id: verse.into(), feeds: BTreeMap::new() };
            view.remember_node(node, scope);
        }
        view.remember_build("h1", BuildFacts { publisher_did: PUBLISHER.into(), version: "1.0.0".into() });
        view
    }

    fn summary(local: &LocalAdmission, query: &AdmissionQuery<'_>) -> (String, Option<String>) {
        let view = local.admission(query).expect("the local read always answers");
        (view.state, view.reason)
    }

    /// The portal's view over governance: verse policy and trust per target, consented records, the kill list by any
    /// digest with the Owner override per verse, local and host-wide disable, the host switch, and the summary read.
    #[test]
    fn portal_view_judges_policy_records_kill_list_override_and_summary() {
        let fake = Arc::new(FakeGovernance {
            overrides: vec![("verse-a".into(), "h1".into())],
            refusing_verses: vec!["verse-b".into()],
            ..FakeGovernance::default()
        });
        let view = view_over(&fake, true);
        let (at_a, at_b) = (BundleTarget::Node("node-a".into()), BundleTarget::Node("node-b".into()));
        let unresolved = BundleTarget::Node("node-never-opened".into());
        let sealed = identity("h1", Some(PUBLISHER), "d1");
        let judge = |target: &BundleTarget, identity: &BundleIdentity| admission::verdict(&*view, target, identity);
        let unadmitted = |can_admit| AdmissionVerdict::Unadmitted { can_admit };
        let admitted = AdmissionVerdict::Admitted { publisher_did: PUBLISHER.into() };

        for (row, target, identity, expected) in [
            ("a root-sealed build may be admitted where the policy admits it", &at_a, sealed.clone(), unadmitted(true)),
            ("the target's verse policy refuses it", &at_b, sealed.clone(), unadmitted(false)),
            ("a target this peer never resolved admits nothing", &unresolved, sealed.clone(), unadmitted(false)),
            ("an unsealed build is never admitted", &at_a, identity("h1", None, "d1"), unadmitted(false)),
            ("a publisher that is no trust root", &at_a, identity("h1", Some("did:key:z6Mkother"), "d1"), unadmitted(false)),
        ] {
            assert_eq!(judge(target, &identity), expected, "{row}");
        }

        view.observe(&event(PACKAGE, "h1", RecordState::Admitted, RecordReason::Consented));
        assert_eq!(judge(&at_a, &sealed), unadmitted(true), "an admitted event alone carries no capability digest");
        view.learn_consent(PACKAGE, "h1", "d1");
        for (row, target, identity, expected) in [
            ("consented pair, sealed by the root", &at_a, sealed.clone(), admitted.clone()),
            (
                "an admitted build stays off a verse whose policy refuses it",
                &at_b,
                sealed.clone(),
                AdmissionVerdict::Blocked(BlockedReason::VersePolicy),
            ),
            ("another capability digest needs its own consent", &at_a, identity("h1", Some(PUBLISHER), "d2"), unadmitted(true)),
        ] {
            assert_eq!(judge(target, &identity), expected, "{row}");
        }

        // The kill list names the archive digest under no package id: refused everywhere but where the Owner overrode it.
        fake.list(&["z1"]);
        assert_eq!(judge(&at_b, &sealed), AdmissionVerdict::Revoked(RevokedReason::RevocationList));
        assert_eq!(judge(&at_a, &sealed), admitted, "the (verse-a, h1) override spares it in verse-a only");
        assert_eq!(
            admission::revoked_before_bytes(&*view, &at_a, Some("z1")),
            Some(RevokedReason::RevocationList),
            "a sealed archive digest is refused before bytes; the override keys on the content hash"
        );

        let local = LocalAdmission { governance: view.clone(), client_did: "did:key:z6Mkclient".into() };
        assert_eq!(local.local_did(), "did:key:z6Mkclient");
        let members = vec!["m1".to_owned()];
        let query = AdmissionQuery {
            package_id: PACKAGE,
            content_hash: "h1",
            archive_digest: "z1",
            member_digests: &members,
            sealed_by: Some(PUBLISHER),
            capability_digest: "d1",
        };
        assert_eq!(
            summary(&local, &query),
            ("revoked".to_owned(), Some("revocation_list".to_owned())),
            "the summary is scope-free, so no Owner override applies"
        );
        // A member digest is a kill-list key under any id. Mutation check: dropping `member_digests` reads "admitted".
        fake.list(&["m1"]);
        assert_eq!(summary(&local, &query), ("revoked".to_owned(), Some("revocation_list".to_owned())));
        fake.list(&[]);
        assert_eq!(summary(&local, &query), ("admitted".to_owned(), None));
        let archive_only = AdmissionQuery {
            package_id: "",
            content_hash: "",
            archive_digest: "z2",
            member_digests: &[],
            sealed_by: None,
            capability_digest: "",
        };
        assert_eq!(summary(&local, &archive_only), ("static_sandbox".to_owned(), None), "the export's pre-check");
        // README §8.5: a static build the local identity can never admit here says so; a trusted seal leaves it open.
        let unsealed = AdmissionQuery { content_hash: "h7", sealed_by: None, ..query };
        let untrusted = AdmissionQuery { content_hash: "h8", sealed_by: Some("did:key:z6Mkother"), ..query };
        let other_build = AdmissionQuery { content_hash: "h9", ..query };
        for (row, query, reason) in [
            ("unsealed", unsealed, Some("cannot_admit".to_owned())),
            ("sealed by no trust root", untrusted, Some("cannot_admit".to_owned())),
            ("sealed by a root, not yet consented", other_build, None),
        ] {
            assert_eq!(summary(&local, &query), ("static_sandbox".to_owned(), reason), "{row}");
        }

        // The admitted set feeds the binding facts: consented pairs, minus kill-listed hashes, empty under Disable all.
        let mut seen = None;
        assert!(!view.changed_since(&mut seen), "the first read only records the generation");
        assert_eq!(view.admitted_set(), BTreeSet::from([(PACKAGE.to_owned(), "h1".to_owned())]));
        fake.list(&["h1"]);
        assert!(view.changed_since(&mut seen), "a kill-list change moves the generation");
        assert!(!view.changed_since(&mut seen), "and is reported once");
        assert!(view.admitted_set().is_empty(), "a kill-listed hash is never in the admitted set");
        fake.list(&[]);
        let handle = view.host_disabled_handle();
        handle.store(true, Ordering::Release);
        assert!(view.host_disabled() && view.admitted_set().is_empty(), "the shared handle is Disable all");
        handle.store(false, Ordering::Release);
        assert_eq!(view.admitted_set().len(), 1);

        view.observe(&event(PACKAGE, "h1", RecordState::Disabled, RecordReason::LocalDisable));
        assert!(view.admitted_set().is_empty(), "a locally disabled pair leaves the admitted set");
        assert_eq!(judge(&at_a, &sealed), AdmissionVerdict::Revoked(RevokedReason::KillSwitch), "locally disabled");
        view.observe(&event(PACKAGE, "h1", RecordState::Admitted, RecordReason::Restored));
        assert_eq!(judge(&at_a, &sealed), admitted, "the consented record is restored");
        view.observe(&event("org.example.other", "h9", RecordState::Disabled, RecordReason::DisableAll));
        assert_eq!(judge(&at_a, &sealed), AdmissionVerdict::Revoked(RevokedReason::KillSwitch), "Disable all");
        assert_eq!(admission::revoked_before_bytes(&*view, &at_a, Some("z1")), Some(RevokedReason::KillSwitch));
        view.set_host_disabled(false);
        assert_eq!(judge(&at_a, &sealed), admitted, "Enable all");
        // The QA kill switch reads as Disable all, and its disengage never clears an operator's Disable all.
        view.set_kill_switch(true);
        assert_eq!(judge(&at_a, &sealed), AdmissionVerdict::Revoked(RevokedReason::KillSwitch), "kill switch engaged");
        assert!(view.admitted_set().is_empty());
        view.set_host_disabled(true);
        view.set_kill_switch(false);
        assert!(view.host_disabled(), "Disable all outlives the kill switch's disengage");
        view.set_host_disabled(false);
        assert_eq!(judge(&at_a, &sealed), admitted);

        // The plugin host switch off: nothing is admitted, refusals still apply.
        let off = view_over(&fake, false);
        off.learn_consent(PACKAGE, "h1", "d1");
        assert_eq!(admission::verdict(&*off, &at_a, &sealed), unadmitted(false));
        off.observe(&event(PACKAGE, "h1", RecordState::Disabled, RecordReason::LocalDisable));
        assert_eq!(admission::verdict(&*off, &at_a, &sealed), AdmissionVerdict::Revoked(RevokedReason::KillSwitch));
    }
}
