//! Extension governance, host side (docs/extensions/governance.md): R19 rows 6-8 as `GovernanceGates` over a
//! verse-policy mirror, `extensions.toml` and the node's own roles; the verse Owner's kill-list override;
//! consent and override hydration; the op-log audit sink; the bus role and external-access feeds.
//! Rationale and lock order: src/AGENTS.md §extension-governance.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use bevy::prelude::{App, DetectChanges, MessageReader, Res, ResMut, Resource, Update};
use crossbeam::channel::{Receiver, Sender, TryRecvError, TrySendError};
use fe_api::extensions::{AdminRefusal, AdminReply};
use fe_database::handlers::extension_policy::{
    self as policy_db, AuditAction, AuditEntry, AuditOutcome, ConsentRecord as StoredConsent, ExtensionPolicy,
    KillListOverride, KillListOverrideRequest, LocalPolicy, PolicyFacts, PolicyRow, TrustTier as PolicyTrust,
};
use fe_database::RoleLevel;
use fe_plugin::bus::{AuditAction as BusAction, AuditEntry as BusEntry, AuditSink};
use fe_plugin::lifecycle::{ConsentRecord as ManagerConsent, GovernanceGates, PackageFacts, TrustTier};
use fe_plugin::trust_roots::{RevocationList, RevocationSet, TrustRoots};
use fe_runtime::messages::{DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sdk::manifest_v2::ExtensionKind;
use fe_ui::verse_manager::VerseManager;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::plugin_bridge::{ExtensionExternalPolicy, ExtensionRoleView};

/// Trust roots file in the governance config dir (plugin_host_revival OD-3).
pub(crate) const TRUST_ROOTS_FILE: &str = "trusted_publishers.toml";
/// The host's own deny list and ceilings, beside the trust roots.
pub(crate) const LOCAL_POLICY_FILE: &str = "extensions.toml";
/// Overrides the governance config dir (QA runs several peers on one machine).
pub(crate) const CONFIG_DIR_ENV: &str = "FE_EXT_CONFIG_DIR";
/// Debug builds only: exactly `"1"` loads `dev` roots and admits dev-unsigned builds.
pub(crate) const DEV_FLAG_ENV: &str = "FE_EXT_DEV";
/// Replicated role statements (fe-database `role` table): their arrival re-resolves roles.
const ROLE_TABLE: &str = "role";
/// Bound on one governance DB round trip (fe-api waits 6 s).
const DB_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on enqueueing one API-path DB command.
const DB_SEND_TIMEOUT: Duration = Duration::from_secs(1);
/// A Bevy-side DB request with no answer by then is re-sent.
const DB_STALL: Duration = Duration::from_secs(30);
/// Spacing before retrying a failed load or refresh.
const RETRY_SPACING: Duration = Duration::from_secs(5);
/// Spacing before retrying when the DB queue was full.
const QUEUE_FULL_BACKOFF: Duration = Duration::from_millis(250);
/// Least time between two role sweeps.
const ROLE_SWEEP_SPACING: Duration = Duration::from_secs(2);
/// Petal scopes one frame of a sweep resolves, so a sweep never floods the DB queue.
const SWEEP_SCOPES_PER_FRAME: usize = 8;
/// Most remote DIDs the bus role feed resolves.
const MAX_WATCHED_DIDS: usize = 64;
/// Audit entries buffered ahead of the DB queue.
const AUDIT_BUFFER: usize = 4096;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Row 6 seam for licensed builds; S-ENTITLEMENT's `EntitlementVerifier` plugs in once fe-format ships it.
pub(crate) trait EntitlementCheck: Send + Sync {
    /// `Ok` when the licensed build holds a valid entitlement on this host.
    fn verify(&self, package: &PackageFacts) -> Result<(), String>;
}

/// No verifier in this build: every licensed build is refused (unlicensed builds never reach it).
pub(crate) struct NoEntitlementVerifier;

impl EntitlementCheck for NoEntitlementVerifier {
    fn verify(&self, package: &PackageFacts) -> Result<(), String> {
        Err(format!("{} is licensed and this build has no entitlement verifier", package.package_id))
    }
}

/// Where governance reads its files, whom it serves and how it treats dev builds.
pub(crate) struct GovernanceConfig {
    /// The node DID: the only identity that administers extensions, and the one they write as.
    pub local_did: String,
    /// Holds `trusted_publishers.toml`, `extensions.toml` and the revocation cache.
    pub config_dir: PathBuf,
    /// The dev flag; it only counts in debug builds.
    pub dev_flag: bool,
    /// Row 6 for licensed builds.
    pub entitlement: Arc<dyn EntitlementCheck>,
}

impl GovernanceConfig {
    /// Config from the environment: [`default_config_dir`], [`dev_flag_from_env`], no entitlement verifier.
    pub(crate) fn from_env(local_did: String, qa_root: Option<&Path>) -> Self {
        Self {
            local_did,
            config_dir: default_config_dir(qa_root),
            dev_flag: dev_flag_from_env(),
            entitlement: Arc::new(NoEntitlementVerifier),
        }
    }
}

/// `FE_EXT_CONFIG_DIR`, else `<qa_root>/extensions`, else the per-user config dir's `fractalengine/extensions`.
pub(crate) fn default_config_dir(qa_root: Option<&Path>) -> PathBuf {
    if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(root) = qa_root {
        return root.join("extensions");
    }
    dirs::config_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("fractalengine").join("extensions"))
        .unwrap_or_else(|| PathBuf::from("data").join("extensions"))
}

/// The dev flag: `FE_EXT_DEV=1` in a debug build; always false in release builds.
pub(crate) fn dev_flag_from_env() -> bool {
    cfg!(debug_assertions) && std::env::var(DEV_FLAG_ENV).ok().as_deref() == Some("1")
}

/// Read `extensions.toml`: absent = no local restriction; unreadable or invalid = `Err` (row 7 refuses all).
pub(crate) fn load_local_policy(path: &Path) -> Result<LocalPolicy, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(LocalPolicy::default()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let local: LocalPolicy = toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    // The verse row's shape rules (deny entries, kinds) apply to the local file too.
    ExtensionPolicy { deny: local.deny.clone(), ceilings: local.ceilings.clone(), ..ExtensionPolicy::default() }
        .validate("")
        .map_err(|reason| format!("{}: {reason}", path.display()))?;
    Ok(local)
}

// ---------------------------------------------------------------------------
// Trust roots and tiers
// ---------------------------------------------------------------------------

/// The loaded trust roots plus each root's key bytes and tier; roots match by key bytes, never by label.
pub(crate) struct TrustIndex {
    roots: TrustRoots,
    keys: Vec<([u8; 32], TrustTier)>,
    allow_unsigned_dev: bool,
}

impl TrustIndex {
    /// Load `trusted_publishers.toml`; a missing or malformed file leaves no root, so nothing is trusted.
    pub(crate) fn load(path: &Path, dev_flag: bool) -> Self {
        let loaded = std::fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|text| Self::from_toml_str(&text, dev_flag));
        loaded.unwrap_or_else(|reason| {
            tracing::warn!(path = %path.display(), %reason, "no extension trust roots: every package is untrusted");
            Self { roots: TrustRoots::default(), keys: Vec::new(), allow_unsigned_dev: cfg!(debug_assertions) && dev_flag }
        })
    }

    /// Parse the file text; `dev` roots load only with the dev flag in a debug build (fe-plugin `TrustRoots`).
    pub(crate) fn from_toml_str(text: &str, dev_flag: bool) -> Result<Self, String> {
        let roots = TrustRoots::from_toml_str_with(text, dev_flag).map_err(|error| error.to_string())?;
        // TrustRoots has no iterator: re-read only the key ids; `get` keeps its dev-flag and weak-key rules.
        #[derive(Deserialize)]
        struct KeyIds {
            #[serde(default)]
            publisher: Vec<KeyId>,
        }
        #[derive(Deserialize)]
        struct KeyId {
            key_id: String,
        }
        let ids: KeyIds = toml::from_str(text).map_err(|error| error.to_string())?;
        let keys = ids
            .publisher
            .iter()
            .filter_map(|id| roots.get(&id.key_id))
            .map(|key| (key.verifying_key.to_bytes(), key.tier()))
            .collect();
        Ok(Self { roots, keys, allow_unsigned_dev: cfg!(debug_assertions) && dev_flag })
    }

    /// The trust roots (revocation sources and list verification); hand the plugin manager this same set.
    pub(crate) fn roots(&self) -> &TrustRoots {
        &self.roots
    }

    /// Whether `did` (a `did:key`) is a loaded root: fe-hexon `PluginTrust::is_trust_root`.
    pub(crate) fn is_trust_root(&self, did: &str) -> bool {
        self.tier_of(did).is_some()
    }

    /// The tier of a verified build from the root that vouched (`PluginPackage::trusted_by`); `None` = untrusted.
    pub(crate) fn tier_for(&self, trusted_by: Option<&str>) -> Option<TrustTier> {
        match trusted_by {
            Some(did) => self.tier_of(did),
            None if self.allow_unsigned_dev => Some(TrustTier::DevUnsigned),
            None => None,
        }
    }

    /// A key listed under several roles gets its least-privileged tier (a release tier needs only release roles).
    fn tier_of(&self, did: &str) -> Option<TrustTier> {
        let key = fe_identity::did_key::public_key_from_did_key(did).ok()?.to_bytes();
        self.keys
            .iter()
            .filter(|(bytes, _)| *bytes == key)
            .map(|(_, tier)| *tier)
            .min_by_key(|tier| privilege(*tier))
    }
}

/// Rank used to pick the weakest tier.
fn privilege(tier: TrustTier) -> u8 {
    match tier {
        TrustTier::Publisher => 0,
        TrustTier::DevUnsigned => 1,
        TrustTier::Dev => 2,
        TrustTier::Release => 3,
    }
}

/// fe-plugin's tier as fe-database's policy evaluator names it.
fn policy_trust(tier: TrustTier) -> PolicyTrust {
    match tier {
        TrustTier::Release => PolicyTrust::Release,
        TrustTier::Dev => PolicyTrust::Dev,
        TrustTier::Publisher => PolicyTrust::Publisher,
        TrustTier::DevUnsigned => PolicyTrust::DevUnsigned,
    }
}

// ---------------------------------------------------------------------------
// The running plugin manager, as governance drives it
// ---------------------------------------------------------------------------

/// What governance changes in the plugin manager; the binary implements it over its `PluginManager` handle.
/// Implementations must not call back into governance and should hand slow work to the plugin worker.
pub(crate) trait ManagerControl: Send + Sync {
    /// `Governance::record_consent` (startup hydration and every new consent).
    fn record_consent(&self, record: ManagerConsent);
    /// `Governance::add_kill_list_override`, after the DB recorded and audited it.
    fn add_kill_list_override(&self, verse_id: &str, content_hash: &str);
    /// `PluginManager::reevaluate_verse_policy`; the instances it stopped.
    fn reevaluate_verse_policy(&self, verse_id: &str) -> Vec<String>;
    /// `PluginManager::accept_revocation_list`; the instances it stopped, or why the list was refused.
    fn accept_revocation_list(&self, list: &RevocationList) -> Result<Vec<String>, String>;
}

/// One change for the plugin manager, queued until a manager is attached.
enum ManagerOp {
    Consent(ManagerConsent),
    Override { verse_id: String, content_hash: String },
    Reevaluate(String),
    Revocation(RevocationList),
}

#[derive(Default)]
struct Wiring {
    manager: Option<Arc<dyn ManagerControl>>,
    queued: Vec<ManagerOp>,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// The verse policy rows this host knows, and which of them changed and are being re-read.
#[derive(Default)]
struct PolicyMirror {
    hydrated: bool,
    rows: HashMap<String, PolicyRow>,
    /// verse id -> the change generation a refresh must read past (row 7 refuses there until then).
    stale: HashMap<String, u64>,
    generation: u64,
}

/// Accepted revocation lists, newest per signer, and their union.
#[derive(Default)]
struct KillList {
    per_signer: HashMap<String, (([u32; 6], u32), RevocationSet)>,
    effective: RevocationSet,
}

struct GovernanceCore {
    local_did: String,
    trust: TrustIndex,
    local_policy: Result<LocalPolicy, String>,
    entitlement: Arc<dyn EntitlementCheck>,
    db: Sender<DbCommand>,
    policies: RwLock<PolicyMirror>,
    /// The node DID's resolved role per petal (row 8).
    roles: RwLock<HashMap<String, RoleLevel>>,
    /// `(verse_id, content_hash)` Owner overrides recorded on this host.
    overrides: RwLock<HashSet<(String, String)>>,
    kill_list: RwLock<KillList>,
    external: Option<ExtensionExternalPolicy>,
    role_view: Option<ExtensionRoleView>,
    /// Remote DIDs the bus role feed resolves at every known petal.
    watched: Mutex<Vec<String>>,
    /// A gate missed a role, or a DID was added: the next frame sweeps roles.
    want_roles: AtomicBool,
    wiring: Mutex<Wiring>,
    audit_tx: OnceLock<Sender<Value>>,
    audit_dropped: AtomicU64,
}

/// Host-side extension governance (cheap clone); every extension host module shares this resource.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionGovernance(Arc<GovernanceCore>);

/// What `LoadExtensionGovernance` answers (fe-database serde shapes).
#[derive(Deserialize, Default)]
struct GovernanceSnapshot {
    #[serde(default)]
    policies: Vec<PolicyRow>,
    #[serde(default)]
    consents: Vec<StoredConsent>,
    #[serde(default)]
    overrides: Vec<KillListOverride>,
}

fn read_guard<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_guard<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

fn lock_guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn refusal(code: &str, message: impl Into<String>) -> AdminRefusal {
    AdminRefusal { code: code.to_string(), message: message.into() }
}

/// A DB error's leading `code: ` (fe-database `PolicyWriteError` / `OverrideError` Display) as a refusal.
fn db_refusal(message: &str) -> AdminRefusal {
    const CODES: [&str; 4] = ["invalid-policy", "role-denied", "confirmation-mismatch", "invalid-input"];
    match message.split_once(": ") {
        Some((code, detail)) if CODES.contains(&code) => refusal(code, detail),
        _ => refusal("not-available", message),
    }
}

/// An already-settled host answer.
fn ready(result: Result<Value, AdminRefusal>) -> AdminReply {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(result);
    rx
}

/// Wait for a DB reply off the caller's thread; a dropped answer closes the reply (fe-api: 503).
fn answer_later<T: Send + 'static>(
    receiver: Receiver<T>,
    finish: impl FnOnce(Option<T>) -> Result<Value, AdminRefusal> + Send + 'static,
) -> AdminReply {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let work = move || {
        let outcome = receiver.recv_timeout(DB_REPLY_TIMEOUT).ok();
        let _ = tx.send(finish(outcome));
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(work);
        }
        Err(_) => {
            if let Err(error) = std::thread::Builder::new().name("fe-ext-governance".into()).spawn(work) {
                tracing::warn!(%error, "governance reply thread did not start");
            }
        }
    }
    rx
}

/// Block for a DB reply; on a multi-thread tokio worker the wait leaves the executor (`block_in_place`).
fn wait_blocking<T>(receiver: &Receiver<T>, timeout: Duration) -> Option<T> {
    let wait = || receiver.recv_timeout(timeout).ok();
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(wait)
        }
        _ => wait(),
    }
}

/// `issued_at` by value: strict RFC 3339 UTC `([year, month, day, hour, minute, second], nanos)`.
fn issued_instant(text: &str) -> Option<([u32; 6], u32)> {
    let body = text.strip_suffix('Z')?;
    let (main, fraction) = body.split_once('.').unwrap_or((body, ""));
    let bytes = main.as_bytes();
    if bytes.len() != 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let field = |from: usize, to: usize| {
        main.get(from..to)
            .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u32>().ok())
    };
    let stamp = [field(0, 4)?, field(5, 7)?, field(8, 10)?, field(11, 13)?, field(14, 16)?, field(17, 19)?];
    if fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let nanos = format!("{fraction:0<9}").parse::<u32>().ok()?;
    Some((stamp, nanos))
}

/// Keep the newer row by the DB's last-writer-wins key `(written_at, author_did)`; true when `row` was stored.
fn store_newest(rows: &mut HashMap<String, PolicyRow>, verse_id: &str, row: PolicyRow) -> bool {
    if row.verse_id != verse_id {
        tracing::warn!(verse_id, row_verse = %row.verse_id, "extension policy row names another verse; ignored");
        return false;
    }
    let newer = rows
        .get(verse_id)
        .map_or(true, |current| (row.written_at, row.author_did.as_str()) >= (current.written_at, current.author_did.as_str()));
    if newer && rows.get(verse_id) != Some(&row) {
        rows.insert(verse_id.to_string(), row);
        return true;
    }
    false
}

/// The petal a role scope names, if it is a petal scope.
fn petal_of(scope: &str) -> Option<String> {
    fe_database::parse_scope(scope).ok()?.petal_id
}

/// The reply shape of the policy routes: `{verse_id, policy}` (`policy: null` = `first-party-only`).
fn policy_reply(verse_id: &str, row: Option<&PolicyRow>) -> Value {
    match row {
        Some(row) => json!({
            "verse_id": verse_id,
            "policy": row.policy,
            "author_did": row.author_did,
            "written_at": row.written_at,
        }),
        None => json!({ "verse_id": verse_id, "policy": Value::Null }),
    }
}

impl ExtensionGovernance {
    /// Load the trust roots and `extensions.toml` from `config.config_dir`; nothing is admitted until hydrated.
    pub(crate) fn new(
        config: GovernanceConfig,
        db: Sender<DbCommand>,
        external: Option<ExtensionExternalPolicy>,
        role_view: Option<ExtensionRoleView>,
    ) -> Self {
        let trust = TrustIndex::load(&config.config_dir.join(TRUST_ROOTS_FILE), config.dev_flag);
        let local_policy = load_local_policy(&config.config_dir.join(LOCAL_POLICY_FILE));
        if let Err(reason) = &local_policy {
            tracing::error!(%reason, "extensions.toml is invalid: every extension is refused until it is fixed");
        }
        if config.local_did.is_empty() {
            tracing::warn!("extension governance has no local DID: nothing is administered on this host");
        }
        Self(Arc::new(GovernanceCore {
            local_did: config.local_did,
            trust,
            local_policy,
            entitlement: config.entitlement,
            db,
            policies: RwLock::new(PolicyMirror::default()),
            roles: RwLock::new(HashMap::new()),
            overrides: RwLock::new(HashSet::new()),
            kill_list: RwLock::new(KillList::default()),
            external,
            role_view,
            watched: Mutex::new(Vec::new()),
            want_roles: AtomicBool::new(false),
            wiring: Mutex::new(Wiring::default()),
            audit_tx: OnceLock::new(),
            audit_dropped: AtomicU64::new(0),
        }))
    }

    /// The node DID governance administers as.
    pub(crate) fn local_did(&self) -> &str {
        &self.0.local_did
    }

    /// The loaded trust roots and tiers.
    pub(crate) fn trust(&self) -> &TrustIndex {
        &self.0.trust
    }

    /// fe-hexon `PluginTrust::is_trust_root` / `RegistrySource` predicate over the loaded roots.
    pub(crate) fn trust_root_predicate(&self) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
        let governance = self.clone();
        Arc::new(move |did| governance.0.trust.is_trust_root(did))
    }

    /// Rows 6-8 for `PluginManager::with_gates`.
    pub(crate) fn gates(&self) -> Arc<dyn GovernanceGates> {
        Arc::new(self.clone())
    }

    /// The op-log audit writer for `BusSources::audit` and the network sources.
    pub(crate) fn audit_sink(&self) -> Arc<dyn AuditSink> {
        Arc::new(self.clone())
    }

    /// Whether the startup load (policies, consents, overrides) has landed.
    pub(crate) fn is_hydrated(&self) -> bool {
        read_guard(&self.0.policies).hydrated
    }

    /// Attach the plugin manager; queued consents, overrides, re-evaluations and lists apply in order.
    pub(crate) fn attach_manager(&self, manager: Arc<dyn ManagerControl>) {
        let mut wiring = lock_guard(&self.0.wiring);
        for op in std::mem::take(&mut wiring.queued) {
            if let Err(reason) = self.apply(manager.as_ref(), op) {
                tracing::warn!(%reason, "queued governance change refused by the plugin manager");
            }
        }
        wiring.manager = Some(manager);
    }

    /// Apply now (in order, under the wiring lock) or queue until a manager is attached.
    fn dispatch(&self, op: ManagerOp) -> Result<Vec<String>, String> {
        let mut wiring = lock_guard(&self.0.wiring);
        match wiring.manager.clone() {
            Some(manager) => self.apply(manager.as_ref(), op),
            None => {
                wiring.queued.push(op);
                Ok(Vec::new())
            }
        }
    }

    fn dispatch_logged(&self, op: ManagerOp) {
        if let Err(reason) = self.dispatch(op) {
            tracing::warn!(%reason, "governance change refused by the plugin manager");
        }
    }

    fn apply(&self, manager: &dyn ManagerControl, op: ManagerOp) -> Result<Vec<String>, String> {
        match op {
            ManagerOp::Consent(record) => {
                manager.record_consent(record);
                Ok(Vec::new())
            }
            ManagerOp::Override { verse_id, content_hash } => {
                manager.add_kill_list_override(&verse_id, &content_hash);
                Ok(Vec::new())
            }
            ManagerOp::Reevaluate(verse_id) => {
                let stopped = manager.reevaluate_verse_policy(&verse_id);
                if !stopped.is_empty() {
                    tracing::info!(verse_id, stopped = ?stopped, "verse policy change stopped extension instances");
                }
                Ok(stopped)
            }
            ManagerOp::Revocation(list) => {
                let stopped = manager.accept_revocation_list(&list)?;
                for plugin_id in &stopped {
                    let mut entry = self.entry(AuditAction::Revocation, plugin_id, AuditOutcome::Ok);
                    entry.reason = Some(format!("kill list from '{}' issued {}", list.signer_key_id, list.issued_at));
                    self.append_audit(entry);
                }
                Ok(stopped)
            }
        }
    }

    /// A local-DID audit entry about `target`.
    fn entry(&self, action: AuditAction, target: &str, outcome: AuditOutcome) -> AuditEntry {
        AuditEntry {
            action,
            installation: None,
            package_id: None,
            content_hash: None,
            actor_did: self.0.local_did.clone(),
            target: target.to_string(),
            outcome,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: String::new(),
        }
    }

    // -- Audit ------------------------------------------------------------------

    /// Append one `ExtensionAudit` entry without blocking: a forwarder thread feeds the DB queue in order.
    pub(crate) fn append_audit(&self, entry: AuditEntry) {
        let value = match serde_json::to_value(&entry) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "extension audit entry did not serialise");
                return;
            }
        };
        if let Err(error) = self.audit_queue().try_send(value) {
            let dropped = self.0.audit_dropped.fetch_add(1, Ordering::Relaxed) + 1;
            let reason = match error {
                TrySendError::Full(_) => "buffer full",
                TrySendError::Disconnected(_) => "forwarder stopped",
            };
            tracing::warn!(dropped, reason, "extension audit entry dropped");
        }
    }

    /// Audit entries lost to a full buffer or a stopped forwarder (status counter).
    pub(crate) fn audit_dropped(&self) -> u64 {
        self.0.audit_dropped.load(Ordering::Relaxed)
    }

    fn audit_queue(&self) -> &Sender<Value> {
        self.0.audit_tx.get_or_init(|| {
            let (tx, rx) = crossbeam::channel::bounded::<Value>(AUDIT_BUFFER);
            let db = self.0.db.clone();
            let spawned = std::thread::Builder::new().name("fe-ext-audit".into()).spawn(move || {
                while let Ok(entry) = rx.recv() {
                    // The append's lamport is not needed here; its receiver drops at once.
                    let (reply, _) = crossbeam::channel::bounded(1);
                    if db.send(DbCommand::AppendExtensionAudit { entry, reply }).is_err() {
                        break;
                    }
                }
            });
            if let Err(error) = spawned {
                tracing::warn!(%error, "extension audit forwarder did not start");
            }
            tx
        })
    }

    /// The newest `limit` audit entries of one installation, newest first; blocks up to 5 s.
    pub(crate) fn audit_rows(&self, installation: &str, limit: usize) -> Option<Vec<Value>> {
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let command = DbCommand::ListExtensionAudit {
            installation: Some(installation.to_string()),
            verse_id: None,
            limit,
            reply,
        };
        self.0.db.send_timeout(command, DB_SEND_TIMEOUT).ok()?;
        match wait_blocking(&receiver, DB_REPLY_TIMEOUT)? {
            Ok(rows) => Some(rows),
            Err(error) => {
                tracing::warn!(installation, %error, "extension audit read failed");
                None
            }
        }
    }

    // -- Verse policy (R18) --------------------------------------------------------

    /// `ExtensionHost::verse_policy`: the stored row, read from the DB (`policy: null` = `first-party-only`).
    pub(crate) fn verse_policy(&self, verse_id: &str) -> AdminReply {
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let command = DbCommand::GetExtensionPolicy { verse_id: verse_id.to_string(), reply };
        if self.0.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return ready(Err(refusal("not-available", "the database is not accepting commands")));
        }
        let governance = self.clone();
        let verse_id = verse_id.to_string();
        answer_later(receiver, move |outcome| {
            let stored = outcome.ok_or_else(|| refusal("not-available", "the database did not answer in time"))?;
            let row = match stored.map_err(|message| db_refusal(&message))? {
                Some(value) => Some(
                    serde_json::from_value::<PolicyRow>(value)
                        .map_err(|error| refusal("not-available", format!("stored policy row: {error}")))?,
                ),
                None => None,
            };
            let reply = policy_reply(&verse_id, row.as_ref());
            if let Some(row) = row {
                governance.learn_row(&verse_id, row);
            }
            Ok(reply)
        })
    }

    /// `ExtensionHost::set_verse_policy`: the DB writes the row AS THE NODE DID (Manager+ there), audited either way.
    pub(crate) fn set_verse_policy(&self, actor_did: &str, verse_id: &str, policy: Value) -> AdminReply {
        // Shape first: a malformed body never reaches the DB queue.
        let checked = serde_json::from_value::<ExtensionPolicy>(policy)
            .map_err(|error| error.to_string())
            .and_then(|policy| policy.validate(verse_id).map(|()| policy));
        let policy = match checked.and_then(|policy| serde_json::to_value(&policy).map_err(|error| error.to_string())) {
            Ok(policy) => policy,
            Err(reason) => return ready(Err(refusal("invalid-policy", reason))),
        };
        tracing::info!(actor = actor_did, verse_id, "verse extension policy write requested");
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let command = DbCommand::SetExtensionPolicy { verse_id: verse_id.to_string(), policy, reply };
        if self.0.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return ready(Err(refusal("not-available", "the database is not accepting commands")));
        }
        let governance = self.clone();
        let verse_id = verse_id.to_string();
        answer_later(receiver, move |outcome| {
            let written = outcome
                .ok_or_else(|| refusal("not-available", "the database did not answer; the policy may or may not be written"))?
                .map_err(|message| db_refusal(&message))?;
            let row = serde_json::from_value::<PolicyRow>(written)
                .map_err(|error| refusal("not-available", format!("written policy row: {error}")))?;
            let reply = policy_reply(&verse_id, Some(&row));
            governance.learn_row(&verse_id, row);
            Ok(reply)
        })
    }

    /// A row read or written through the API: newest wins; a change re-judges the verse's running instances,
    /// unless a refresh is pending there (row 7 refuses while stale, so re-judging now would stop them all).
    fn learn_row(&self, verse_id: &str, row: PolicyRow) {
        let (changed, stale) = {
            let mut mirror = write_guard(&self.0.policies);
            let changed = store_newest(&mut mirror.rows, verse_id, row);
            (changed, mirror.stale.contains_key(verse_id))
        };
        if changed && !stale {
            self.feed_external(verse_id);
            self.dispatch_logged(ManagerOp::Reevaluate(verse_id.to_string()));
        }
    }

    /// A replicated row arrived: row 7 refuses in this verse until a refresh reads past this generation.
    fn mark_stale(&self, verse_id: &str) -> u64 {
        let generation = {
            let mut mirror = write_guard(&self.0.policies);
            mirror.generation += 1;
            let generation = mirror.generation;
            mirror.stale.insert(verse_id.to_string(), generation);
            generation
        };
        self.feed_external(verse_id);
        generation
    }

    /// A refresh answered; true when it read past the newest arrival (stale cleared, instances re-judged).
    fn apply_refreshed(&self, verse_id: &str, generation: u64, row: Option<PolicyRow>) -> bool {
        let cleared = {
            let mut mirror = write_guard(&self.0.policies);
            // Rows are replaced, never deleted: "no row" leaves a known row in place.
            if let Some(row) = row {
                store_newest(&mut mirror.rows, verse_id, row);
            }
            let current = mirror.stale.get(verse_id) == Some(&generation);
            if current {
                mirror.stale.remove(verse_id);
            }
            current
        };
        if cleared {
            self.feed_external(verse_id);
            self.dispatch_logged(ManagerOp::Reevaluate(verse_id.to_string()));
        }
        cleared
    }

    /// Open or close external bus access for one verse from the mirror and the local policy.
    fn feed_external(&self, verse_id: &str) {
        let Some(external) = &self.0.external else { return };
        let allowed = match &self.0.local_policy {
            Ok(local) => {
                let mirror = read_guard(&self.0.policies);
                !mirror.stale.contains_key(verse_id)
                    && ExtensionPolicy::bus_external(mirror.rows.get(verse_id).map(|row| &row.policy), local)
            }
            Err(_) => false,
        };
        external.set_verse(verse_id, allowed);
    }

    /// Startup load: policies (newest wins against any refresh already applied), overrides, local consents.
    fn apply_snapshot(&self, snapshot: GovernanceSnapshot) {
        let GovernanceSnapshot { policies, consents, overrides } = snapshot;
        let verses: Vec<String> = {
            let mut mirror = write_guard(&self.0.policies);
            let verses: Vec<String> = policies.iter().map(|row| row.verse_id.clone()).collect();
            for row in policies {
                let verse_id = row.verse_id.clone();
                store_newest(&mut mirror.rows, &verse_id, row);
            }
            mirror.hydrated = true;
            verses
        };
        for verse_id in &verses {
            self.feed_external(verse_id);
        }
        {
            let mut set = write_guard(&self.0.overrides);
            for record in &overrides {
                set.insert((record.verse_id.clone(), record.content_hash.to_ascii_lowercase()));
            }
        }
        for record in overrides {
            self.dispatch_logged(ManagerOp::Override { verse_id: record.verse_id, content_hash: record.content_hash });
        }
        for record in consents {
            // A consent given under another identity (a rotated node key) never carries over.
            if record.actor_did != self.0.local_did {
                tracing::warn!(installation = %record.installation_id, "consent by another identity ignored");
                continue;
            }
            self.dispatch_logged(ManagerOp::Consent(ManagerConsent {
                installation_id: record.installation_id,
                package_id: record.package_id,
                content_hash: record.content_hash,
                capability_digest: record.capability_digest,
                actor_did: record.actor_did,
                at: record.at,
            }));
        }
        tracing::info!(verses = verses.len(), "extension governance hydrated");
    }

    /// The verse's mirrored policy row (portal `GovernanceView`).
    pub(crate) fn policy_row(&self, verse_id: &str) -> Option<PolicyRow> {
        read_guard(&self.0.policies).rows.get(verse_id).cloned()
    }

    /// Row 7: the verse policy (default `first-party-only`) AND `extensions.toml`; fails closed while unknown.
    pub(crate) fn evaluate(&self, verse_id: &str, package: &PackageFacts) -> Result<(), String> {
        let local = self
            .0
            .local_policy
            .as_ref()
            .map_err(|reason| format!("this device's extension policy is unreadable ({reason})"))?;
        let mirror = read_guard(&self.0.policies);
        if !mirror.hydrated {
            return Err("the verse extension policies are still loading".into());
        }
        if mirror.stale.contains_key(verse_id) {
            return Err(format!("the extension policy of verse {verse_id} changed and is being re-read"));
        }
        let manifest = package.manifest.as_deref();
        let facts = PolicyFacts {
            package_id: &package.package_id,
            version: &package.version,
            content_hash: &package.content_hash,
            component_digest: package.component_digest.as_deref(),
            plaintext_digest: package.plaintext_digest.as_deref(),
            trust: policy_trust(package.trust),
            kind: manifest.map_or(ExtensionKind::WebBundle, |manifest| manifest.kind),
            manifest,
        };
        policy_db::evaluate(mirror.rows.get(verse_id).map(|row| &row.policy), local, &facts)
            .map_err(|denial| denial.to_string())
    }

    // -- Kill list, override, consent ---------------------------------------------

    /// `administer(KillListOverride)`: the verse Owner (checked by the DB against `verse.created_by`), on this host,
    /// for `(verse_id, content_hash)` only; audited by the DB whether recorded or refused. Blocks up to 5 s.
    pub(crate) fn kill_list_override(&self, actor_did: &str, params: &Value) -> Result<Value, AdminRefusal> {
        if self.0.local_did.is_empty() || actor_did != self.0.local_did {
            return Err(refusal("not-local-identity", "only the host's local identity overrides the kill list"));
        }
        let request: KillListOverrideRequest = serde_json::from_value(params.clone())
            .map_err(|error| refusal("invalid-input", format!("override body: {error}")))?;
        let body = serde_json::to_value(&request).map_err(|error| refusal("invalid-input", error.to_string()))?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.0
            .db
            .send_timeout(DbCommand::RecordKillListOverride { request: body, reply }, DB_SEND_TIMEOUT)
            .map_err(|_| refusal("not-available", "the database is not accepting commands"))?;
        let answer = wait_blocking(&receiver, DB_REPLY_TIMEOUT)
            .ok_or_else(|| refusal("not-available", "the database did not answer; the override may or may not be recorded"))?;
        let recorded: KillListOverride = serde_json::from_value(answer.map_err(|message| db_refusal(&message))?)
            .map_err(|error| refusal("not-available", format!("recorded override: {error}")))?;
        // Only the pair the Owner asked for gains the override, and only on this host.
        if recorded.verse_id != request.verse_id || !recorded.content_hash.eq_ignore_ascii_case(&request.content_hash) {
            return Err(refusal("not-available", "the database recorded a different override"));
        }
        write_guard(&self.0.overrides).insert((recorded.verse_id.clone(), recorded.content_hash.to_ascii_lowercase()));
        self.dispatch_logged(ManagerOp::Override {
            verse_id: recorded.verse_id.clone(),
            content_hash: recorded.content_hash.clone(),
        });
        Ok(json!({
            "verse_id": recorded.verse_id,
            "package_id": request.package_id,
            "content_hash": recorded.content_hash,
        }))
    }

    /// Whether the verse Owner overrode the kill list for this build in this verse on this host.
    pub(crate) fn is_overridden(&self, verse_id: &str, content_hash: &str) -> bool {
        read_guard(&self.0.overrides).contains(&(verse_id.to_string(), content_hash.to_ascii_lowercase()))
    }

    /// Whether any digest is on an accepted kill list, under any package id (portal `GovernanceView`).
    pub(crate) fn is_revoked_any(&self, digests: &[&str]) -> bool {
        read_guard(&self.0.kill_list).effective.is_revoked_any(digests)
    }

    /// R23 (d): verify a list (revocation roots only), refuse one older than the signer's newest, then let the
    /// plugin manager stop what it names (an Owner override keeps its verse). Each stop is audited once.
    pub(crate) fn accept_revocation_list(&self, list: &RevocationList) -> Result<Vec<String>, String> {
        let verified = self.0.trust.roots().verify_revocation_list(list).map_err(|error| error.to_string())?;
        let issued = issued_instant(&list.issued_at)
            .ok_or_else(|| format!("revocation list issued_at '{}' is not RFC 3339 UTC", list.issued_at))?;
        {
            let mut kill = write_guard(&self.0.kill_list);
            let newest = kill.per_signer.get(&list.signer_key_id).map(|(at, _)| *at);
            match newest {
                Some(newest) if issued < newest => {
                    return Err(format!(
                        "revocation list from '{}' issued {} is older than the one in force",
                        list.signer_key_id, list.issued_at
                    ))
                }
                // The same instant twice (a re-delivery or equivocation): union, fail closed.
                Some(newest) if issued == newest => {
                    if let Some((_, set)) = kill.per_signer.get_mut(&list.signer_key_id) {
                        set.extend(verified);
                    }
                }
                _ => {
                    kill.per_signer.insert(list.signer_key_id.clone(), (issued, verified));
                }
            }
            let effective = kill.per_signer.values().fold(RevocationSet::default(), |mut union, (_, set)| {
                union.extend(set.clone());
                union
            });
            kill.effective = effective;
        }
        self.dispatch(ManagerOp::Revocation(list.clone()))
    }

    /// Record a consent the local DID gave in the dialog (one `consent` audit op), then hand it to the plugin
    /// manager. Blocks up to 5 s: call it off the frame (the ops drain).
    pub(crate) fn record_consent(&self, record: ManagerConsent) -> Result<(), String> {
        if record.actor_did != self.0.local_did {
            return Err("only the host's local identity consents".into());
        }
        let entry = AuditEntry {
            installation: Some(record.installation_id.clone()),
            package_id: Some(record.package_id.clone()),
            content_hash: Some(record.content_hash.clone()),
            capability_digest: Some(record.capability_digest.clone()),
            at: record.at.clone(),
            ..self.entry(AuditAction::Consent, &record.installation_id, AuditOutcome::Ok)
        };
        let entry = serde_json::to_value(&entry).map_err(|error| error.to_string())?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.0
            .db
            .send_timeout(DbCommand::AppendExtensionAudit { entry, reply }, DB_SEND_TIMEOUT)
            .map_err(|_| "the database is not accepting commands".to_string())?;
        wait_blocking(&receiver, DB_REPLY_TIMEOUT)
            .ok_or_else(|| "the database did not answer; consent may or may not be recorded".to_string())??;
        self.dispatch(ManagerOp::Consent(record)).map(|_| ())
    }

    // -- Roles ----------------------------------------------------------------------

    /// Let the bus role feed resolve `did` at every known petal (remote callers); capped, the local DID ignored.
    pub(crate) fn watch_did(&self, did: &str) {
        if did.is_empty() || did == self.0.local_did {
            return;
        }
        let mut watched = lock_guard(&self.0.watched);
        if watched.len() >= MAX_WATCHED_DIDS || watched.iter().any(|known| known == did) {
            return;
        }
        watched.push(did.to_string());
        self.0.want_roles.store(true, Ordering::Release);
    }

    fn watched_dids(&self) -> Vec<String> {
        lock_guard(&self.0.watched).clone()
    }

    fn set_local_role(&self, petal_id: &str, role: RoleLevel) {
        write_guard(&self.0.roles).insert(petal_id.to_string(), role);
    }

    fn set_peer_roles(&self, petal_id: &str, roles: &[(String, String)]) {
        let Some(view) = &self.0.role_view else { return };
        for (did, role) in roles {
            if did != &self.0.local_did {
                view.set_role(did, petal_id, RoleLevel::from(role.as_str()));
            }
        }
    }
}

impl GovernanceGates for ExtensionGovernance {
    fn entitlement(&self, package: &PackageFacts) -> Result<(), String> {
        if !package.licensed {
            return Ok(());
        }
        self.0.entitlement.verify(package)
    }

    fn policy(&self, verse_id: &str, package: &PackageFacts) -> Result<(), String> {
        self.evaluate(verse_id, package)
    }

    fn role(&self, petal_id: &str, writes: bool) -> Result<(), String> {
        let floor = if writes { RoleLevel::Editor } else { RoleLevel::Viewer };
        let held = read_guard(&self.0.roles).get(petal_id).copied();
        match held {
            Some(role) if role.is_at_least(floor) => Ok(()),
            Some(role) => Err(format!("this node holds {role} at petal {petal_id}; this build needs {floor} or above")),
            None => {
                // Fail closed and ask the role feed; the DB re-checks every write at use anyway.
                self.0.want_roles.store(true, Ordering::Release);
                Err(format!("this node's role at petal {petal_id} is not resolved yet"))
            }
        }
    }
}

impl AuditSink for ExtensionGovernance {
    fn record(&self, entry: BusEntry) {
        let action = match entry.action {
            // The installation's `consent` entry already records the grant (governance.md §Audit list).
            BusAction::Grant => {
                tracing::debug!(target_address = %entry.target, "bus grant covered by the consent entry");
                return;
            }
            BusAction::Subscribe => AuditAction::BusSubscribe,
            BusAction::Unsubscribe => AuditAction::BusUnsubscribe,
            // A portal `act` is a command-like write; the closed action list has no `property-write`.
            BusAction::CommandCall | BusAction::PropertyWrite => AuditAction::CommandCall,
            BusAction::Denial => AuditAction::Denial,
        };
        let outcome = match (entry.outcome.as_str(), action) {
            ("ok", _) => AuditOutcome::Ok,
            (_, AuditAction::Denial) => AuditOutcome::Denied,
            _ => AuditOutcome::Refused,
        };
        self.append_audit(AuditEntry {
            action,
            installation: entry.installation_id,
            package_id: None,
            content_hash: None,
            actor_did: entry.actor_did,
            target: entry.target,
            outcome,
            reason: (outcome != AuditOutcome::Ok).then_some(entry.outcome),
            args_digest: entry.args_digest,
            verse_id: None,
            capability_digest: None,
            at: String::new(),
        });
    }
}

// ---------------------------------------------------------------------------
// Bevy feed: hydration, policy-row arrival, role resolution
// ---------------------------------------------------------------------------

/// One verse whose policy row must be re-read.
struct PendingRefresh {
    verse_id: String,
    /// The newest change generation to read past.
    generation: u64,
    /// The request in flight, when it was sent and the generation it was sent for.
    inflight: Option<(Receiver<Result<Option<Value>, String>>, Instant, u64)>,
    not_before: Option<Instant>,
}

/// Bevy-side progress of the DB requests governance makes.
#[derive(Resource, Default)]
struct GovernanceFeed {
    hydration: Option<(Receiver<Result<Value, String>>, Instant)>,
    hydration_retry_at: Option<Instant>,
    refreshes: Vec<PendingRefresh>,
    /// petal id -> its `VERSE#..-FRACTAL#..-PETAL#..` scope.
    petal_scopes: HashMap<String, String>,
    roles_dirty: bool,
    sweep: VecDeque<String>,
    last_sweep: Option<Instant>,
}

impl GovernanceFeed {
    /// Learn scopes of petals not seen before; true when any was new.
    fn learn_petals(&mut self, verses: &VerseManager) -> bool {
        let mut learned = false;
        for verse in &verses.verses {
            for fractal in &verse.fractals {
                for petal in &fractal.petals {
                    if !self.petal_scopes.contains_key(&petal.id) {
                        let scope = fe_database::build_scope(&verse.id, Some(&fractal.id), Some(&petal.id));
                        self.petal_scopes.insert(petal.id.clone(), scope);
                        learned = true;
                    }
                }
            }
        }
        learned
    }

    fn want_refresh(&mut self, verse_id: &str, generation: u64) {
        match self.refreshes.iter_mut().find(|refresh| refresh.verse_id == verse_id) {
            Some(refresh) => refresh.generation = generation,
            None => self.refreshes.push(PendingRefresh {
                verse_id: verse_id.to_string(),
                generation,
                inflight: None,
                not_before: None,
            }),
        }
    }
}

/// Build governance from `config`, insert it and its Bevy feed; call after `plugin_bridge::install_with`.
pub(crate) fn install(app: &mut App, config: GovernanceConfig) -> ExtensionGovernance {
    let db = app.world().resource::<fe_runtime::app::DbCommandSender>().0.clone();
    let external = app.world().get_resource::<ExtensionExternalPolicy>().cloned();
    let role_view = app.world().get_resource::<ExtensionRoleView>().cloned();
    let governance = ExtensionGovernance::new(config, db, external, role_view);
    app.insert_resource(governance.clone());
    app.init_resource::<GovernanceFeed>();
    app.add_systems(Update, hydrate_governance);
    app.add_systems(Update, follow_db_results);
    app.add_systems(Update, sweep_roles);
    governance
}

/// Ask the DB once for policies, consents and overrides; retry until it answers.
fn hydrate_governance(governance: Res<ExtensionGovernance>, mut feed: ResMut<GovernanceFeed>) {
    if governance.is_hydrated() {
        return;
    }
    let feed = &mut *feed;
    let now = Instant::now();
    let polled = feed
        .hydration
        .as_ref()
        .map(|(receiver, sent_at)| (receiver.try_recv(), now.duration_since(*sent_at)));
    match polled {
        Some((Err(TryRecvError::Empty), waited)) if waited < DB_STALL => {}
        Some((outcome, _)) => {
            feed.hydration = None;
            let parsed = match outcome {
                Ok(Ok(value)) => serde_json::from_value::<GovernanceSnapshot>(value).map_err(|error| error.to_string()),
                Ok(Err(error)) => Err(error),
                Err(_) => Err("no answer from the database".to_string()),
            };
            match parsed {
                Ok(snapshot) => governance.apply_snapshot(snapshot),
                Err(reason) => {
                    tracing::warn!(%reason, "loading extension governance failed; retrying");
                    feed.hydration_retry_at = Some(now + RETRY_SPACING);
                }
            }
        }
        None if feed.hydration_retry_at.is_some_and(|at| now < at) => {}
        None => {
            let (reply, receiver) = crossbeam::channel::bounded(1);
            match governance.0.db.try_send(DbCommand::LoadExtensionGovernance { reply }) {
                Ok(()) => feed.hydration = Some((receiver, now)),
                Err(_) => feed.hydration_retry_at = Some(now + QUEUE_FULL_BACKOFF),
            }
        }
    }
}

/// Follow DB results: policy-row arrivals (stale, then refresh), role results and role changes.
fn follow_db_results(
    governance: Res<ExtensionGovernance>,
    mut feed: ResMut<GovernanceFeed>,
    mut results: MessageReader<DbResult>,
) {
    let feed = &mut *feed;
    for result in results.read() {
        match result {
            DbResult::ReplicatedRowApplied { verse_id, table, outcome: ReplicatedRowOutcome::Applied, .. } => {
                if table == policy_db::POLICY_TABLE {
                    let generation = governance.mark_stale(verse_id);
                    feed.want_refresh(verse_id, generation);
                } else if table == ROLE_TABLE {
                    feed.roles_dirty = true;
                }
            }
            DbResult::LocalRoleResolved { scope, role } => {
                if let Some(petal_id) = petal_of(scope) {
                    governance.set_local_role(&petal_id, RoleLevel::from(role.as_str()));
                }
            }
            DbResult::PeerRolesResolved { scope, roles } => {
                if let Some(petal_id) = petal_of(scope) {
                    governance.set_peer_roles(&petal_id, roles);
                }
            }
            DbResult::RoleAssigned { peer_did, .. } | DbResult::RoleRevoked { peer_did, .. } => {
                governance.watch_did(peer_did);
                feed.roles_dirty = true;
            }
            _ => {}
        }
    }
    let now = Instant::now();
    feed.refreshes.retain_mut(|refresh| drive_refresh(&governance, refresh, now));
}

/// Advance one refresh; false once it read past the newest arrival.
fn drive_refresh(governance: &ExtensionGovernance, refresh: &mut PendingRefresh, now: Instant) -> bool {
    if let Some((receiver, sent_at, sent_generation)) = refresh.inflight.take() {
        let failure = match receiver.try_recv() {
            Ok(Ok(stored)) => {
                let row = match stored.map(serde_json::from_value::<PolicyRow>).transpose() {
                    Ok(row) => row,
                    Err(error) => {
                        // Fail closed: the verse stays stale until a readable row arrives.
                        tracing::warn!(verse_id = %refresh.verse_id, %error, "stored extension policy row does not parse");
                        refresh.not_before = Some(now + RETRY_SPACING);
                        return true;
                    }
                };
                // A newer arrival during the read leaves the verse stale: read again at once.
                return !governance.apply_refreshed(&refresh.verse_id, sent_generation, row);
            }
            Ok(Err(error)) => error,
            Err(TryRecvError::Empty) if now.duration_since(sent_at) < DB_STALL => {
                refresh.inflight = Some((receiver, sent_at, sent_generation));
                return true;
            }
            Err(_) => "no answer from the database".to_string(),
        };
        tracing::warn!(verse_id = %refresh.verse_id, %failure, "extension policy refresh failed; retrying");
        refresh.not_before = Some(now + RETRY_SPACING);
        return true;
    }
    if refresh.not_before.is_some_and(|at| now < at) {
        return true;
    }
    let (reply, receiver) = crossbeam::channel::bounded(1);
    let command = DbCommand::GetExtensionPolicy { verse_id: refresh.verse_id.clone(), reply };
    match governance.0.db.try_send(command) {
        Ok(()) => refresh.inflight = Some((receiver, now, refresh.generation)),
        Err(_) => refresh.not_before = Some(now + QUEUE_FULL_BACKOFF),
    }
    true
}

/// Resolve the node's role (row 8) and watched DIDs' roles (bus view) at every known petal, a few per frame.
fn sweep_roles(
    governance: Res<ExtensionGovernance>,
    mut feed: ResMut<GovernanceFeed>,
    verses: Option<Res<VerseManager>>,
) {
    let feed = &mut *feed;
    if let Some(verses) = verses.filter(|verses| verses.is_changed()) {
        if feed.learn_petals(&verses) {
            feed.roles_dirty = true;
        }
    }
    if governance.0.want_roles.swap(false, Ordering::AcqRel) {
        feed.roles_dirty = true;
    }
    let now = Instant::now();
    let spaced = feed.last_sweep.map_or(true, |at| now.duration_since(at) >= ROLE_SWEEP_SPACING);
    if feed.sweep.is_empty() && feed.roles_dirty && spaced {
        feed.sweep = feed.petal_scopes.values().cloned().collect();
        feed.roles_dirty = false;
        feed.last_sweep = Some(now);
    }
    if feed.sweep.is_empty() {
        return;
    }
    let watched = governance.watched_dids();
    for _ in 0..SWEEP_SCOPES_PER_FRAME {
        let Some(scope) = feed.sweep.pop_front() else { break };
        if governance.0.db.try_send(DbCommand::ResolveLocalRole { scope: scope.clone() }).is_err() {
            feed.sweep.push_front(scope);
            break;
        }
        if !watched.is_empty() {
            let peers = DbCommand::ResolveRolesForPeers { scope, peer_dids: watched.clone() };
            if governance.0.db.try_send(peers).is_err() {
                // Peer roles catch up on the next sweep; the bus keeps them fail-closed until then.
                feed.roles_dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! One journey through the host seams: the DB thread is faked at its command channel, policy judgement is
    //! fe-database's real `evaluate`, trust tiers come from a real trust-roots file.
    use super::*;
    use ed25519_dalek::SigningKey;
    use fe_plugin::bus::BusPolicy;

    const LOCAL: &str = "did:key:zLocalNode";
    const VERSE: &str = "01JVERSE";
    const OTHER_VERSE: &str = "01JOTHER";
    const PETAL: &str = "01JPETAL";

    #[derive(Default)]
    struct RecordingManager(Mutex<Vec<String>>);

    impl RecordingManager {
        fn ops(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    impl ManagerControl for RecordingManager {
        fn record_consent(&self, record: ManagerConsent) {
            self.0.lock().unwrap().push(format!("consent {}", record.installation_id));
        }
        fn add_kill_list_override(&self, verse_id: &str, content_hash: &str) {
            self.0.lock().unwrap().push(format!("override {verse_id} {content_hash}"));
        }
        fn reevaluate_verse_policy(&self, verse_id: &str) -> Vec<String> {
            self.0.lock().unwrap().push(format!("reevaluate {verse_id}"));
            Vec::new()
        }
        fn accept_revocation_list(&self, list: &RevocationList) -> Result<Vec<String>, String> {
            self.0.lock().unwrap().push(format!("revocation {}", list.issued_at));
            Ok(Vec::new())
        }
    }

    fn did_of(seed: u8) -> String {
        fe_format::verifying_key_to_did(&SigningKey::from_bytes(&[seed; 32]).verifying_key())
    }

    fn key_hex(seed: u8) -> String {
        hex::encode(SigningKey::from_bytes(&[seed; 32]).verifying_key().to_bytes())
    }

    fn build(package_id: &str, content_hash: &str, trust: TrustTier) -> PackageFacts {
        PackageFacts {
            package_id: package_id.into(),
            version: "1.0.0".into(),
            content_hash: content_hash.into(),
            component_digest: Some("c".repeat(64)),
            plaintext_digest: None,
            capability_digest: "d".repeat(64),
            trust,
            writes: false,
            licensed: false,
            manifest: None,
        }
    }

    fn policy_row(verse_id: &str, written_at: u64, policy: Value) -> Value {
        json!({"schema_version": 1, "verse_id": verse_id, "author_did": "did:key:zOwner", "written_at": written_at, "policy": policy})
    }

    /// The next queued DB command `pick` accepts; others (role sweeps, audits) are dropped.
    fn take<T>(db: &Receiver<DbCommand>, mut pick: impl FnMut(DbCommand) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Ok(command) = db.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if let Some(found) = pick(command) {
                return found;
            }
        }
        panic!("the expected DB command never arrived");
    }

    #[test]
    fn governance_journey_gates_policy_rows_roles_and_the_owner_override() {
        let dir = tempfile::tempdir().unwrap();
        // Seed 1 is a release root; seed 2 a publisher that a second entry also lists as release.
        let roots = format!(
            "[[publisher]]\nkey_id = \"release\"\npublic_key = \"{}\"\nrole = \"release\"\n\n\
             [[publisher]]\nkey_id = \"vendor\"\npublic_key = \"{}\"\n\n\
             [[publisher]]\nkey_id = \"vendor-as-release\"\npublic_key = \"{}\"\nrole = \"release\"\n",
            key_hex(1),
            key_hex(2),
            key_hex(2)
        );
        std::fs::write(dir.path().join(TRUST_ROOTS_FILE), roots).unwrap();
        std::fs::write(dir.path().join(LOCAL_POLICY_FILE), "deny = [\"org.example.retired\"]\n").unwrap();

        let (db_tx, db_rx) = crossbeam::channel::unbounded();
        let mut app = App::new();
        app.add_message::<DbResult>();
        app.insert_resource(fe_runtime::app::DbCommandSender(db_tx));
        app.insert_resource(ExtensionExternalPolicy::default());
        let external = app.world().resource::<ExtensionExternalPolicy>().clone();
        let config = GovernanceConfig {
            local_did: LOCAL.into(),
            config_dir: dir.path().to_path_buf(),
            dev_flag: false,
            entitlement: Arc::new(NoEntitlementVerifier),
        };
        let governance = install(&mut app, config);
        let gates = governance.gates();

        // Tiers come from the vouching root's key; a key listed as publisher never gains release.
        let trust = governance.trust();
        assert_eq!(trust.tier_for(Some(&did_of(1))), Some(TrustTier::Release));
        assert_eq!(trust.tier_for(Some(&did_of(2))), Some(TrustTier::Publisher));
        assert_eq!(trust.tier_for(Some(&did_of(3))), None, "an unknown signer is untrusted");
        assert_eq!(trust.tier_for(None), None, "unsigned needs the dev flag");

        let vendor_hash = "a".repeat(64);
        let vendor = build("org.vendor.tool", &vendor_hash, TrustTier::Publisher);
        let release = build("org.fractalengine.fleet", &"b".repeat(64), TrustTier::Release);
        let retired = build("org.example.retired", &"e".repeat(64), TrustTier::Release);
        let licensed = PackageFacts { licensed: true, ..release.clone() };

        // Before the startup load lands, row 7 refuses everything (fail closed).
        assert!(gates.policy(OTHER_VERSE, &release).unwrap_err().contains("loading"));
        assert!(gates.entitlement(&licensed).unwrap_err().contains("no entitlement verifier"));
        assert!(gates.entitlement(&release).is_ok(), "unlicensed builds never reach the verifier");

        app.update();
        let load = take(&db_rx, |command| match command {
            DbCommand::LoadExtensionGovernance { reply } => Some(reply),
            _ => None,
        });
        let allow_vendor = json!({"mode": "allow-list", "allow": [{"package_id": "org.vendor.tool", "content_hash": vendor_hash}], "ceilings": {"bus_external": true}});
        let consent = |installation: &str, actor: &str| {
            json!({"installation_id": installation, "package_id": "org.vendor.tool", "content_hash": vendor_hash, "capability_digest": "d".repeat(64), "actor_did": actor, "at": "2026-11-09T00:00:00Z"})
        };
        load.send(Ok(json!({
            "policies": [policy_row(VERSE, 10, allow_vendor)],
            "consents": [consent("inst-local", LOCAL), consent("inst-old-key", "did:key:zRotated")],
            "overrides": [{"verse_id": VERSE, "content_hash": "f".repeat(64)}],
        })))
        .unwrap();
        let manager = Arc::new(RecordingManager::default());
        governance.attach_manager(manager.clone());
        app.update();

        // Hydrated: the verse's allow-list admits the vendor build; an unknown verse is first-party-only;
        // this device's deny list refuses even a release build; external bus access follows the row.
        assert!(governance.is_hydrated());
        assert!(gates.policy(VERSE, &vendor).is_ok());
        assert!(gates.policy(OTHER_VERSE, &vendor).unwrap_err().contains("first-party-only"));
        assert!(gates.policy(OTHER_VERSE, &release).is_ok());
        assert!(gates.policy(OTHER_VERSE, &retired).unwrap_err().contains("local policy: deny-listed"));
        assert!(external.external_access_allowed(VERSE));
        assert!(!external.external_access_allowed(OTHER_VERSE));
        assert!(governance.is_overridden(VERSE, &"F".repeat(64)));
        assert_eq!(
            manager.ops(),
            [format!("override {VERSE} {}", "f".repeat(64)), "consent inst-local".to_string()],
            "a consent recorded under a rotated key never carries over"
        );

        // A replicated policy row: refused while re-read, then the new row rules and running instances are re-judged.
        app.world_mut().write_message(DbResult::ReplicatedRowApplied {
            verse_id: VERSE.into(),
            table: policy_db::POLICY_TABLE.into(),
            record_id: VERSE.into(),
            outcome: ReplicatedRowOutcome::Applied,
        });
        app.update();
        assert!(gates.policy(VERSE, &vendor).unwrap_err().contains("re-read"));
        assert!(!external.external_access_allowed(VERSE), "external access closes while the row is re-read");
        let (refreshed_verse, refresh) = take(&db_rx, |command| match command {
            DbCommand::GetExtensionPolicy { verse_id, reply } => Some((verse_id, reply)),
            _ => None,
        });
        assert_eq!(refreshed_verse, VERSE);
        refresh.send(Ok(Some(policy_row(VERSE, 20, json!({"mode": "allow-list", "allow": []}))))).unwrap();
        app.update();
        assert!(gates.policy(VERSE, &vendor).unwrap_err().contains("no allow entry"));
        assert!(!external.external_access_allowed(VERSE));
        assert!(manager.ops().contains(&format!("reevaluate {VERSE}")));

        // Row 8: an unresolved role fails closed; the node's resolved role decides.
        assert!(gates.role(PETAL, false).unwrap_err().contains("not resolved"));
        app.world_mut().write_message(DbResult::LocalRoleResolved {
            scope: format!("VERSE#{VERSE}-FRACTAL#01JFRACTAL-PETAL#{PETAL}"),
            role: "viewer".into(),
        });
        app.update();
        assert!(gates.role(PETAL, false).is_ok());
        assert!(gates.role(PETAL, true).unwrap_err().contains("viewer"));

        // The Owner override: only the local identity asks; DB refusal codes surface; a recorded override
        // reaches this verse on this host and nothing else.
        let hash = "9".repeat(64);
        let params = |confirm: &str| json!({"verse_id": VERSE, "package_id": "org.vendor.tool", "content_hash": hash, "confirm": confirm});
        assert_eq!(governance.kill_list_override("did:key:zRemote", &params("org.vendor.tool")).unwrap_err().code, "not-local-identity");
        let asking = governance.clone();
        let mismatch = params("org.vendor");
        let worker = std::thread::spawn(move || asking.kill_list_override(LOCAL, &mismatch));
        take(&db_rx, |command| match command {
            DbCommand::RecordKillListOverride { reply, .. } => Some(reply),
            _ => None,
        })
        .send(Err("confirmation-mismatch: type the package id exactly to override the kill list".into()))
        .unwrap();
        assert_eq!(worker.join().unwrap().unwrap_err().code, "confirmation-mismatch");

        let asking = governance.clone();
        let matching = params("org.vendor.tool");
        let worker = std::thread::spawn(move || asking.kill_list_override(LOCAL, &matching));
        let (request, reply) = take(&db_rx, |command| match command {
            DbCommand::RecordKillListOverride { request, reply } => Some((request, reply)),
            _ => None,
        });
        assert_eq!(request["confirm"], "org.vendor.tool");
        reply.send(Ok(json!({"verse_id": VERSE, "content_hash": hash}))).unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["verse_id"], VERSE);
        assert!(governance.is_overridden(VERSE, &hash));
        assert!(!governance.is_overridden(OTHER_VERSE, &hash), "the override never reaches another verse");
        assert!(manager.ops().contains(&format!("override {VERSE} {hash}")));
    }
}
