//! Wasm plugin lifecycle ops (flow: src/AGENTS.md §lifecycle), extension governance: R19 activation
//! preconditions and R23 kill-switch arrival (src/AGENTS.md §governance), and the 2.0 instance
//! start / update / uninstall path (src/AGENTS.md §update).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crossbeam::channel::Sender;
use fe_sdk::manifest_v2::ExtensionManifest;

use crate::capability::{CapabilityManifest, CapabilityToken};
use crate::package::{payload_hash, verify_package, PackageError, SignatureEntry, UnsignedPolicy};
use crate::registry_client::{RegistryClient, RegistryInstallError};
use crate::tick::{DeactivateReason, GuestLifecycle, Launch, TickHost, TickState, UpdateFailure};
use crate::trust_roots::{RevocationError, RevocationList, RevocationSet, TrustRoots};
use crate::registry::{PluginRegistry, PluginState, PluginTier};
use crate::wasm::aot::AotCache;
use crate::wasm::component::InstanceHandle;
use crate::wasm::fuel::FuelBudget;
use crate::wasm::{PluginStoreData, WasmEngine, WasmPlugin};
use crate::PluginCommand;

// ---------------------------------------------------------------------------
// Governance (docs/extensions/governance.md §Activation preconditions, §Kill switch layers)
// ---------------------------------------------------------------------------

/// Host-wide stop switch: exactly `"1"` stops every instance `disabled` and answers `host-off`.
pub const EXT_DISABLE_ALL_ENV: &str = "FE_EXT_DISABLE_ALL";

/// Whether an env value switches Disable all on (exactly `"1"`, like [`crate::PLUGIN_HOST_ENV`]).
pub fn ext_disable_all(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Read [`EXT_DISABLE_ALL_ENV`]; every new [`PluginManager`] starts from it.
pub fn ext_disable_all_from_env() -> bool {
    let on = ext_disable_all(std::env::var(EXT_DISABLE_ALL_ENV).ok().as_deref());
    if on {
        tracing::warn!("{EXT_DISABLE_ALL_ENV}=1: every extension instance is disabled on this host");
    }
    on
}

/// The R19 activation preconditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precondition {
    HostOff,
    NotLocalIdentity,
    PackageInvalid,
    UntrustedSigner,
    Revoked,
    EntitlementRequired,
    PolicyDenied,
    RoleDenied,
    ConsentMissing,
}

impl Precondition {
    /// R19 check order; a non-local actor is judged on `NotLocalIdentity` before `HostOff`.
    pub const ORDER: [Self; 9] = [
        Self::HostOff,
        Self::NotLocalIdentity,
        Self::PackageInvalid,
        Self::UntrustedSigner,
        Self::Revoked,
        Self::EntitlementRequired,
        Self::PolicyDenied,
        Self::RoleDenied,
        Self::ConsentMissing,
    ];

    /// The stable refusal code REST, the GUI and the audit trail show.
    pub const fn code(self) -> &'static str {
        match self {
            Self::HostOff => "host-off",
            Self::NotLocalIdentity => "not-local-identity",
            Self::PackageInvalid => "package-invalid",
            Self::UntrustedSigner => "untrusted-signer",
            Self::Revoked => "revoked",
            Self::EntitlementRequired => "entitlement-required",
            Self::PolicyDenied => "policy-denied",
            Self::RoleDenied => "role-denied",
            Self::ConsentMissing => "consent-missing",
        }
    }
}

/// The first failing precondition and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationRefusal {
    pub precondition: Precondition,
    pub detail: String,
}

impl std::fmt::Display for ActivationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.precondition.code(), self.detail)
    }
}

impl std::error::Error for ActivationRefusal {}

fn refusal<T>(precondition: Precondition, detail: impl Into<String>) -> Result<T, ActivationRefusal> {
    Err(ActivationRefusal { precondition, detail: detail.into() })
}

/// How a package is trusted (the verifying root's `role`, governance.md §Trust roots).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTier {
    Release,
    Dev,
    Publisher,
    DevUnsigned,
}

/// What governance needs to know about one verified build.
#[derive(Debug, Clone)]
pub struct PackageFacts {
    pub package_id: String,
    pub version: String,
    /// blake3 of the canonical signed manifest.
    pub content_hash: String,
    /// blake3 of `plugin/component.wasm`.
    pub component_digest: Option<String>,
    /// Licensed body phase only.
    pub plaintext_digest: Option<String>,
    /// The consent digest (manifest-v2.md §Capability digest).
    pub capability_digest: String,
    pub trust: TrustTier,
    /// Declares scene writes or readings: the role floor is Editor+ (Viewer+ otherwise).
    pub writes: bool,
    /// A `paid` licence: precondition 6 asks S-ENTITLEMENT.
    pub licensed: bool,
    /// The extension manifest (`None` for a web bundle).
    pub manifest: Option<Arc<ExtensionManifest>>,
}

impl PackageFacts {
    /// Whether a manifest writes (any `scene.write` kind, or a readings grant).
    pub fn manifest_writes(manifest: &ExtensionManifest) -> bool {
        manifest.readings.is_some()
            || manifest
                .scene
                .as_ref()
                .and_then(|scene| scene.write.as_ref())
                .is_some_and(|kinds| !kinds.is_empty())
    }

    /// Every digest a kill-list or deny entry may name for this build.
    pub fn hashes(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.content_hash.as_str())
            .chain(self.component_digest.as_deref())
            .chain(self.plaintext_digest.as_deref())
    }

    /// Whether `set` names any of this build's digests, under any package id.
    fn is_revoked_in(&self, set: &RevocationSet) -> bool {
        set.is_revoked_any(&self.hashes().collect::<Vec<_>>())
    }
}

/// How the caller's sealed-package verification ended (fe-hexon `verify_plugin_package`).
#[derive(Debug, Clone)]
pub enum PackageCheck {
    Verified(PackageFacts),
    /// Seal, integrity, identity or manifest v2 failure (row 3).
    Invalid(String),
    /// Unsigned without the dev flag, or no trust root vouches (row 4).
    Untrusted(String),
}

/// One activation, install, bind or enable to judge.
#[derive(Debug, Clone, Copy)]
pub struct ActivationRequest<'a> {
    pub actor_did: &'a str,
    pub installation_id: &'a str,
    pub verse_id: &'a str,
    pub petal_id: &'a str,
    pub package: &'a PackageCheck,
}

/// Rows 6-8, which need state this crate does not hold; the binary implements them.
pub trait GovernanceGates: Send + Sync {
    /// Row 6: a `paid` licence has a valid token through S-ENTITLEMENT's `EntitlementVerifier`.
    fn entitlement(&self, package: &PackageFacts) -> Result<(), String>;
    /// Row 7: the verse policy and the local policy both admit it (fe-database `extension_policy::evaluate`).
    fn policy(&self, verse_id: &str, package: &PackageFacts) -> Result<(), String>;
    /// Row 8: the local DID holds Editor+ at the petal for a writing build, Viewer+ otherwise.
    fn role(&self, petal_id: &str, writes: bool) -> Result<(), String>;
}

/// Refuses rows 7-8 (and licensed builds at row 6) until the binary wires real gates.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedGates;

impl GovernanceGates for FailClosedGates {
    fn entitlement(&self, package: &PackageFacts) -> Result<(), String> {
        if package.licensed {
            Err("no entitlement verifier on this host".into())
        } else {
            Ok(())
        }
    }
    fn policy(&self, _verse_id: &str, _package: &PackageFacts) -> Result<(), String> {
        Err("no verse policy source on this host".into())
    }
    fn role(&self, _petal_id: &str, _writes: bool) -> Result<(), String> {
        Err("no role source on this host".into())
    }
}

/// A local consent record (never replicated); fe-database keeps the durable copy in the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRecord {
    pub installation_id: String,
    pub package_id: String,
    pub content_hash: String,
    pub capability_digest: String,
    pub actor_did: String,
    pub at: String,
}

/// How an instance came to run; both kinds share the one instance per `(petal, package)` (bindings.md §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceOrigin {
    /// The operator's per-petal local bind (Enable); implies acceptance in its verse (bindings.md §5.3).
    LocalBind,
    /// Started by the binding activator under [`binding_instance_id`].
    Binding,
}

/// The tick, context and seat id of a binding-started instance: one per `(installation, petal)`.
pub fn binding_instance_id(installation_id: &str, petal_id: &str) -> String {
    format!("{installation_id}@{petal_id}")
}

/// Where a governed instance runs and which build it is (kill-switch arrival matches against this).
#[derive(Debug, Clone)]
struct RunningBuild {
    installation_id: String,
    verse_id: String,
    petal_id: String,
    origin: InstanceOrigin,
    package: PackageFacts,
}

/// A governed instance as the binding activator sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningInstance {
    pub plugin_id: String,
    pub installation_id: String,
    pub package_id: String,
    pub version: String,
    pub content_hash: String,
    pub verse_id: String,
    pub petal_id: String,
    pub origin: InstanceOrigin,
    /// A fault degraded it; nothing restarts it until the operator's Enable.
    pub degraded: bool,
}

/// `issued_at` as a sortable value: `[year, month, day, hour, minute, second]` plus nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct IssuedAt([u32; 6], u32);

/// Strict RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SS[.fraction]Z`), compared by value, not by string.
fn parse_issued_at(text: &str) -> Option<IssuedAt> {
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
    Some(IssuedAt(stamp, nanos))
}

/// Why a revocation list was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum RevocationIntakeError {
    #[error(transparent)]
    Verify(#[from] RevocationError),
    #[error("revocation list issued_at '{0}' is not RFC 3339 UTC")]
    BadIssuedAt(String),
    /// A replayed older list would silently un-revoke; it is refused.
    #[error("revocation list from '{signer}' issued {issued_at} is older than the one already accepted")]
    Stale { signer: String, issued_at: String },
}

/// Largest revocation list one OD-8 pull accepts.
pub const REVOCATION_LIST_MAX_BYTES: usize = 1024 * 1024;

/// Whether a pull URL is https, or plain http to an exact loopback host (QA's loopback trust root).
fn is_pull_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or(authority)
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    !authority.contains('@') && matches!(host, "127.0.0.1" | "[::1]" | "localhost")
}

/// OD-8 pull of one revocation-capable root's list (anonymous, bounded); [`PluginManager::accept_revocation_list`]
/// verifies it and applies the newest-per-signer rule.
pub async fn fetch_revocation_list(client: &reqwest::Client, url: &str) -> Result<RevocationList, String> {
    if !is_pull_url(url) {
        return Err(format!("revocation_url must be https (or loopback http): {url}"));
    }
    let mut response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("revocation pull failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("revocation pull answered {}", response.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("revocation pull failed: {e}"))? {
        body.extend_from_slice(&chunk);
        if body.len() > REVOCATION_LIST_MAX_BYTES {
            return Err("revocation list exceeds 1 MiB".into());
        }
    }
    serde_json::from_slice(&body).map_err(|e| format!("revocation list does not parse: {e}"))
}

/// The newest verified list per revocation-capable signer, plus a startup seed; the effective set is their union.
#[derive(Debug, Clone, Default)]
struct RevocationIntake {
    seed: RevocationSet,
    lists: HashMap<String, (IssuedAt, RevocationSet)>,
}

impl RevocationIntake {
    fn effective(&self) -> RevocationSet {
        let mut set = self.seed.clone();
        for (_, list) in self.lists.values() {
            set.extend(list.clone());
        }
        set
    }
}

/// Host-local governance state: Disable all, consents, acceptances, local disables, Owner kill-list overrides,
/// running builds.
pub struct Governance {
    local_did: String,
    disable_all: bool,
    gates: Arc<dyn GovernanceGates>,
    consents: HashMap<String, ConsentRecord>,
    /// `(installation_id, verse_id)`: "Allow in <verse>" on this host (bindings.md §5.3); never replicated.
    accepted: HashSet<(String, String)>,
    /// Kill layer a: an installation id (every petal) or one instance id, until the operator's Enable.
    disabled: HashSet<String>,
    /// `(verse_id, content_hash)`: the verse Owner's override on this host (bindings.md §5.7).
    overrides: HashSet<(String, String)>,
    intake: RevocationIntake,
    running: HashMap<String, RunningBuild>,
}

impl Default for Governance {
    fn default() -> Self {
        Self {
            local_did: String::new(),
            disable_all: false,
            gates: Arc::new(FailClosedGates),
            consents: HashMap::new(),
            accepted: HashSet::new(),
            disabled: HashSet::new(),
            overrides: HashSet::new(),
            intake: RevocationIntake::default(),
            running: HashMap::new(),
        }
    }
}

impl Governance {
    /// The identity extensions write as; empty means nothing is local (fail closed).
    pub fn local_did(&self) -> &str {
        &self.local_did
    }

    /// Whether Disable all (or `FE_EXT_DISABLE_ALL=1`) is on.
    pub fn is_disabled_all(&self) -> bool {
        self.disable_all
    }

    /// Record (or replace) an installation's consent; hydrated from fe-database `load_consents` at startup.
    pub fn record_consent(&mut self, record: ConsentRecord) {
        self.consents.insert(record.installation_id.clone(), record);
    }

    /// Drop an installation's consent; the caller stops its instances (`consent-revoked`).
    pub fn revoke_consent(&mut self, installation_id: &str) -> Option<ConsentRecord> {
        self.consents.remove(installation_id)
    }

    /// The installation's consent, if any.
    pub fn consent(&self, installation_id: &str) -> Option<&ConsentRecord> {
        self.consents.get(installation_id)
    }

    /// Record the verse Owner's audited override (fe-database `record_kill_list_override` checked who asked).
    pub fn add_kill_list_override(&mut self, verse_id: &str, content_hash: &str) {
        self.overrides.insert((verse_id.to_string(), content_hash.to_ascii_lowercase()));
    }

    fn overridden(&self, verse_id: &str, content_hash: &str) -> bool {
        self.overrides.contains(&(verse_id.to_string(), content_hash.to_ascii_lowercase()))
    }

    /// Whether the Owner's override for `(verse_id, content_hash)` exists on this host (resolver `kill_override`).
    pub fn is_overridden(&self, verse_id: &str, content_hash: &str) -> bool {
        self.overridden(verse_id, content_hash)
    }

    /// Record "Allow in <verse>" (fe-database `record_acceptance` audited it; hydrated from `load_acceptances`).
    pub fn accept_in_verse(&mut self, installation_id: &str, verse_id: &str) {
        self.accepted.insert((installation_id.to_string(), verse_id.to_string()));
    }

    /// Withdraw an acceptance; the binding activator then stops its instances in that verse `unbound`.
    pub fn withdraw_acceptance(&mut self, installation_id: &str, verse_id: &str) -> bool {
        self.accepted.remove(&(installation_id.to_string(), verse_id.to_string()))
    }

    /// Accepted in the verse explicitly, or implied by a running local bind of it there (bindings.md §5.3).
    pub fn is_accepted(&self, installation_id: &str, verse_id: &str) -> bool {
        self.accepted.contains(&(installation_id.to_string(), verse_id.to_string()))
            || self.running.values().any(|build| {
                build.origin == InstanceOrigin::LocalBind
                    && build.installation_id == installation_id
                    && build.verse_id == verse_id
            })
    }

    /// Every `(installation_id, verse_id)` accepted now, explicit or implied (resolver `LocalFacts::accepted`).
    pub fn acceptances(&self) -> BTreeSet<(String, String)> {
        let implied = self
            .running
            .values()
            .filter(|build| build.origin == InstanceOrigin::LocalBind)
            .map(|build| (build.installation_id.clone(), build.verse_id.clone()));
        self.accepted.iter().cloned().chain(implied).collect()
    }

    /// Kill layer a for one instance: the installation is disabled, or this petal's binding instance is.
    pub fn is_locally_disabled(&self, installation_id: &str, petal_id: Option<&str>) -> bool {
        self.disabled.contains(installation_id)
            || petal_id.is_some_and(|petal| self.disabled.contains(&binding_instance_id(installation_id, petal)))
    }

    /// Plugin ids of governed instances running in `verse_id`.
    pub fn running_in(&self, verse_id: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .running
            .iter()
            .filter(|(_, build)| build.verse_id == verse_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }
}

// ---------------------------------------------------------------------------
// 2.0 instances: start, update (P4.3), uninstall (src/AGENTS.md §update)
// ---------------------------------------------------------------------------

/// A 2.0 build ready to start: instantiated, its context staged on the `TickHost`'s contexts, nothing run yet.
pub struct StagedInstance {
    /// The context and `TickHost` key; the same for every build of one installation.
    pub plugin_id: String,
    /// blake3 of the `plugin/component.wasm` the instance was built from (`ExtensionComponent::digest`).
    pub component_digest: String,
    pub launch: Launch,
}

/// The staged build must be the one R19 judged: its component digest, its context and its seat's scope.
fn bind_staged(
    request: &ActivationRequest<'_>,
    package: &PackageFacts,
    staged: &StagedInstance,
) -> Result<(), LifecycleError> {
    let judged = package
        .component_digest
        .as_deref()
        .is_some_and(|digest| digest.eq_ignore_ascii_case(&staged.component_digest));
    if !judged {
        return Err(LifecycleError::PayloadMismatch(staged.plugin_id.clone()));
    }
    let context = &staged.launch.context;
    if context.plugin_id() != staged.plugin_id || context.petal_id() != request.petal_id {
        return Err(LifecycleError::LaunchMismatch(format!("{}: the context is bound to another instance or petal", staged.plugin_id)));
    }
    let seat_matches = staged.launch.seat.as_ref().is_none_or(|seat| {
        let spec = &seat.spec;
        spec.installation_id == request.installation_id
            && spec.verse_id == request.verse_id
            && spec.petal_id == request.petal_id
            && spec.identity_did == request.actor_did
            && spec.manifest.id == package.package_id
            && spec.content_hash.eq_ignore_ascii_case(&package.content_hash)
    });
    if !seat_matches {
        return Err(LifecycleError::LaunchMismatch(format!("{}: the bus seat names another installation, scope or build", staged.plugin_id)));
    }
    Ok(())
}

/// The component's lifecycle exports as `TickHost` runs them; a hook fault degrades the instance and refuses.
impl GuestLifecycle for InstanceHandle {
    fn on_install(&mut self) -> Result<(), String> {
        self.lock()?.on_install().map_err(|fault| fault.to_string())?
    }

    fn on_activate(&mut self) -> Result<(), String> {
        self.lock()?.on_activate().map_err(|fault| fault.to_string())?
    }

    fn on_uninstall(&mut self) {
        let outcome = self.lock().and_then(|mut instance| instance.on_uninstall().map_err(|fault| fault.to_string()));
        if let Err(error) = outcome {
            tracing::warn!(%error, "extension on-uninstall faulted");
        }
    }

    fn degraded(&self) -> Option<String> {
        self.lock().ok()?.degraded().map(str::to_string)
    }
}

// ---------------------------------------------------------------------------
// InstalledPlugin — tracks a Wasm plugin's runtime state
// ---------------------------------------------------------------------------

/// A Wasm plugin that has been loaded into the engine.
///
/// Contains the compiled module, the store for execution, and the
/// instance for calling exports.
pub struct InstalledPlugin {
    /// The compiled Wasm module.
    pub plugin: WasmPlugin,
    /// The wasmtime store holding plugin state.
    pub store: wasmtime::Store<PluginStoreData>,
    /// The instantiated module.
    pub instance: wasmtime::Instance,
}

/// What install verified; activation runs only these bytes under this manifest.
struct VerifiedInstall {
    content_hash: String,
    manifest: CapabilityManifest,
}

// ---------------------------------------------------------------------------
// Plugin manager
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of Wasm plugins.
///
/// Wraps a [`WasmEngine`], [`AotCache`], and [`PluginRegistry`] to
/// provide a unified interface for plugin management.
pub struct PluginManager {
    /// The Wasmtime engine for compilation and execution.
    engine: Arc<WasmEngine>,
    /// AOT compilation cache for fast reloading.
    aot_cache: AotCache,
    /// Reference to the Bevy plugin registry.
    registry: Arc<std::sync::Mutex<PluginRegistry>>,
    /// Installed plugin instances keyed by plugin_id.
    installed: std::collections::HashMap<String, InstalledPlugin>,
    /// Command sender for plugin -> engine communication.
    cmd_tx: Sender<PluginCommand>,
    contexts: crate::scene_host::PluginContexts,
    trust_roots: TrustRoots,
    unsigned_policy: UnsignedPolicy,
    revocations: RevocationSet,
    /// Package admission switch (src/AGENTS.md §admission-gate); off refuses install and activation.
    admission: bool,
    verified: std::collections::HashMap<String, VerifiedInstall>,
    governance: Governance,
    /// The worker's scheduler; a stop leaves the bus first and drains the in-flight commit (OD-E1).
    ticks: Option<TickHost>,
    /// Every instance id ever started per installation, so uninstall forgets stopped binding instances too.
    instance_ids: HashMap<String, BTreeSet<String>>,
}

impl PluginManager {
    /// Create a new plugin manager with an AOT cache rooted at `cache_dir`.
    pub fn new(
        engine: Arc<WasmEngine>,
        cache_dir: impl Into<PathBuf>,
        registry: Arc<std::sync::Mutex<PluginRegistry>>,
        cmd_tx: Sender<PluginCommand>,
    ) -> Result<Self, std::io::Error> {
        let aot_cache = AotCache::new(cache_dir)?;
        Ok(Self {
            engine,
            aot_cache,
            registry,
            installed: std::collections::HashMap::new(),
            cmd_tx,
            contexts: crate::scene_host::PluginContexts::default(),
            trust_roots: TrustRoots::default(),
            unsigned_policy: UnsignedPolicy::default(),
            revocations: RevocationSet::default(),
            admission: false,
            verified: std::collections::HashMap::new(),
            governance: Governance { disable_all: ext_disable_all_from_env(), ..Governance::default() },
            ticks: None,
            instance_ids: HashMap::new(),
        })
    }

    /// The host's local DID: the only actor that passes row 2 (R12).
    pub fn with_local_did(mut self, local_did: impl Into<String>) -> Self {
        self.governance.local_did = local_did.into();
        self
    }

    /// Rows 6-8 (entitlement, policy, role); default [`FailClosedGates`].
    pub fn with_gates(mut self, gates: Arc<dyn GovernanceGates>) -> Self {
        self.governance.gates = gates;
        self
    }

    /// Stop scheduled instances through the worker's `TickHost` (bus first, then the scheduler).
    pub fn with_ticks(mut self, ticks: TickHost) -> Self {
        self.ticks = Some(ticks);
        self
    }

    /// Host-local governance state.
    pub fn governance(&self) -> &Governance {
        &self.governance
    }

    /// Mutable governance state (consent, overrides).
    pub fn governance_mut(&mut self) -> &mut Governance {
        &mut self.governance
    }

    /// R19, in order; the first failure is the refusal. Returns the verified build on success.
    pub fn check_activation<'a>(&self, request: &ActivationRequest<'a>) -> Result<&'a PackageFacts, ActivationRefusal> {
        let governance = &self.governance;
        if governance.local_did.is_empty() || request.actor_did != governance.local_did {
            return refusal(Precondition::NotLocalIdentity, "only the host's local identity administers extensions");
        }
        if !self.admission {
            return refusal(Precondition::HostOff, "package admission is off on this host");
        }
        if governance.disable_all {
            return refusal(Precondition::HostOff, "Disable all is on");
        }
        let package = match request.package {
            PackageCheck::Verified(package) => package,
            PackageCheck::Invalid(reason) => return refusal(Precondition::PackageInvalid, reason.clone()),
            PackageCheck::Untrusted(reason) => return refusal(Precondition::UntrustedSigner, reason.clone()),
        };
        if package.is_revoked_in(&self.revocations)
            && !governance.overridden(request.verse_id, &package.content_hash)
        {
            return refusal(Precondition::Revoked, format!("{} is on an accepted kill list", package.package_id));
        }
        let gates = &governance.gates;
        gates.entitlement(package).or_else(|detail| refusal(Precondition::EntitlementRequired, detail))?;
        gates.policy(request.verse_id, package).or_else(|detail| refusal(Precondition::PolicyDenied, detail))?;
        gates.role(request.petal_id, package.writes).or_else(|detail| refusal(Precondition::RoleDenied, detail))?;
        match governance.consents.get(request.installation_id) {
            Some(consent)
                if consent.package_id == package.package_id && consent.capability_digest == package.capability_digest =>
            {
                Ok(package)
            }
            Some(_) => refusal(Precondition::ConsentMissing, "the recorded consent covers another capability digest"),
            None => refusal(Precondition::ConsentMissing, "no consent is recorded for this installation"),
        }
    }

    /// R19, then activate the verified bytes; the build is tracked so kill switches can find it.
    pub async fn activate_governed(
        &mut self,
        request: &ActivationRequest<'_>,
        plugin_id: &str,
        wasm_bytes: &[u8],
        fuel_budget: FuelBudget,
    ) -> Result<(), LifecycleError> {
        let package = self.check_activation(request)?;
        // Row 5 judged these facts' hashes and the revocation check below is skipped, so the bytes run must be
        // the build the facts describe (its component digest, or a 1.0 payload hash).
        let payload = payload_hash(wasm_bytes);
        if !package.hashes().any(|hash| hash.eq_ignore_ascii_case(&payload)) {
            return Err(LifecycleError::PayloadMismatch(plugin_id.into()));
        }
        {
            // An operator's Enable after a local, policy or role stop lands here; Revoked stays terminal.
            let mut registry = self
                .registry
                .lock()
                .map_err(|_| LifecycleError::RegistryError("registry lock poisoned".into()))?;
            if matches!(registry.get_state(plugin_id), Some(PluginState::Disabled(_))) {
                registry.enable(plugin_id).map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
            }
        }
        // Row 5 already decided revocation, including the Owner's per-verse override.
        self.activate_checked(plugin_id, wasm_bytes, request.petal_id, fuel_budget, false).await?;
        let package = package.clone();
        self.track_running(plugin_id, request, package, InstanceOrigin::LocalBind);
        Ok(())
    }

    /// R23 (d): verify a signed list (only `revocation = true` roots), keep the newest per signer, and stop
    /// every running build it names unless overridden in that verse. In-flight commits finish; rows stay (OD-E1).
    pub fn accept_revocation_list(&mut self, list: &RevocationList) -> Result<Vec<String>, LifecycleError> {
        let verified = self
            .trust_roots
            .verify_revocation_list(list)
            .map_err(RevocationIntakeError::from)?;
        let issued = parse_issued_at(&list.issued_at)
            .ok_or_else(|| RevocationIntakeError::BadIssuedAt(list.issued_at.clone()))?;
        let lists = &mut self.governance.intake.lists;
        match lists.get(&list.signer_key_id).map(|(at, _)| *at) {
            Some(newest) if issued < newest => {
                return Err(RevocationIntakeError::Stale {
                    signer: list.signer_key_id.clone(),
                    issued_at: list.issued_at.clone(),
                }
                .into())
            }
            // Same instant twice (a re-delivery or the signer's own equivocation): union, fail closed.
            Some(newest) if issued == newest => {
                if let Some((_, set)) = lists.get_mut(&list.signer_key_id) {
                    set.extend(verified);
                }
            }
            _ => {
                lists.insert(list.signer_key_id.clone(), (issued, verified));
            }
        }
        self.revocations = self.governance.intake.effective();

        let mut stopped: Vec<String> = self
            .governance
            .running
            .iter()
            .filter(|(_, build)| {
                build.package.is_revoked_in(&self.revocations)
                    && !self.governance.overridden(&build.verse_id, &build.package.content_hash)
            })
            .map(|(id, _)| id.clone())
            .collect();
        // Ungoverned (1.0 core) instances carry no verse, so no override can apply.
        stopped.extend(
            self.verified
                .iter()
                .filter(|(id, record)| {
                    self.installed.contains_key(*id)
                        && !self.governance.running.contains_key(*id)
                        && self.revocations.is_revoked_any(&[record.content_hash.as_str()])
                })
                .map(|(id, _)| id.clone()),
        );
        stopped.sort();
        stopped.dedup();
        for plugin_id in &stopped {
            self.stop_instance(plugin_id, DeactivateReason::Revoked);
        }
        Ok(stopped)
    }

    /// R23 (b): Disable all; every instance stops `disabled` and activation answers `host-off` until cleared.
    pub fn disable_all(&mut self) -> Vec<String> {
        self.governance.disable_all = true;
        let mut ids: Vec<String> = self.governance.running.keys().chain(self.installed.keys()).cloned().collect();
        ids.sort();
        ids.dedup();
        ids.retain(|plugin_id| self.stop_instance(plugin_id, DeactivateReason::Disabled));
        // Instances scheduled outside this manager (e.g. a lab host) stop too.
        if let Some(ticks) = &self.ticks {
            ticks.stop_all(DeactivateReason::Disabled);
        }
        ids
    }

    /// Clear Disable all; nothing restarts by itself (each installation is enabled again by the local DID).
    pub fn enable_all(&mut self) {
        self.governance.disable_all = false;
    }

    /// R23 (a): local Disable. An installation id disables it at every petal (its binding instances stop too and
    /// bindings cannot restart it); a binding instance id disables that petal only. False when nothing ran.
    pub fn disable_instance(&mut self, plugin_id: &str) -> bool {
        self.governance.disabled.insert(plugin_id.to_string());
        let mut ids = self.instance_ids.get(plugin_id).cloned().unwrap_or_default();
        ids.insert(plugin_id.to_string());
        let mut stopped = false;
        for id in ids {
            stopped |= self.stop_instance(&id, DeactivateReason::Disabled);
        }
        stopped
    }

    /// Withdraw "Allow in <verse>" (bindings.md §5.3): unless a local bind there still implies it, every binding
    /// instance of the installation in that verse stops `unbound` after its in-flight writes. Returns those ids.
    pub fn withdraw_acceptance(&mut self, installation_id: &str, verse_id: &str) -> Vec<String> {
        self.governance.withdraw_acceptance(installation_id, verse_id);
        if self.governance.is_accepted(installation_id, verse_id) {
            return Vec::new();
        }
        let mut ids: Vec<String> = self
            .governance
            .running
            .iter()
            .filter(|(_, build)| {
                build.origin == InstanceOrigin::Binding && build.installation_id == installation_id && build.verse_id == verse_id
            })
            .map(|(plugin_id, _)| plugin_id.clone())
            .collect();
        ids.sort();
        ids.retain(|plugin_id| self.stop_instance(plugin_id, DeactivateReason::Unbound));
        ids
    }

    /// Clear kill layer a for an installation and all its instance ids; nothing restarts by itself.
    pub fn enable_installation(&mut self, installation_id: &str) {
        let ids = self.instance_ids.get(installation_id).cloned().unwrap_or_default();
        self.governance.disabled.remove(installation_id);
        for id in ids {
            self.governance.disabled.remove(&id);
        }
    }

    /// Governed instances still scheduled (a stopped one is gone), sorted by instance id.
    pub fn running_instances(&self) -> Vec<RunningInstance> {
        let mut instances: Vec<RunningInstance> = self
            .governance
            .running
            .iter()
            .filter_map(|(plugin_id, build)| {
                let state = self.ticks.as_ref().and_then(|ticks| ticks.counters(plugin_id)).map(|counters| counters.state);
                if state == Some(TickState::Stopped) {
                    return None;
                }
                Some(RunningInstance {
                    plugin_id: plugin_id.clone(),
                    installation_id: build.installation_id.clone(),
                    package_id: build.package.package_id.clone(),
                    version: build.package.version.clone(),
                    content_hash: build.package.content_hash.clone(),
                    verse_id: build.verse_id.clone(),
                    petal_id: build.petal_id.clone(),
                    origin: build.origin,
                    degraded: matches!(state, Some(TickState::Degraded(_))),
                })
            })
            .collect();
        instances.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
        instances
    }

    /// The governed instance serving `(petal_id, package_id)`, if one is scheduled.
    pub fn instance_at(&self, petal_id: &str, package_id: &str) -> Option<RunningInstance> {
        self.running_instances()
            .into_iter()
            .find(|instance| instance.petal_id == petal_id && instance.package_id == package_id)
    }

    /// R23 (c) / R18: a verse policy row changed (local write or replicated arrival); every governed
    /// instance in that verse the policy gate now refuses stops `policy-denied`.
    pub fn reevaluate_verse_policy(&mut self, verse_id: &str) -> Vec<String> {
        let denied: Vec<String> = self
            .governance
            .running
            .iter()
            .filter(|(_, build)| build.verse_id == verse_id && self.governance.gates.policy(verse_id, &build.package).is_err())
            .map(|(id, _)| id.clone())
            .collect();
        for plugin_id in &denied {
            self.stop_instance(plugin_id, DeactivateReason::PolicyDenied);
        }
        denied
    }

    /// Stop one instance for a kill-switch or unbinding reason: the `TickHost` leaves the bus first and drains
    /// the in-flight commit (OD-E1); a 1.0 core instance has its lease revoked. Returns whether anything ran.
    pub fn stop_instance(&mut self, plugin_id: &str, reason: DeactivateReason) -> bool {
        let tracked = self.governance.running.remove(plugin_id).is_some();
        let loaded = self.installed.remove(plugin_id).is_some();
        let scheduled = self
            .ticks
            .as_ref()
            .and_then(|ticks| ticks.counters(plugin_id))
            .is_some_and(|counters| counters.state != TickState::Stopped);
        if !(tracked || loaded || scheduled) {
            return false;
        }
        match (&self.ticks, scheduled) {
            // TickHost::stop unbinds the context itself.
            (Some(ticks), true) => ticks.stop(plugin_id, reason),
            _ => {
                if let Err(error) = self.contexts.unbind(plugin_id) {
                    tracing::warn!(plugin = %plugin_id, %error, "revoking a stopped instance's lease failed");
                }
            }
        }
        if let Ok(mut registry) = self.registry.lock() {
            let applied = match reason {
                DeactivateReason::Revoked => registry.revoke(plugin_id, "revoked"),
                DeactivateReason::Disabled => registry.disable(plugin_id, "disabled"),
                DeactivateReason::PolicyDenied => registry.disable(plugin_id, "policy-denied"),
                DeactivateReason::RoleLost => registry.disable(plugin_id, "role-lost"),
                _ => registry.deactivate(plugin_id),
            };
            if let Err(error) = applied {
                tracing::debug!(plugin = %plugin_id, %error, "registry state after a kill switch");
            }
        }
        tracing::info!(plugin = %plugin_id, ?reason, "extension instance stopped by governance");
        true
    }

    /// R19, then start a 2.0 instance (the operator's local bind or Enable) on the worker's `TickHost`; it clears
    /// kill layer a for the installation, and the build is tracked so kill switches find it.
    pub fn start_instance(&mut self, request: &ActivationRequest<'_>, staged: StagedInstance) -> Result<(), LifecycleError> {
        let (ticks, package) = self.admit_staged(request, &staged)?;
        ticks.launch(&staged.plugin_id, staged.launch).map_err(LifecycleError::HostError)?;
        self.governance.disabled.remove(request.installation_id);
        self.governance.disabled.remove(&staged.plugin_id);
        self.track_running(&staged.plugin_id, request, package, InstanceOrigin::LocalBind);
        Ok(())
    }

    /// R19, then local acceptance and kill layer a (bindings.md §5.2 rows 12-13): the cheap check before a
    /// binding-started build is staged. [`start_binding_instance`](Self::start_binding_instance) re-runs it.
    pub fn check_binding_start(&self, request: &ActivationRequest<'_>) -> Result<(), LifecycleError> {
        self.check_activation(request)?;
        self.check_binding_local(request)
    }

    /// Rows 12-13; never satisfied by a replicated row, only by this host's own records (R12).
    fn check_binding_local(&self, request: &ActivationRequest<'_>) -> Result<(), LifecycleError> {
        if !self.governance.is_accepted(request.installation_id, request.verse_id) {
            return Err(LifecycleError::NotAccepted(format!(
                "{} is not allowed in verse {} on this host",
                request.installation_id, request.verse_id
            )));
        }
        if self.governance.is_locally_disabled(request.installation_id, Some(request.petal_id)) {
            return Err(LifecycleError::InstallationDisabled(request.installation_id.to_string()));
        }
        Ok(())
    }

    /// Start an instance a binding resolved (bindings.md §5.6): R19, acceptance and kill layer a, under the
    /// [`binding_instance_id`] of its installation and petal; any refusal discards the staged context.
    pub fn start_binding_instance(&mut self, request: &ActivationRequest<'_>, staged: StagedInstance) -> Result<(), LifecycleError> {
        let (ticks, package) = self.admit_staged(request, &staged)?;
        let expected = binding_instance_id(request.installation_id, request.petal_id);
        let refused = if staged.plugin_id != expected {
            Err(LifecycleError::LaunchMismatch(format!("{}: a binding instance is keyed {expected}", staged.plugin_id)))
        } else {
            self.check_binding_local(request)
        };
        if let Err(error) = refused {
            ticks.contexts().discard(&staged.launch.context);
            return Err(error);
        }
        ticks.launch(&staged.plugin_id, staged.launch).map_err(LifecycleError::HostError)?;
        self.track_running(&staged.plugin_id, request, package, InstanceOrigin::Binding);
        Ok(())
    }

    /// P4.3: R19 for the new build (consent covers ITS capability digest), then `TickHost::update`; on a refusal the
    /// previous build is re-activated and keeps running (src/AGENTS.md §update). A binding instance re-checks
    /// acceptance and kill layer a, and stays a binding instance.
    pub fn update_instance(&mut self, request: &ActivationRequest<'_>, staged: StagedInstance) -> Result<(), LifecycleError> {
        let (ticks, package) = self.admit_staged(request, &staged)?;
        // An update replaces a build of the SAME installation and package; anything else is an install (consent anew).
        let running = self.governance.running.get(&staged.plugin_id);
        if let Some(build) = running.filter(|build| {
            build.installation_id != request.installation_id || build.package.package_id != package.package_id
        }) {
            ticks.contexts().discard(&staged.launch.context);
            return Err(LifecycleError::LaunchMismatch(format!(
                "{}: runs {} of installation {}, not an update of it",
                staged.plugin_id, build.package.package_id, build.installation_id
            )));
        }
        let origin = running.map_or(InstanceOrigin::LocalBind, |build| build.origin);
        if origin == InstanceOrigin::Binding {
            if let Err(error) = self.check_binding_local(request) {
                ticks.contexts().discard(&staged.launch.context);
                return Err(error);
            }
        }
        let StagedInstance { plugin_id, launch, .. } = staged;
        match ticks.update(&plugin_id, launch) {
            Ok(()) => {
                self.track_running(&plugin_id, request, package, origin);
                Ok(())
            }
            Err(UpdateFailure::NotRunning(detail)) => Err(LifecycleError::NotRunning(detail)),
            Err(UpdateFailure::RolledBack(reason)) => Err(LifecycleError::RolledBack(reason)),
            Err(UpdateFailure::PreviousStopped(reason)) => {
                self.governance.running.remove(&plugin_id);
                if let Ok(mut registry) = self.registry.lock() {
                    if let Err(error) = registry.deactivate(&plugin_id) {
                        tracing::debug!(plugin = %plugin_id, %error, "registry state after a failed update");
                    }
                }
                Err(LifecycleError::UpdateAborted(reason))
            }
        }
    }

    /// Uninstall a 2.0 instance (G1-C10): stop `unbound`, `on-uninstall` with one final flush, then forget its tick,
    /// bus and network counters. An installation id covers every instance ever started for it (binding instances
    /// at other petals, running or stopped). Consent records, acceptances and the package store are the caller's.
    pub fn uninstall_instance(&mut self, plugin_id: &str) {
        let mut ids = self.instance_ids.remove(plugin_id).unwrap_or_default();
        ids.insert(plugin_id.to_string());
        for id in &ids {
            self.governance.running.remove(id);
            self.governance.disabled.remove(id);
            if let Some(ticks) = &self.ticks {
                ticks.uninstall(id);
            }
            if let Ok(mut registry) = self.registry.lock() {
                if registry.unregister(id).is_err() {
                    tracing::debug!(plugin = %id, "uninstalled instance had no registry entry");
                }
            }
        }
    }

    /// R19 plus the staged-build binding; any refusal revokes the staged context so a refused build holds no lease.
    fn admit_staged(
        &self,
        request: &ActivationRequest<'_>,
        staged: &StagedInstance,
    ) -> Result<(TickHost, PackageFacts), LifecycleError> {
        let admitted = match &self.ticks {
            None => Err(LifecycleError::HostError("no tick host is attached to this manager".into())),
            Some(ticks) => self
                .check_activation(request)
                .map_err(LifecycleError::from)
                .and_then(|package| bind_staged(request, package, staged).map(|()| (ticks.clone(), package.clone()))),
        };
        if admitted.is_err() {
            self.ticks
                .as_ref()
                .map_or(&self.contexts, TickHost::contexts)
                .discard(&staged.launch.context);
        }
        admitted
    }

    /// Track a started build for the kill switches; a local bind's registry entry moves to `Active` (binding
    /// instances have no registry entry).
    fn track_running(&mut self, plugin_id: &str, request: &ActivationRequest<'_>, package: PackageFacts, origin: InstanceOrigin) {
        self.governance.running.insert(
            plugin_id.to_string(),
            RunningBuild {
                installation_id: request.installation_id.to_string(),
                verse_id: request.verse_id.to_string(),
                petal_id: request.petal_id.to_string(),
                origin,
                package,
            },
        );
        self.instance_ids
            .entry(request.installation_id.to_string())
            .or_default()
            .insert(plugin_id.to_string());
        if origin == InstanceOrigin::Binding {
            return;
        }
        if let Ok(mut registry) = self.registry.lock() {
            if matches!(registry.get_state(plugin_id), Some(PluginState::Disabled(_))) {
                let _ = registry.enable(plugin_id);
            }
            if !matches!(registry.get_state(plugin_id), Some(PluginState::Active)) {
                if let Err(error) = registry.activate(plugin_id) {
                    tracing::debug!(plugin = %plugin_id, %error, "registry state after a start");
                }
            }
        }
    }

    /// Configure publisher trust roots and the unsigned-package policy (default: no roots, refuse unsigned).
    pub fn with_trust(mut self, trust_roots: TrustRoots, unsigned_policy: UnsignedPolicy) -> Self {
        self.trust_roots = trust_roots;
        self.unsigned_policy = unsigned_policy;
        self
    }

    /// Switch package admission on or off; pass [`crate::PackageAdmission`]'s value (default off).
    pub fn with_admission(mut self, enabled: bool) -> Self {
        self.admission = enabled;
        self
    }

    /// Replace the startup seed (e.g. the cached last good lists); lists accepted since stay in the union.
    pub fn set_revocations(&mut self, revocations: RevocationSet) {
        self.governance.intake.seed = revocations;
        self.revocations = self.governance.intake.effective();
    }

    fn require_admission(&self) -> Result<(), LifecycleError> {
        if self.admission {
            Ok(())
        } else {
            Err(LifecycleError::HostOff)
        }
    }

    fn refuse_if_revoked(&self, plugin_id: &str, content_hash: &str) -> Result<(), LifecycleError> {
        if self.revocations.is_revoked_any(&[content_hash]) {
            return Err(PackageError::Revoked {
                package_id: plugin_id.to_string(),
                content_hash: content_hash.to_string(),
            }
            .into());
        }
        Ok(())
    }

    /// Install a Wasm plugin: verify publisher signature and signed id, check revocation, AOT compile, register as `Installed`.
    ///
    /// The capability manifest is parsed from `manifest_bytes` (the signed bytes), never supplied separately.
    pub fn install_plugin(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
        manifest_bytes: &[u8],
        signatures: &[SignatureEntry],
    ) -> Result<(), LifecycleError> {
        self.require_admission()?;
        // Step 1: signed id + ed25519 over manifest_bytes || blake3(payload) against the trust roots
        let actual_hash = payload_hash(wasm_bytes);
        let verified = verify_package(
            plugin_id,
            manifest_bytes,
            wasm_bytes,
            signatures,
            &self.trust_roots,
            self.unsigned_policy,
        )?;
        self.refuse_if_revoked(plugin_id, &actual_hash)?;
        let manifest = std::str::from_utf8(manifest_bytes)
            .map_err(|e| e.to_string())
            .and_then(|text| CapabilityManifest::from_json(text).map_err(|e| e.to_string()))
            .map_err(LifecycleError::Manifest)?;
        tracing::info!(plugin = %plugin_id, hash = %actual_hash, ?verified, "Plugin signature verified");

        // Step 2: AOT compile and cache
        let compiled = self
            .aot_cache
            .get_or_compile(self.engine.engine(), wasm_bytes)
            .map_err(|e| LifecycleError::AotCompileError(e.to_string()))?;

        // Deserialize the compiled module to verify it's valid
        // SAFETY: compiled bytes come from wasmtime::Module::serialize on the same engine
        let module = unsafe { wasmtime::Module::deserialize(self.engine.engine(), &compiled) }
            .map_err(|e| LifecycleError::WasmError(e.to_string()))?;

        let plugin = WasmPlugin {
            plugin_id: plugin_id.to_string(),
            module,
            wasm_bytes: wasm_bytes.to_vec(),
        };

        // Step 3: Register in registry
        {
            let mut reg = self.registry.lock().unwrap();
            reg.register(
                plugin_id,
                &format!("wasm:{}", actual_hash),
                PluginTier::Wasm,
            )
            .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        // Activation is bound to exactly what was verified here.
        let _ = plugin;
        self.verified.insert(
            plugin_id.to_string(),
            VerifiedInstall {
                content_hash: actual_hash,
                manifest,
            },
        );

        tracing::info!(plugin = %plugin_id, "Plugin installed successfully");
        Ok(())
    }

    /// Fetch `uri` over registry protocol v1, then install through the same signature + revocation gate; returns the plugin id.
    pub async fn install_from_registry(
        &mut self,
        client: &RegistryClient,
        uri: &str,
    ) -> Result<String, LifecycleError> {
        self.require_admission()?;
        // fetch_package binds the requested uri to the served id; install_plugin binds that id to the signed manifest.
        let package = client.fetch_package(uri).await?;
        self.install_plugin(
            &package.package_id,
            &package.payload,
            &package.manifest_bytes,
            &package.signatures,
        )?;
        Ok(package.package_id)
    }

    /// Attach the same admitted instance registry installed by PluginHostPlugin.
    pub fn with_contexts(mut self, contexts: crate::scene_host::PluginContexts) -> Self {
        self.contexts = contexts;
        self
    }

    /// Activate an installed plugin; `wasm_bytes` must be the verified payload and the token comes from its signed manifest.
    pub async fn activate_with_bytes(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
        petal_id: &str,
        fuel_budget: FuelBudget,
    ) -> Result<(), LifecycleError> {
        self.activate_checked(plugin_id, wasm_bytes, petal_id, fuel_budget, true).await
    }

    /// The activation body; `check_revocation` is false only after R19 row 5 judged it (with overrides).
    async fn activate_checked(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
        petal_id: &str,
        fuel_budget: FuelBudget,
        check_revocation: bool,
    ) -> Result<(), LifecycleError> {
        self.require_admission()?;
        if self.governance.disable_all {
            return Err(LifecycleError::HostOff);
        }
        {
            let registry = self
                .registry
                .lock()
                .map_err(|_| LifecycleError::RegistryError("registry lock poisoned".into()))?;
            let state = registry
                .get_state(plugin_id)
                .ok_or_else(|| LifecycleError::PluginNotFound(plugin_id.into()))?;
            if !matches!(
                state,
                PluginState::Installed | PluginState::Deactivated | PluginState::Degraded(_)
            ) {
                return Err(LifecycleError::InvalidState {
                    plugin_id: plugin_id.into(),
                    state: format!("{state:?}"),
                });
            }
        }
        let content_hash = payload_hash(wasm_bytes);
        let capabilities = match self.verified.get(plugin_id) {
            None => return Err(LifecycleError::NotVerified(plugin_id.into())),
            Some(record) if record.content_hash != content_hash => {
                return Err(LifecycleError::PayloadMismatch(plugin_id.into()))
            }
            Some(record) => record.manifest.clone(),
        };
        if check_revocation {
            self.refuse_if_revoked(plugin_id, &content_hash)?;
        }
        // Load or compile from AOT cache
        let compiled = self
            .aot_cache
            .get_or_compile(self.engine.engine(), wasm_bytes)
            .map_err(|e| LifecycleError::AotCompileError(e.to_string()))?;

        // SAFETY: compiled bytes come from wasmtime::Module::serialize on the same engine
        let module = unsafe { wasmtime::Module::deserialize(self.engine.engine(), &compiled) }
            .map_err(|e| LifecycleError::WasmError(e.to_string()))?;

        let plugin = WasmPlugin {
            plugin_id: plugin_id.to_string(),
            module,
            wasm_bytes: wasm_bytes.to_vec(),
        };

        // Create plugin context
        let token = CapabilityToken::mint(plugin_id, &capabilities);
        let ctx = self
            .contexts
            .bind(
                plugin_id.to_string(),
                petal_id.to_string(),
                token,
                self.cmd_tx.clone(),
            )
            .map_err(LifecycleError::HostError)?;

        // Instantiate
        let instantiated = self.engine.instantiate(&plugin, ctx, fuel_budget).await;
        let (mut store, instance) = match instantiated {
            Ok(instance) => instance,
            Err(error) => {
                self.contexts
                    .unbind(plugin_id)
                    .map_err(LifecycleError::HostError)?;
                return Err(LifecycleError::WasmError(error.to_string()));
            }
        };

        // Try to call on_activate export
        if instance.get_func(&mut store, "on_activate").is_some() {
            tracing::debug!(plugin = %plugin_id, "Calling on_activate");
            if let Err(error) = self
                .engine
                .call_export(&mut store, &instance, "on_activate", &[], &mut [])
                .await
            {
                self.contexts
                    .unbind(plugin_id)
                    .map_err(LifecycleError::HostError)?;
                return Err(LifecycleError::WasmError(error.to_string()));
            }
        }

        // Store the running instance
        self.installed.insert(
            plugin_id.to_string(),
            InstalledPlugin {
                plugin,
                store,
                instance,
            },
        );

        // Update registry state
        {
            let mut reg = self.registry.lock().unwrap();
            reg.activate(plugin_id)
                .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        tracing::info!(plugin = %plugin_id, "Plugin activated");
        Ok(())
    }

    /// Deactivate a running plugin: call `on_deactivate`, drop instance, registry `Deactivated`.
    pub async fn deactivate_plugin(&mut self, plugin_id: &str) -> Result<(), LifecycleError> {
        // Try to call on_deactivate if the plugin is running
        if let Some(installed) = self.installed.get_mut(plugin_id) {
            if installed
                .instance
                .get_func(&mut installed.store, "on_deactivate")
                .is_some()
            {
                tracing::debug!(plugin = %plugin_id, "Calling on_deactivate");
                // Call but don't fail if the export errors — we still want to deactivate
                let _ = self
                    .engine
                    .call_export(
                        &mut installed.store,
                        &installed.instance,
                        "on_deactivate",
                        &[],
                        &mut [],
                    )
                    .await;
            }
        }

        // Remove from installed map
        self.installed.remove(plugin_id);
        self.contexts
            .unbind(plugin_id)
            .map_err(LifecycleError::HostError)?;

        // Update registry state
        {
            let mut reg = self.registry.lock().unwrap();
            reg.deactivate(plugin_id)
                .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        tracing::info!(plugin = %plugin_id, "Plugin deactivated");
        Ok(())
    }

    /// Uninstall a plugin: call `on_uninstall`, unregister, evict from AOT cache, forget every counter (G1-C10).
    pub async fn uninstall_plugin(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<(), LifecycleError> {
        // A scheduled instance stops `unbound` and its tick, bus and network counters go.
        self.governance.running.remove(plugin_id);
        if let Some(ticks) = &self.ticks {
            ticks.uninstall(plugin_id);
        }
        // Deactivate if active, calling on_uninstall
        if let Some(installed) = self.installed.get_mut(plugin_id) {
            if installed
                .instance
                .get_func(&mut installed.store, "on_uninstall")
                .is_some()
            {
                tracing::debug!(plugin = %plugin_id, "Calling on_uninstall");
                let _ = self
                    .engine
                    .call_export(
                        &mut installed.store,
                        &installed.instance,
                        "on_uninstall",
                        &[],
                        &mut [],
                    )
                    .await;
            }
            self.installed.remove(plugin_id);
        }

        self.contexts
            .unbind(plugin_id)
            .map_err(LifecycleError::HostError)?;
        // Remove from registry
        {
            let mut reg = self.registry.lock().unwrap();
            reg.unregister(plugin_id)
                .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }
        self.verified.remove(plugin_id);

        // Evict from AOT cache
        self.aot_cache
            .evict(wasm_bytes)
            .map_err(|e| LifecycleError::IoError(e.to_string()))?;

        tracing::info!(plugin = %plugin_id, "Plugin uninstalled");
        Ok(())
    }

    /// Get a reference to an installed plugin's store.
    pub fn get_store(&self, plugin_id: &str) -> Option<&wasmtime::Store<PluginStoreData>> {
        self.installed.get(plugin_id).map(|p| &p.store)
    }

    /// Get a mutable reference to an installed plugin's store.
    pub fn get_store_mut(
        &mut self,
        plugin_id: &str,
    ) -> Option<&mut wasmtime::Store<PluginStoreData>> {
        self.installed.get_mut(plugin_id).map(|p| &mut p.store)
    }

    /// Get the instance for a plugin.
    pub fn get_instance(&self, plugin_id: &str) -> Option<&wasmtime::Instance> {
        self.installed.get(plugin_id).map(|p| &p.instance)
    }

    /// Get a reference to the Wasm engine.
    pub fn engine(&self) -> &Arc<WasmEngine> {
        &self.engine
    }

    /// Get a reference to the AOT cache.
    pub fn aot_cache(&self) -> &AotCache {
        &self.aot_cache
    }

    /// Check if a plugin is installed (in the installed map).
    pub fn is_running(&self, plugin_id: &str) -> bool {
        self.installed.contains_key(plugin_id)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("Plugin '{0}' not found")]
    PluginNotFound(String),

    #[error("Invalid state for plugin '{plugin_id}': {state}")]
    InvalidState { plugin_id: String, state: String },

    #[error("plugin package admission is off (enable the plugin-host feature and FE_PLUGIN_HOST_ALLOW=1)")]
    HostOff,

    #[error("Plugin '{0}' was not installed through package verification")]
    NotVerified(String),

    #[error("Plugin '{0}': activation bytes differ from the verified payload")]
    PayloadMismatch(String),

    #[error("Signed capability manifest is invalid: {0}")]
    Manifest(String),

    #[error("Package verification failed: {0}")]
    Package(#[from] PackageError),

    #[error("Registry install failed: {0}")]
    Registry(#[from] RegistryInstallError),

    #[error("AOT compilation error: {0}")]
    AotCompileError(String),

    #[error("Wasm error: {0}")]
    WasmError(String),

    #[error("Registry error: {0}")]
    RegistryError(String),

    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Plugin host binding failed: {0}")]
    HostError(String),

    #[error("activation refused: {0}")]
    Refused(#[from] ActivationRefusal),

    #[error("revocation list refused: {0}")]
    Revocation(#[from] RevocationIntakeError),

    #[error("staged build does not match the judged request: {0}")]
    LaunchMismatch(String),

    #[error("no running instance to update: {0}")]
    NotRunning(String),

    #[error("update failed, previous version kept: {0}")]
    RolledBack(String),

    #[error("update failed and the previous version stopped: {0}")]
    UpdateAborted(String),

    /// bindings.md §5.2 row 12: no local acceptance for the installation in that verse.
    #[error("not-accepted: {0}")]
    NotAccepted(String),

    /// bindings.md §5.2 row 13: kill layer a holds until the operator's Enable.
    #[error("disabled: {0} is disabled on this host")]
    InstallationDisabled(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::PluginRegistry;

    const MANIFEST_BYTES: &[u8] = br#"{"id":"test-plugin","property_scope":["*"]}"#;

    fn publisher() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
    }

    fn manifest_for(id: &str) -> Vec<u8> {
        format!(r#"{{"id":"{id}"}}"#).into_bytes()
    }

    fn trust_roots_for(key: &ed25519_dalek::SigningKey) -> TrustRoots {
        TrustRoots::from_toml_str(&format!(
            "[[publisher]]\nkey_id = \"pub\"\npublic_key = \"{}\"\n",
            hex::encode(key.verifying_key().to_bytes())
        ))
        .unwrap()
    }

    fn sign(key: &ed25519_dalek::SigningKey, key_id: &str, manifest: &[u8], bytes: &[u8]) -> SignatureEntry {
        use ed25519_dalek::Signer;
        let message = crate::package::signed_message(manifest, bytes);
        SignatureEntry {
            key_id: key_id.into(),
            signature: hex::encode(key.sign(&message).to_bytes()),
        }
    }

    fn make_test_manager() -> (PluginManager, tempfile::TempDir) {
        let engine = Arc::new(WasmEngine::new().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let registry = Arc::new(std::sync::Mutex::new(PluginRegistry::new()));
        let (tx, _rx) = crossbeam::channel::unbounded();

        let manager = PluginManager::new(engine, tmp.path().join("plugins"), registry, tx)
            .unwrap()
            .with_trust(trust_roots_for(&publisher()), UnsignedPolicy::Refuse)
            .with_admission(true);

        (manager, tmp)
    }

    /// Installs a module signed by the trusted publisher under a manifest naming `id`.
    fn install_signed(manager: &mut PluginManager, id: &str, bytes: &[u8]) {
        let manifest = manifest_for(id);
        manager
            .install_plugin(id, bytes, &manifest, &[sign(&publisher(), "pub", &manifest, bytes)])
            .unwrap();
    }

    const LOCAL: &str = "did:key:z6MkLocalHost";

    fn revoker() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[5u8; 32])
    }

    /// The publisher plus a revocation-capable root.
    fn trust_roots_with_revoker() -> TrustRoots {
        TrustRoots::from_toml_str(&format!(
            "[[publisher]]\nkey_id = \"pub\"\npublic_key = \"{}\"\n\n[[publisher]]\nkey_id = \"revoker\"\npublic_key = \"{}\"\nrevocation = true\n",
            hex::encode(publisher().verifying_key().to_bytes()),
            hex::encode(revoker().verifying_key().to_bytes())
        ))
        .unwrap()
    }

    /// A list naming `hash` under a DIFFERENT package id (matching is by hash under any id).
    fn signed_list(key: &ed25519_dalek::SigningKey, key_id: &str, issued_at: &str, hash: &str) -> RevocationList {
        use ed25519_dalek::Signer;
        let mut list = RevocationList {
            schema_version: 1,
            issued_at: issued_at.into(),
            entries: vec![crate::trust_roots::RevocationEntry {
                package_id: "renamed".into(),
                content_hash: hash.into(),
                reason: "withdrawn".into(),
                revoked_at: issued_at.into(),
            }],
            signer_key_id: key_id.into(),
            signature: String::new(),
        };
        list.signature = hex::encode(key.sign(&list.signed_bytes()).to_bytes());
        list
    }

    /// Rows 6-8 as switches the journey flips (true = refuse).
    #[derive(Default)]
    struct SwitchGates {
        entitlement: std::sync::atomic::AtomicBool,
        policy: std::sync::atomic::AtomicBool,
        role: std::sync::atomic::AtomicBool,
    }

    impl SwitchGates {
        fn set(&self, entitlement: bool, policy: bool, role: bool) {
            use std::sync::atomic::Ordering::SeqCst;
            self.entitlement.store(entitlement, SeqCst);
            self.policy.store(policy, SeqCst);
            self.role.store(role, SeqCst);
        }
    }

    impl GovernanceGates for SwitchGates {
        fn entitlement(&self, _package: &PackageFacts) -> Result<(), String> {
            if self.entitlement.load(std::sync::atomic::Ordering::SeqCst) { Err("no entitlement token".into()) } else { Ok(()) }
        }
        fn policy(&self, _verse_id: &str, _package: &PackageFacts) -> Result<(), String> {
            if self.policy.load(std::sync::atomic::Ordering::SeqCst) { Err("verse policy: no allow entry admits this build".into()) } else { Ok(()) }
        }
        fn role(&self, _petal_id: &str, _writes: bool) -> Result<(), String> {
            if self.role.load(std::sync::atomic::Ordering::SeqCst) { Err("below Editor at the petal".into()) } else { Ok(()) }
        }
    }

    fn facts(package_id: &str, hash: &str) -> PackageFacts {
        PackageFacts {
            package_id: package_id.into(),
            version: "1.0.0".into(),
            content_hash: hash.into(),
            component_digest: None,
            plaintext_digest: None,
            capability_digest: "digest-1".into(),
            trust: TrustTier::Publisher,
            writes: true,
            licensed: true,
            manifest: None,
        }
    }

    fn request<'a>(actor: &'a str, installation: &'a str, verse: &'a str, package: &'a PackageCheck) -> ActivationRequest<'a> {
        ActivationRequest { actor_did: actor, installation_id: installation, verse_id: verse, petal_id: "petal", package }
    }

    fn consent(installation: &str, package_id: &str, digest: &str) -> ConsentRecord {
        ConsentRecord {
            installation_id: installation.into(),
            package_id: package_id.into(),
            content_hash: String::new(),
            capability_digest: digest.into(),
            actor_did: LOCAL.into(),
            at: "2026-11-08T00:00:00Z".into(),
        }
    }

    fn governed(manager: &mut PluginManager, gates: Arc<SwitchGates>) {
        let gates: Arc<dyn GovernanceGates> = gates;
        manager.governance = Governance { local_did: LOCAL.into(), gates, ..Governance::default() };
    }

    /// A 2.0 guest stand-in: logs its lifecycle hooks, never stages, refuses `on-activate` when asked.
    #[derive(Clone)]
    struct FakeGuest {
        log: Arc<std::sync::Mutex<Vec<&'static str>>>,
        refuse: bool,
    }

    impl crate::tick::TickInstance for FakeGuest {
        fn refuel(&mut self, _fuel: u64) -> Result<(), String> {
            Ok(())
        }
        fn on_tick(&mut self, _info: &crate::tick::TickInfo) -> Result<(), crate::tick::TickFault> {
            Ok(())
        }
        fn on_deactivate(&mut self, _reason: DeactivateReason) {
            self.log.lock().unwrap().push("deactivate");
        }
    }

    impl crate::tick::TickSink for FakeGuest {
        fn submit(&mut self, _tick: u64, _flush: crate::transaction::StagedFlush, reply: Sender<crate::tick::FlushReply>) {
            let _ = reply.send(crate::tick::FlushReply::Refused("no database in this journey".into()));
        }
    }

    impl GuestLifecycle for FakeGuest {
        fn on_install(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("install");
            Ok(())
        }
        fn on_activate(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("activate");
            if self.refuse { Err("guest refused".into()) } else { Ok(()) }
        }
        fn on_uninstall(&mut self) {
            self.log.lock().unwrap().push("uninstall");
        }
        fn degraded(&self) -> Option<String> {
            None
        }
    }

    /// A staged 2.0 build of `fleet` on `ticks`' contexts; no seat (J1 covers the bus).
    fn staged_build(ticks: &TickHost, digest: &str, guest: &FakeGuest, first_activation: bool) -> StagedInstance {
        let (commands, _queued) = crossbeam::channel::unbounded();
        let token = CapabilityToken::mint("fleet", &CapabilityManifest::from_json(r#"{"id":"fleet"}"#).unwrap());
        let context = ticks.contexts().stage("fleet".into(), "petal".into(), token, commands).unwrap();
        let window: crate::tick::SharedWindow = Arc::new(std::sync::Mutex::new(crate::transaction::StagedWindow::new(0, 0)));
        let scheduler = crate::tick::TickScheduler::without_ticks(ticks.clock(), window, Box::new(guest.clone()), Box::new(guest.clone()));
        StagedInstance {
            plugin_id: "fleet".into(),
            component_digest: digest.into(),
            launch: Launch { scheduler, seat: None, guest: Box::new(guest.clone()), context, first_activation },
        }
    }

    /// Journey (AC-4 table + identity binding): every refusal is typed, then the valid package installs and activation is pinned to it.
    #[tokio::test]
    async fn install_plugin_signature_journey() {
        let (mut manager, _tmp) = make_test_manager();
        let bytes = wat::parse_str(r#"(module)"#).unwrap();
        let stranger = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
        let valid = sign(&publisher(), "pub", MANIFEST_BYTES, &bytes);

        // Admission off: nothing is even verified.
        manager.admission = false;
        let err = manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, std::slice::from_ref(&valid))
            .unwrap_err();
        assert!(matches!(err, LifecycleError::HostOff));
        manager.admission = true;

        type RefusalCase = (&'static str, Vec<SignatureEntry>, fn(&LifecycleError) -> bool);
        let refusals: [RefusalCase; 5] = [
            // Unsigned is refused by default.
            ("test-plugin", vec![], |e| matches!(e, LifecycleError::Package(PackageError::Unsigned(_)))),
            // Unlisted key is refused with a typed error.
            ("test-plugin", vec![sign(&stranger, "stranger", MANIFEST_BYTES, &bytes)], |e| {
                matches!(e, LifecycleError::Package(PackageError::UntrustedKey { .. }))
            }),
            // A trusted key signing different payload bytes fails verification.
            ("test-plugin", vec![sign(&publisher(), "pub", MANIFEST_BYTES, b"other")], |e| {
                matches!(e, LifecycleError::Package(PackageError::InvalidSignature { .. }))
            }),
            // A failing trusted entry is tampering even beside a valid one.
            ("test-plugin", vec![valid.clone(), SignatureEntry { key_id: "pub".into(), signature: "00".into() }], |e| {
                matches!(e, LifecycleError::Package(PackageError::InvalidSignature { .. }))
            }),
            // A validly signed package served under another id: the signed manifest names test-plugin.
            ("renamed-plugin", vec![valid.clone()], |e| {
                matches!(e, LifecycleError::Package(PackageError::IdMismatch { .. }))
            }),
        ];
        for (id, signatures, refused) in refusals {
            let err = manager.install_plugin(id, &bytes, MANIFEST_BYTES, &signatures).unwrap_err();
            assert!(refused(&err), "{id} {signatures:?}: {err}");
        }

        // The dev flag admits an unsigned package, in debug builds only (it does not exist in release).
        #[cfg(debug_assertions)]
        {
            manager.unsigned_policy = UnsignedPolicy::AllowDev;
            manager
                .install_plugin("dev-plugin", &bytes, &manifest_for("dev-plugin"), &[])
                .unwrap();
            manager.unsigned_policy = UnsignedPolicy::Refuse;
        }

        // Counter-signature list: an untrusted entry beside a valid trusted one is accepted.
        let signatures = [sign(&stranger, "stranger", MANIFEST_BYTES, &bytes), valid];
        manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &signatures)
            .unwrap();
        assert_eq!(
            manager.registry.lock().unwrap().get_state("test-plugin"),
            Some(&PluginState::Installed)
        );

        // Activation runs only the verified bytes.
        let other = wat::parse_str(r#"(module (func (export "on_activate")))"#).unwrap();
        let err = manager
            .activate_with_bytes("test-plugin", &other, "petal", FuelBudget::default())
            .await
            .unwrap_err();
        assert!(matches!(err, LifecycleError::PayloadMismatch(_)), "{err}");

        // R19 (governance.md J6): each row refuses until every row before it passes; the observed
        // order is the contract's order, with a non-local actor judged before host-off.
        let gates = Arc::new(SwitchGates::default());
        gates.set(true, true, true);
        governed(&mut manager, gates.clone());
        let hash = payload_hash(&bytes);
        let verified = PackageCheck::Verified(facts("test-plugin", &hash));
        let invalid = PackageCheck::Invalid("id-mismatch".into());
        let untrusted = PackageCheck::Untrusted("no-trust-root".into());
        let refused = |manager: &PluginManager, actor: &str, verse: &str, package: &PackageCheck| {
            manager.check_activation(&request(actor, "inst-1", verse, package)).err().map(|r| r.precondition)
        };
        let mut observed = Vec::new();
        manager.admission = false;
        observed.push(refused(&manager, "did:key:z6MkRemote", "verse-1", &verified));
        observed.push(refused(&manager, LOCAL, "verse-1", &verified));
        manager.admission = true;
        observed.push(refused(&manager, LOCAL, "verse-1", &invalid));
        observed.push(refused(&manager, LOCAL, "verse-1", &untrusted));
        manager.set_revocations(RevocationSet::from_entries(&[crate::trust_roots::RevocationEntry {
            package_id: "original-name".into(),
            content_hash: hash.clone(),
            reason: "withdrawn".into(),
            revoked_at: "2026-11-08T00:00:00Z".into(),
        }]));
        // The Owner's override reaches only its own verse.
        manager.governance_mut().add_kill_list_override("verse-2", &hash);
        observed.push(refused(&manager, LOCAL, "verse-1", &verified));
        observed.push(refused(&manager, LOCAL, "verse-2", &verified));
        manager.set_revocations(RevocationSet::default());
        gates.set(false, true, true);
        observed.push(refused(&manager, LOCAL, "verse-1", &verified));
        gates.set(false, false, true);
        observed.push(refused(&manager, LOCAL, "verse-1", &verified));
        gates.set(false, false, false);
        observed.push(refused(&manager, LOCAL, "verse-1", &verified));
        use Precondition as P;
        assert_eq!(
            observed,
            [P::NotLocalIdentity, P::HostOff, P::PackageInvalid, P::UntrustedSigner, P::Revoked, P::EntitlementRequired,
                P::PolicyDenied, P::RoleDenied, P::ConsentMissing]
                .map(Some)
                .to_vec()
        );
        // Consent covers one exact capability digest: a stale one is still missing, the exact one passes.
        manager.governance_mut().record_consent(consent("inst-1", "test-plugin", "digest-0"));
        assert_eq!(refused(&manager, LOCAL, "verse-1", &verified), Some(P::ConsentMissing));
        manager.governance_mut().record_consent(consent("inst-1", "test-plugin", "digest-1"));
        assert_eq!(refused(&manager, LOCAL, "verse-1", &verified), None);
        assert!(ext_disable_all(Some("1")) && !ext_disable_all(Some("true")) && !ext_disable_all(None));
    }

    /// Journey (R23 kill switch, OD-E1): a revoked hash is refused at install under any id; then a running
    /// governed instance is stopped by policy removal, by an accepted list (unless the Owner overrode it in
    /// that verse), and by Disable all; a replayed older list and a non-revoker's list are refused. Then a
    /// 2.0 build starts, updates (P4.3: R19 again, rollback), is revoked by component digest and uninstalled.
    #[tokio::test]
    async fn kill_switch_journey() {
        let (mut manager, _tmp) = make_test_manager();
        let bytes = wat::parse_str(r#"(module)"#).unwrap();
        let hash = payload_hash(&bytes);
        // The list names the build under a different package id; the hash alone revokes it.
        manager.set_revocations(RevocationSet::from_entries(&[crate::trust_roots::RevocationEntry {
            package_id: "original-name".into(),
            content_hash: hash.clone(),
            reason: "withdrawn".into(),
            revoked_at: "2026-11-08T00:00:00Z".into(),
        }]));
        let err = manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &[sign(&publisher(), "pub", MANIFEST_BYTES, &bytes)])
            .unwrap_err();
        assert!(matches!(err, LifecycleError::Package(PackageError::Revoked { .. })));

        // Two installations of the same build, bound in two verses.
        manager.set_revocations(RevocationSet::default());
        manager.trust_roots = trust_roots_with_revoker();
        let gates = Arc::new(SwitchGates::default());
        governed(&mut manager, gates.clone());
        install_signed(&mut manager, "test-plugin", &bytes);
        install_signed(&mut manager, "other-plugin", &bytes);
        let first = PackageCheck::Verified(facts("test-plugin", &hash));
        let second = PackageCheck::Verified(facts("other-plugin", &hash));
        manager.governance_mut().record_consent(consent("inst-1", "test-plugin", "digest-1"));
        manager.governance_mut().record_consent(consent("inst-2", "other-plugin", "digest-1"));
        manager
            .activate_governed(&request(LOCAL, "inst-1", "verse-1", &first), "test-plugin", &bytes, FuelBudget::default())
            .await
            .unwrap();
        manager
            .activate_governed(&request(LOCAL, "inst-2", "verse-2", &second), "other-plugin", &bytes, FuelBudget::default())
            .await
            .unwrap();
        assert_eq!(manager.governance().running_in("verse-1"), vec!["test-plugin".to_string()]);
        let state = |manager: &PluginManager, id: &str| manager.registry.lock().unwrap().get_state(id).cloned();
        // The bytes run must be the build R19 judged: facts naming another build are refused (G2).
        let elsewhere = PackageCheck::Verified(facts("test-plugin", &"0".repeat(64)));
        assert!(matches!(
            manager
                .activate_governed(&request(LOCAL, "inst-1", "verse-1", &elsewhere), "test-plugin", &bytes, FuelBudget::default())
                .await,
            Err(LifecycleError::PayloadMismatch(_))
        ));

        // (c) Policy removal stops only that verse's instance; a later Enable re-runs R19 and restarts it.
        gates.set(false, true, false);
        assert_eq!(manager.reevaluate_verse_policy("verse-1"), vec!["test-plugin".to_string()]);
        assert!(!manager.is_running("test-plugin") && manager.is_running("other-plugin"));
        assert_eq!(state(&manager, "test-plugin"), Some(PluginState::Disabled("policy-denied".into())));
        gates.set(false, false, false);
        manager
            .activate_governed(&request(LOCAL, "inst-1", "verse-1", &first), "test-plugin", &bytes, FuelBudget::default())
            .await
            .unwrap();

        // (d) The Owner overrode the kill list in verse-1 on this host: that instance keeps running; verse-2's stops.
        manager.governance_mut().add_kill_list_override("verse-1", &hash);
        let list = signed_list(&revoker(), "revoker", "2026-11-08T10:00:00.5Z", &hash);
        assert_eq!(manager.accept_revocation_list(&list).unwrap(), vec!["other-plugin".to_string()]);
        assert!(manager.is_running("test-plugin") && !manager.is_running("other-plugin"));
        assert_eq!(state(&manager, "other-plugin"), Some(PluginState::Revoked("revoked".into())));
        assert_eq!(
            manager.check_activation(&request(LOCAL, "inst-2", "verse-2", &second)).unwrap_err().precondition,
            Precondition::Revoked
        );
        // A replayed older list would un-revoke; it is refused (compared by value: .5 s after 10:00:00 is newer).
        let replay = signed_list(&revoker(), "revoker", "2026-11-08T10:00:00Z", "00".repeat(32).as_str());
        assert!(matches!(
            manager.accept_revocation_list(&replay),
            Err(LifecycleError::Revocation(RevocationIntakeError::Stale { .. }))
        ));
        // A key trusted to sign packages can never revoke.
        let forged = signed_list(&publisher(), "pub", "2026-11-09T00:00:00Z", &hash);
        assert!(matches!(
            manager.accept_revocation_list(&forged),
            Err(LifecycleError::Revocation(RevocationIntakeError::Verify(RevocationError::NotRevocationCapable(_))))
        ));

        // (b) Disable all stops what is left and answers host-off until cleared.
        assert_eq!(manager.disable_all(), vec!["test-plugin".to_string()]);
        assert_eq!(
            manager.check_activation(&request(LOCAL, "inst-1", "verse-1", &first)).unwrap_err().precondition,
            Precondition::HostOff
        );
        manager.enable_all();
        assert!(manager.check_activation(&request(LOCAL, "inst-1", "verse-1", &first)).is_ok());

        // OD-8 pulls go to https, or to an exact loopback host (a look-alike host or userinfo is refused).
        assert!(is_pull_url("https://roots.example/revocations.json") && is_pull_url("http://127.0.0.1:8080/l"));
        assert!(!is_pull_url("http://127.0.0.1.evil.example/l") && !is_pull_url("http://u@localhost/l"));

        // G4-A11 at the manager: a 2.0 build starts only as the build R19 judged (its component digest).
        let ticks = TickHost::new(Arc::new(crate::tick::ManualClock::default()), crate::scene_host::PluginContexts::default());
        manager = manager.with_ticks(ticks.clone());
        let fleet = |content: &str, component: &str, digest: &str| {
            PackageCheck::Verified(PackageFacts {
                component_digest: Some(component.repeat(32)),
                capability_digest: digest.into(),
                ..facts("fleet", &content.repeat(32))
            })
        };
        let (v1, v2) = (fleet("c1", "d1", "digest-1"), fleet("c2", "d2", "digest-2"));
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let guest = FakeGuest { log: log.clone(), refuse: false };
        manager.governance_mut().record_consent(consent("inst-3", "fleet", "digest-1"));
        let other_bytes = staged_build(&ticks, &"ee".repeat(32), &guest, true);
        let other_lease = other_bytes.launch.context.revocation_lease();
        assert!(matches!(
            manager.start_instance(&request(LOCAL, "inst-3", "verse-3", &v1), other_bytes),
            Err(LifecycleError::PayloadMismatch(_))
        ));
        assert!(other_lease.require_active().is_err(), "a refused build holds no lease");
        manager
            .start_instance(&request(LOCAL, "inst-3", "verse-3", &v1), staged_build(&ticks, &"d1".repeat(32), &guest, true))
            .unwrap();
        assert_eq!(manager.governance().running_in("verse-3"), vec!["fleet".to_string()]);
        assert_eq!(*log.lock().unwrap(), ["install", "activate"]);

        // P4.3: the update's grants are not consented yet, so R19 refuses before anything stops.
        let unconsented = manager.update_instance(&request(LOCAL, "inst-3", "verse-3", &v2), staged_build(&ticks, &"d2".repeat(32), &guest, false));
        assert!(matches!(unconsented, Err(LifecycleError::Refused(ref refusal)) if refusal.precondition == Precondition::ConsentMissing));
        assert_eq!(*log.lock().unwrap(), ["install", "activate"], "the running build was not touched");
        // The running id staged for another installation is no update (an install, consented anew): refused untouched.
        manager.governance_mut().record_consent(consent("inst-4", "fleet", "digest-2"));
        let foreign = staged_build(&ticks, &"d2".repeat(32), &guest, false);
        let foreign_lease = foreign.launch.context.revocation_lease();
        assert!(matches!(
            manager.update_instance(&request(LOCAL, "inst-4", "verse-3", &v2), foreign),
            Err(LifecycleError::LaunchMismatch(_))
        ));
        assert!(foreign_lease.require_active().is_err(), "a refused update holds no lease");
        assert_eq!(*log.lock().unwrap(), ["install", "activate"], "the running build was not touched");
        // Consented, but the new build refuses on-activate: the previous one is re-activated.
        manager.governance_mut().record_consent(consent("inst-3", "fleet", "digest-2"));
        let broken = FakeGuest { log: Arc::new(std::sync::Mutex::new(Vec::new())), refuse: true };
        assert!(matches!(
            manager.update_instance(&request(LOCAL, "inst-3", "verse-3", &v2), staged_build(&ticks, &"d2".repeat(32), &broken, false)),
            Err(LifecycleError::RolledBack(_))
        ));
        assert_eq!(*log.lock().unwrap(), ["install", "activate", "deactivate", "activate"]);
        manager
            .update_instance(&request(LOCAL, "inst-3", "verse-3", &v2), staged_build(&ticks, &"d2".repeat(32), &guest, false))
            .unwrap();
        assert_eq!(ticks.counters("fleet").map(|counters| counters.state), Some(TickState::Running));

        // The kill switch now tracks the UPDATED build: a list naming its component digest stops it.
        let list = signed_list(&revoker(), "revoker", "2026-11-09T00:00:00Z", &"d2".repeat(32));
        assert_eq!(manager.accept_revocation_list(&list).unwrap(), vec!["fleet".to_string()]);
        assert_eq!(ticks.counters("fleet").map(|counters| counters.state), Some(TickState::Stopped));
        // Uninstall forgets the instance's counters (G1-C10).
        manager.uninstall_instance("fleet");
        assert!(ticks.counters("fleet").is_none() && manager.governance().running_in("verse-3").is_empty());
    }

    #[tokio::test]
    async fn deactivate_plugin() {
        let (mut manager, _tmp) = make_test_manager();

        // Install first
        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        install_signed(&mut manager, "deact-test", &bytes);

        // Deactivate (should fail since it's Installed, not Active)
        let result = manager.deactivate_plugin("deact-test").await;
        assert!(result.is_err()); // Can't deactivate from Installed state
    }

    #[tokio::test]
    async fn uninstall_plugin_removes_everything() {
        let (mut manager, _tmp) = make_test_manager();

        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        install_signed(&mut manager, "uninstall-test", &bytes);

        manager
            .uninstall_plugin("uninstall-test", &bytes)
            .await
            .unwrap();

        // Registry should not have it anymore
        let reg = manager.registry.lock().unwrap();
        assert!(reg.get("uninstall-test").is_none());
    }

    #[test]
    fn manager_is_running() {
        let (manager, _tmp) = make_test_manager();
        assert!(!manager.is_running("nonexistent"));
    }
}
