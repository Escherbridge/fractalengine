//! Drains the Extensions panel's queued `ExtensionOp`s and the admin channel's `AdminJob`s on one worker thread against
//! the `PluginManager` (docs/extensions/extensions-ui.md §Operations seam, docs/extensions/admin-channel.md §4-§7).

use std::collections::{BTreeSet, HashMap};
use std::io::Read as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use crossbeam::channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use fe_database::handlers::extension_policy::{version_matches, AuditAction};
use fe_hexon::package::{verify_plugin_package, PluginPackage, PluginPackageError, PluginPackageStore, PluginTrust};
use fe_hexon::registry_source::{
    acquire_link, FetchedPackage, RegistryEntry, RegistryError, SearchQuery, SourceTrust, EXTENSION_KIND,
    PACKAGE_MAX_BYTES, WEB_BUNDLE_KIND,
};
use fe_plugin::admin::protocol::{
    AdminReply, AdminRequest, AdminResult, AdminVerb, BindingSummary, InstallationSummary, PendingConsent, RefusalCode,
};
use fe_plugin::lifecycle::{
    ActivationRefusal, ActivationRequest, ConsentRecord, LifecycleError, PackageCheck, PackageFacts, PluginManager,
    Precondition, StagedInstance, TrustTier, EXT_DISABLE_ALL_ENV,
};
use fe_runtime::extension_binding::{
    hook_spec, key_allowed, served_hook_ids, BindingMode, BindingOffer, BindingRecord, BindingResolution, HookKeyKind,
    UnresolvedReason, BINDING_SCHEMA_VERSION,
};
use fe_sdk::manifest_v2::{parse_and_validate, ValidatedManifest};
use fe_ui::extensions::bindings::{BindingPointRow, BindingsSnapshot, ProviderOption, ScopeTarget};
use fe_ui::extensions::browse::BrowseEvent;
use fe_ui::extensions::consent::{ConsentOrigin, ConsentRequest, ConsentSection, ConsentState, KillListOverrideOffer};
use fe_ui::extensions::details::{EntitlementState, StoreDetail, UpdateStage};
use fe_ui::extensions::panel::{
    AuditLine, BindingRow, ConsentSummary, ExtensionEvent, ExtensionOp, ExtensionsPanelState, InstalledRow, RowStatus,
    TrustState,
};
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::verse_manager::VerseManager;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use super::registry_offers::RegistryOffers;
use super::sources::SourceSet;

/// Audit `channel` of an action taken in the app window.
pub const CHANNEL_GUI: &str = "gui";
/// Audit `channel` of an action that arrived over the local admin channel (`fe_plugin::admin::protocol::AUDIT_CHANNEL`).
pub const CHANNEL_CLI: &str = fe_plugin::admin::protocol::AUDIT_CHANNEL;
/// Registry offers are re-read at most this often when nothing else asks (src/AGENTS.md §plugin-host "Registry offers").
const OFFER_REFRESH_PERIOD: Duration = Duration::from_secs(10 * 60);
/// The first registry-offer read after start, once navigation has seeded the scope.
const OFFER_FIRST_REFRESH: Duration = Duration::from_secs(20);

/// The verse and petal an op runs in: the navigation cursor when the op was queued.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveScope {
    pub verse_id: String,
    pub verse_name: String,
    pub petal_id: String,
    /// Display names for the Bindings view's scope labels.
    pub fractal_name: String,
    pub petal_name: String,
}

/// A build the binary must turn into a running instance (component, context, bus seat); `installation_id` is the tick key.
pub struct BuildToRun<'a> {
    pub installation_id: &'a str,
    pub package: &'a PluginPackage,
    pub facts: &'a PackageFacts,
    /// Run `on-install` before `on-activate` (the first start of this installation).
    pub first_activation: bool,
}

/// What only the binary can supply: the clock, durable audit writes, the verse Owner check and the component launch.
pub trait OpsPorts: Send {
    /// Current time as RFC 3339 UTC.
    fn now(&self) -> String;
    /// Trust tier of the root that vouched for a package (`None` is a dev-unsigned build).
    fn trust_tier(&self, trusted_by: Option<&str>) -> TrustTier;
    /// Whether the local DID is `verse_id`'s Owner (offers the kill-list override).
    fn is_verse_owner(&self, verse_id: &str) -> bool;
    /// Durable consent (one `consent` audit entry); must succeed before the in-memory record is written.
    fn persist_consent(&self, record: &ConsentRecord) -> Result<(), String>;
    /// Durable `consent-revoke` audit entry.
    fn persist_consent_revoke(&self, installation_id: &str, package_id: &str) -> Result<(), String>;
    /// Audited verse-Owner override (fe-database `record_kill_list_override` re-checks Owner and confirmation).
    fn record_kill_list_override(
        &self,
        verse_id: &str,
        package_id: &str,
        content_hash: &str,
        confirm: &str,
    ) -> Result<(), String>;
    /// Durable "Allow in <verse>" (one `bind` audit entry carrying the verse); must succeed before the in-memory record.
    fn persist_acceptance(&self, installation_id: &str, verse_id: &str, package_id: &str) -> Result<(), String>;
    /// One local lifecycle change (install, update, enable, disable, uninstall): audit it and re-resolve bindings.
    fn audit_change(&self, action: AuditAction, installation_id: &str);
    /// The binding plane at the open petal (points with this host's resolution, writable scopes, Owner flag);
    /// `providers` stays empty and offers are unfiltered: the worker owns both.
    fn bindings_snapshot(&self, scope: &ActiveScope) -> Result<BindingsSnapshot, String>;
    /// Write one binding row as the local DID; the DB plane judges Manager+ at the scope and the verse policy.
    fn write_binding(&self, template: BindingRecord) -> Result<(), String>;
    /// Instantiate the verified component and stage its context and bus seat.
    fn stage_launch(&self, build: &BuildToRun<'_>, scope: &ActiveScope) -> Result<StagedInstance, String>;
    /// Counter lines for the details view.
    fn counters(&self, _installation_id: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Audit lines for the details view, newest first.
    fn audit(&self, _installation_id: &str) -> Vec<AuditLine> {
        Vec::new()
    }
    /// [`Self::audit_change`] recorded with its audit `channel` (`gui` or `cli`, admin-channel.md §7).
    fn audit_change_via(&self, action: AuditAction, installation_id: &str, _channel: &str) {
        self.audit_change(action, installation_id);
    }
    /// One refused admin-channel verb (`outcome: refused`, `channel: "cli"`): every mutating verb is audited, denials included.
    fn audit_admin_refusal(&self, action: AuditAction, target: &str, code: &str, message: &str) {
        tracing::info!(?action, %target, %code, %message, "extension admin request refused");
    }
    /// The installation is gone (uninstalled, consent or not): the binding activator drops what it held for it.
    fn forget_installation(&self, _installation_id: &str) {}
    /// Durable `unbind` entry with the verse (withdrawn "Allow in <verse>"); must succeed before the in-memory change.
    fn persist_withdrawal(&self, _installation_id: &str, _verse_id: &str, _package_id: &str) -> Result<(), String> {
        Err("not-available: withdrawing an acceptance is not wired on this host".into())
    }
    /// Disable all switched on or off: audit `target: "*"` and tell every other kill-switch reader (the portal resolver).
    fn host_switch_changed(&self, disabled: bool, channel: &str) {
        let action = if disabled { AuditAction::Disable } else { AuditAction::Enable };
        self.audit_change_via(action, "*", channel);
    }
    /// `(licensee DID, device X25519 did:key)` an acquire link and a token request name.
    fn licence_identity(&self) -> Result<(String, String), String> {
        Err("not-available: licences need the entitlement feature on this host".into())
    }
    /// Ask a source's issuer for a licence token, verify it and keep it; `Missing` is "no licence yet".
    fn request_licence_token(&self, _request: &LicenceRequest<'_>) -> Result<LicenceAnswer, String> {
        Err("not-available: licences need the entitlement feature on this host".into())
    }
    /// Open an https page in the system browser.
    fn open_external(&self, url: &str) -> Result<(), String> {
        open_in_system_browser(url)
    }
    /// The registry offers changed: re-resolve `verse_id` (every verse when empty) so Install/Update can show.
    fn registry_offers_changed(&self, _verse_id: &str) {}
}

/// One licence token request (store-client contract v1 §entitlements).
pub struct LicenceRequest<'a> {
    pub source_id: &'a str,
    /// The endpoint base the issuer is reached under.
    pub base: &'a str,
    pub package_id: &'a str,
    /// The listed build the token must cover.
    pub content_hash: &'a str,
    /// The endpoint's `config.json`; `None` when unreachable (a held token still answers).
    pub config: Option<fe_hexon::registry_source::CatalogConfig>,
    /// The source's SSRF-checked http(s) transport; `None` for a folder endpoint.
    pub transport: Option<Arc<dyn fe_hexon::entitlement_client::TokenTransport>>,
}

/// What the issuer answered once the token verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenceAnswer {
    Held { expires: Option<String> },
    Missing,
}

/// Hand an https URL to the platform opener as one argument (no shell, so nothing in the URL is interpreted).
fn open_in_system_browser(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("invalid-input: {error}"))?;
    if parsed.scheme() != "https" {
        return Err("invalid-input: only https pages are opened".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("invalid-input: a page URL carries no credentials".into());
    }
    // explorer.exe splits its argument on commas (`/select,`, `/root,`); an encoded comma means the same to the page.
    let encoded = parsed.as_str().replace(',', "%2C");
    let url = encoded.as_str();
    #[cfg(windows)]
    let opened = std::process::Command::new("explorer").arg(url).spawn().map(drop);
    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg(url).status().map(drop);
    #[cfg(all(unix, not(target_os = "macos")))]
    let opened = std::process::Command::new("xdg-open").arg(url).status().map(drop);
    opened.map_err(|error| format!("could not open the browser: {error}"))
}

// ---- Admin channel jobs (admin-channel.md §4-§7) ----------------------------------------------------

/// One admin-channel request and where its replies go (at most one `pending-consent`, then one final reply).
pub struct AdminJob {
    pub request: AdminRequest,
    pub replies: UnboundedSender<AdminReply>,
}

/// The admin channel's queue into the ops worker (pinned for `admin_channel::spawn`).
#[derive(Resource, Clone)]
pub struct AdminJobSender(Sender<WorkerInput>);

impl AdminJobSender {
    /// Queue one job; gives it back when the worker is gone.
    pub fn send(&self, job: AdminJob) -> Result<(), AdminJob> {
        self.0.send(WorkerInput::Admin(job)).map_err(|error| match error.into_inner() {
            WorkerInput::Admin(job) => job,
            WorkerInput::Panel(_) => unreachable!("only admin jobs are sent here"),
        })
    }
}

/// Everything the worker serves, in arrival order.
pub enum WorkerInput {
    Panel(OpsJob),
    Admin(AdminJob),
}

/// A CLI request waiting on the GUI consent dialog for one staged build.
struct PendingAdmin {
    replies: UnboundedSender<AdminReply>,
    action: AuditAction,
}

/// A final admin reply (refused) carrying its stable code.
struct AdminRefusal {
    code: RefusalCode,
    message: String,
}

impl AdminRefusal {
    fn new(code: RefusalCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// Stable code of a worker message: the governance code it starts with (`revoked: …`, `package-invalid (…)`), if any.
fn refusal_of(message: String) -> AdminRefusal {
    let head: String = message.chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
    let code = match RefusalCode::from(head) {
        RefusalCode::Other(_) => {
            if message.contains("is not installed") || message.contains("unknown extension source") {
                RefusalCode::NotFound
            } else if message.starts_with("stale-capability-digest") {
                RefusalCode::ConsentMissing
            } else {
                RefusalCode::NotAvailable
            }
        }
        known => known,
    };
    AdminRefusal { code, message }
}

/// Stable code of a store-client refusal.
fn registry_refusal(error: RegistryError) -> AdminRefusal {
    let code = match &error {
        RegistryError::Revoked { .. } => RefusalCode::Revoked,
        RegistryError::NotFound(_) | RegistryError::Yanked(..) => RefusalCode::NotFound,
        RegistryError::PackageUntrusted(_) => RefusalCode::UntrustedSigner,
        RegistryError::PackageInvalid(_)
        | RegistryError::PackageContract
        | RegistryError::PublisherMismatch { .. }
        | RegistryError::KindMismatch { .. }
        | RegistryError::VersionMismatch { .. }
        | RegistryError::ContentHashMismatch { .. }
        | RegistryError::TooLarge { .. } => RefusalCode::PackageInvalid,
        RegistryError::AccessUnknown { .. } => RefusalCode::NotAvailable,
        _ => RefusalCode::NotAvailable,
    };
    AdminRefusal::new(code, error.to_string())
}

/// The governance audit action a verb records (admin-channel.md §4).
fn verb_action(verb: &AdminVerb) -> Option<AuditAction> {
    Some(match verb {
        AdminVerb::List {} | AdminVerb::Status { .. } => return None,
        AdminVerb::InstallFile { .. } | AdminVerb::InstallRegistry { .. } => AuditAction::Install,
        AdminVerb::Enable { .. } | AdminVerb::EnableAll {} => AuditAction::Enable,
        AdminVerb::Disable { .. } | AdminVerb::DisableAll {} => AuditAction::Disable,
        AdminVerb::Update { .. } => AuditAction::Update,
        AdminVerb::Unbind { .. } => AuditAction::Unbind,
        AdminVerb::Uninstall { .. } => AuditAction::Uninstall,
    })
}

/// What an admin verb ended in.
enum AdminOutcome {
    Done(AdminResult),
    Refused(AdminRefusal),
    /// The GUI dialog is open for this staged build; the reply waits for the operator.
    Pending(PendingConsent),
}

/// How a panel op settles a CLI request waiting on the same installation.
enum Settle {
    /// Consent or a consented update activation: done when the staged build was promoted.
    Promote,
    /// The operator declined or discarded.
    Declined,
    /// An override answers with a fresh dialog; only a failure settles.
    Override,
    /// The installation was removed while the dialog was open.
    Removed,
}

fn summary_of(row: InstalledRow) -> InstallationSummary {
    InstallationSummary {
        installation_id: row.installation_id,
        package_id: row.package_id,
        name: row.name,
        version: row.version,
        status: row.status.label(),
        update_available: row.update_available,
        bindings: row
            .bindings
            .into_iter()
            .map(|binding| BindingSummary { binding_id: binding.binding_id, label: binding.label, enabled: binding.enabled })
            .collect(),
    }
}

/// What the panel and consent dialog show about one verified build.
struct BuildInfo {
    facts: PackageFacts,
    name: String,
    description: String,
    publisher_did: String,
    /// The trust root that vouched (full DID); `None` for a dev-unsigned build.
    trusted_by: Option<String>,
    signer: String,
    trusted: bool,
    sections: Vec<ConsentSection>,
}

impl BuildInfo {
    /// Another publisher or vouching root than `installed`: an update of this kind is a NEW install (fresh consent).
    fn publisher_change(&self, installed: &BuildInfo) -> Option<ConsentSection> {
        let same = self.publisher_did == installed.publisher_did && self.trusted_by == installed.trusted_by;
        (!same).then(|| ConsentSection {
            title: "Publisher changed".to_owned(),
            lines: vec![
                format!("was {} (signed by {})", short_did(&installed.publisher_did), installed.signer),
                format!("now {} (signed by {})", short_did(&self.publisher_did), self.signer),
            ],
        })
    }
}

/// Sealed bytes verified and held until the dialog answers; nothing of it is stored or run yet.
struct StagedBuild {
    bytes: Vec<u8>,
    package: PluginPackage,
    info: BuildInfo,
    is_update: bool,
    /// How the dialog was reached; an override's fresh review keeps it (the offer's verse rides along).
    origin: ConsentOrigin,
}

/// A build in the package store with the consent record keyed by its package id.
struct InstalledBuild {
    info: BuildInfo,
    activated_once: bool,
}

enum RunMode {
    Start,
    Update,
}

/// One binding write the Bindings view asked for (`SetBinding`, or `ClearBinding` as `inherit`).
struct BindingWrite {
    target: ScopeTarget,
    hook: String,
    key: String,
    mode: BindingMode,
    package_id: Option<String>,
    version_req: Option<String>,
    config: Value,
    runs_on: Option<String>,
}

enum RunError {
    /// The new build failed to start and the previous one runs again.
    RolledBack(String),
    Failed(String),
}

/// The worker: owns installation bookkeeping, runs on its own thread, never touches the Bevy world.
pub struct OpsWorker {
    manager: Arc<Mutex<PluginManager>>,
    store: PluginPackageStore,
    trust: SourceTrust,
    sources: SourceSet,
    ports: Box<dyn OpsPorts>,
    installed: HashMap<String, InstalledBuild>,
    staged: HashMap<String, StagedBuild>,
    /// Newer versions seen in listings, by package id: `(source_id, entry)`.
    offers: HashMap<String, (String, RegistryEntry)>,
    /// CLI requests whose consent dialog is open in the GUI, by installation id.
    pending_admin: HashMap<String, PendingAdmin>,
    /// Licence state per `(source_id, package_id)` from this session's token requests.
    entitlements: HashMap<(String, String), EntitlementState>,
    /// The GUI's navigation cursor, kept current by the pump; admin jobs run in it.
    gui_scope: Arc<Mutex<ActiveScope>>,
    /// Audit channel of the job being served.
    audit_channel: &'static str,
    /// Install/Update builds shared with the plugin worker's `LocalFacts.registry`.
    registry_offers: RegistryOffers,
    /// Package ids the open petal's unresolved bindings name (not-installed, version-unsatisfied, not-provided).
    offer_packages: BTreeSet<String>,
    /// The ids the last offer refresh covered; a new id refreshes at once.
    offers_covered: BTreeSet<String>,
    /// When the periodic offer refresh is next due.
    offers_next_refresh: Instant,
}

fn lock(manager: &Mutex<PluginManager>) -> MutexGuard<'_, PluginManager> {
    manager.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// R19 for a verified build in `scope`, as the host would judge an enable now.
fn judge(
    manager: &PluginManager,
    installation_id: &str,
    scope: &ActiveScope,
    facts: &PackageFacts,
) -> Result<(), ActivationRefusal> {
    let check = PackageCheck::Verified(facts.clone());
    let did = manager.governance().local_did().to_owned();
    let request = ActivationRequest {
        actor_did: &did,
        installation_id,
        verse_id: &scope.verse_id,
        petal_id: &scope.petal_id,
        package: &check,
    };
    manager.check_activation(&request).map(|_| ())
}

fn describe_package_error(error: PluginPackageError) -> String {
    match error {
        PluginPackageError::Invalid(reason) => format!("package-invalid ({reason})"),
        PluginPackageError::UntrustedSigner(reason) => format!("untrusted-signer ({reason})"),
        PluginPackageError::UnsupportedContract => "package-invalid (unsupported contract)".into(),
    }
}

/// `major.minor.patch` of a SemVer string, pre-release and build ignored.
pub(super) fn semver_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((semver_triplet(candidate), semver_triplet(current)), (Some(a), Some(b)) if a > b)
}

/// Package ids of published resolutions a registry build could turn into Install or Update (bindings.md §5.5).
fn offer_packages_of(snapshot: &BindingsSnapshot) -> BTreeSet<String> {
    snapshot
        .points
        .iter()
        .filter_map(|row| match &row.resolution {
            Some(BindingResolution::Unresolved {
                package_id,
                reason: UnresolvedReason::NotInstalled | UnresolvedReason::VersionUnsatisfied | UnresolvedReason::NotProvided,
                ..
            }) => Some(package_id.clone()),
            _ => None,
        })
        .collect()
}

fn short_did(did: &str) -> String {
    if did.chars().count() > 24 {
        let head: String = did.chars().take(16).collect();
        let tail: String = did.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{head}...{tail}")
    } else {
        did.to_owned()
    }
}

const GRANT_KEYS: [(&str, &str); 9] = [
    ("scene", "Scene"),
    ("readings", "Readings"),
    ("routes", "Routes"),
    ("kv", "Storage"),
    ("ingress", "Ingress"),
    ("egress", "Egress"),
    ("bus", "Bus"),
    ("ui", "Interface"),
    ("provides", "Provides"),
];

fn compact(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn grant_lines(value: &Value) -> Vec<String> {
    match value {
        Value::Null | Value::Bool(false) => Vec::new(),
        Value::Bool(true) => vec!["Allowed".into()],
        Value::Array(items) => items.iter().map(compact).collect(),
        Value::Object(map) => map
            .iter()
            .filter(|(_, inner)| !inner.is_null())
            .map(|(key, inner)| format!("{key}: {}", compact(inner)))
            .collect(),
        other => vec![compact(other)],
    }
}

/// One titled group per granted manifest section; lines are stable text so the version delta can diff them.
fn grant_sections(raw: &Value) -> Vec<ConsentSection> {
    GRANT_KEYS
        .iter()
        .filter_map(|(key, title)| {
            let lines = grant_lines(raw.get(*key)?);
            (!lines.is_empty()).then(|| ConsentSection { title: (*title).to_owned(), lines })
        })
        .collect()
}

/// Read a package file under the package cap.
fn read_package_file(path: &str) -> Result<Vec<u8>, String> {
    // Checked before the open: opening a FIFO or device path would block the single ops worker (R-ADMIN-06).
    if !std::fs::metadata(path).map_err(|error| format!("{path}: {error}"))?.is_file() {
        return Err(format!("{path}: not a file"));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let metadata = file.metadata().map_err(|error| format!("{path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{path}: not a file"));
    }
    let mut bytes = Vec::new();
    file.take(PACKAGE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{path}: {error}"))?;
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        return Err(format!("{path}: exceeds {PACKAGE_MAX_BYTES} bytes"));
    }
    Ok(bytes)
}

impl OpsWorker {
    /// Worker over `manager`; call [`Self::hydrate`] before serving ops.
    pub fn new(
        manager: Arc<Mutex<PluginManager>>,
        store: PluginPackageStore,
        trust: SourceTrust,
        sources: SourceSet,
        ports: Box<dyn OpsPorts>,
    ) -> Self {
        Self {
            manager,
            store,
            trust,
            sources,
            ports,
            installed: HashMap::new(),
            staged: HashMap::new(),
            offers: HashMap::new(),
            pending_admin: HashMap::new(),
            entitlements: HashMap::new(),
            gui_scope: Arc::new(Mutex::new(ActiveScope::default())),
            audit_channel: CHANNEL_GUI,
            registry_offers: RegistryOffers::new(),
            offer_packages: BTreeSet::new(),
            offers_covered: BTreeSet::new(),
            offers_next_refresh: Instant::now() + OFFER_FIRST_REFRESH,
        }
    }

    /// Share `offers` with the plugin worker (the binding resolver reads them as `LocalFacts.registry`).
    pub(crate) fn with_registry_offers(mut self, offers: RegistryOffers) -> Self {
        self.registry_offers = offers;
        self
    }

    /// One local lifecycle change, audited on the channel of the job being served.
    fn audit(&self, action: AuditAction, installation_id: &str) {
        self.ports.audit_change_via(action, installation_id, self.audit_channel);
    }

    fn plugin_trust(&self) -> PluginTrust<'_> {
        PluginTrust { is_trust_root: &*self.trust.is_trust_root, allow_unsigned_dev: self.trust.allow_unsigned_dev }
    }

    /// Rebuild the installed list from the package store, re-verifying every build; the last build per id wins.
    pub fn hydrate(&mut self) {
        let listed = match self.store.list() {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(%error, "extension package store unreadable");
                return;
            }
        };
        for (package_id, content_hash) in listed {
            let bytes = match self.store.get(&package_id, &content_hash) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%package_id, %error, "stored extension unreadable");
                    continue;
                }
            };
            match self.verify(&bytes, Some(&package_id)) {
                Ok((_, info)) => {
                    self.installed.insert(package_id, InstalledBuild { info, activated_once: false });
                }
                Err(reason) => tracing::warn!(%package_id, %reason, "stored extension failed re-verification"),
            }
        }
    }

    /// Serve panel ops and admin jobs, in arrival order, until every sender is gone; idle, refresh registry offers.
    pub fn run(mut self, jobs: Receiver<WorkerInput>, events: Sender<ExtensionEvent>) {
        loop {
            let wait = self.offers_next_refresh.saturating_duration_since(Instant::now());
            let input = match jobs.recv_timeout(wait) {
                Ok(input) => input,
                Err(RecvTimeoutError::Timeout) => {
                    self.periodic_offer_refresh();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let produced = match input {
                WorkerInput::Panel(job) => {
                    job.ops.into_iter().flat_map(|op| self.handle(op, &job.scope)).collect::<Vec<_>>()
                }
                WorkerInput::Admin(job) => self.handle_admin(job),
            };
            for event in produced {
                if events.send(event).is_err() {
                    return;
                }
            }
        }
    }

    /// Execute one panel op; every outcome ends in an event that clears the panel's working state.
    /// An op answering a CLI request's dialog also settles that request.
    pub fn handle(&mut self, op: ExtensionOp, scope: &ActiveScope) -> Vec<ExtensionEvent> {
        let settles = match &op {
            ExtensionOp::Consent { installation_id, .. } | ExtensionOp::ActivateUpdate { installation_id, .. } => {
                Some((installation_id.clone(), Settle::Promote))
            }
            ExtensionOp::DiscardStaged { installation_id } => Some((installation_id.clone(), Settle::Declined)),
            ExtensionOp::KillListOverride { installation_id, .. } => Some((installation_id.clone(), Settle::Override)),
            ExtensionOp::Uninstall { installation_id } => Some((installation_id.clone(), Settle::Removed)),
            _ => None,
        };
        let result = self.dispatch(op, scope);
        if let Some((installation_id, settle)) = settles {
            self.settle_admin(&installation_id, settle, &result, scope);
        }
        match result {
            Ok(events) => events,
            Err(message) => vec![ExtensionEvent::Failed { message }],
        }
    }

    fn dispatch(&mut self, op: ExtensionOp, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        match op {
            ExtensionOp::Refresh => Ok(vec![
                ExtensionEvent::Browse(BrowseEvent::Sources(self.sources.browse_sources())),
                self.rows_event(scope),
            ]),
            ExtensionOp::BrowseList { source_id, query } => Ok(self.browse_list(source_id, &query, scope)),
            ExtensionOp::Search { source_id, query } => Ok(self.search(source_id, &query, scope)),
            ExtensionOp::LoadDetail { source_id, entry } => Ok(vec![self.load_detail(source_id, &entry)]),
            ExtensionOp::OpenAcquire { source_id, package_id } => Ok(vec![self.open_acquire(source_id, package_id)]),
            ExtensionOp::RequestLicenceToken { source_id, package_id } => {
                Ok(vec![self.request_licence(source_id, package_id)])
            }
            ExtensionOp::InstallFromFile { path } => {
                let bytes = read_package_file(&path)?;
                self.stage(bytes, None, scope)
            }
            ExtensionOp::InstallFromRegistry { source_id, entry } => self.fetch_and_stage(&source_id, &entry, scope),
            ExtensionOp::Update { installation_id } => {
                let (source_id, entry) = self
                    .offers
                    .get(&installation_id)
                    .cloned()
                    .ok_or_else(|| "no newer version is known; refresh Browse".to_owned())?;
                self.fetch_and_stage(&source_id, &entry, scope)
            }
            ExtensionOp::Enable { installation_id, binding_id } => self.enable(&installation_id, &binding_id, scope),
            ExtensionOp::Disable { installation_id, .. } => {
                lock(&self.manager).disable_instance(&installation_id);
                self.audit(AuditAction::Disable, &installation_id);
                Ok(vec![self.rows_event(scope)])
            }
            ExtensionOp::ActivateUpdate { installation_id, capability_digest } => {
                self.activate_update(&installation_id, &capability_digest, scope)
            }
            ExtensionOp::Consent { installation_id, capability_digest, accept_in_verse } => {
                self.consent(&installation_id, &capability_digest, accept_in_verse, scope)
            }
            ExtensionOp::KillListOverride { installation_id, verse_id, package_id, content_hash, confirm } => {
                self.kill_list_override(&installation_id, &verse_id, &package_id, &content_hash, &confirm, scope)
            }
            ExtensionOp::DiscardStaged { installation_id } => {
                self.staged.remove(&installation_id);
                Ok(vec![self.rows_event(scope)])
            }
            ExtensionOp::RevokeConsent { installation_id } => self.revoke_consent(&installation_id, scope),
            ExtensionOp::Uninstall { installation_id } => self.uninstall(&installation_id, scope),
            ExtensionOp::SetHostSwitch { disabled } => {
                self.set_host_switch(disabled, CHANNEL_GUI).map_err(|message| format!("host-off: {message}"))?;
                Ok(vec![self.rows_event(scope)])
            }
            ExtensionOp::RefreshBindings => Ok(vec![self.bindings_event(scope)?]),
            ExtensionOp::SetBinding { target, hook, key, mode, package_id, version_req, config, runs_on } => {
                let template = BindingWrite { target, hook, key, mode, package_id, version_req, config, runs_on };
                self.write_binding(template, scope)
            }
            ExtensionOp::ClearBinding { target, hook, key } => {
                let template = BindingWrite {
                    target,
                    hook,
                    key,
                    mode: BindingMode::Inherit,
                    package_id: None,
                    version_req: None,
                    config: Value::Object(serde_json::Map::new()),
                    runs_on: None,
                };
                self.write_binding(template, scope)
            }
            ExtensionOp::TakeBindingOffer { offer, verse_id, hook, key, package_id } => {
                self.take_binding_offer(offer, &verse_id, &hook, &key, &package_id, scope)
            }
        }
    }

    // ---- Browse -----------------------------------------------------------------------------

    fn browse_list(&mut self, source_id: String, query: &str, scope: &ActiveScope) -> Vec<ExtensionEvent> {
        let Some(source) = self.sources.get(&source_id).cloned() else {
            return vec![ExtensionEvent::Browse(BrowseEvent::Failed { source_id, reason: "unknown extension source".into() })];
        };
        let query = Some(query.trim()).filter(|text| !text.is_empty());
        match source.list(query) {
            Ok(listing) => {
                self.note_offers(&source_id, &listing.entries);
                if !self.offer_packages.is_empty() {
                    // The listing answers first; the offer refresh runs once the worker is idle.
                    self.offers_next_refresh = Instant::now();
                }
                vec![ExtensionEvent::Browse(BrowseEvent::Listing { source_id, listing }), self.rows_event(scope)]
            }
            Err(error) => {
                vec![ExtensionEvent::Browse(BrowseEvent::Failed { source_id, reason: error.to_string() })]
            }
        }
    }

    /// Search a catalogue endpoint (hosted when advertised, else the verified index); other sources answer a listing.
    fn search(&mut self, source_id: String, query: &SearchQuery, scope: &ActiveScope) -> Vec<ExtensionEvent> {
        let Some(catalog) = self.sources.catalog(&source_id).cloned() else {
            return self.browse_list(source_id, query.q.as_deref().unwrap_or_default(), scope);
        };
        match catalog.search(query) {
            Ok(results) => {
                // Only verified, unrevoked extension builds become update offers (kill list at offer).
                let offered: Vec<RegistryEntry> = results
                    .hits
                    .iter()
                    .filter(|hit| hit.verified && !hit.revoked && hit.entry.kind == EXTENSION_KIND)
                    .map(|hit| hit.entry.to_registry_entry())
                    .collect();
                self.note_offers(&source_id, &offered);
                vec![ExtensionEvent::Browse(BrowseEvent::Results { source_id, results }), self.rows_event(scope)]
            }
            Err(error) => vec![ExtensionEvent::Browse(BrowseEvent::Failed { source_id, reason: error.to_string() })],
        }
    }

    /// One package's hash-pinned detail plus its attestation (the "Reviewed" badge only from a verified counter-signature).
    fn load_detail(&self, source_id: String, entry: &RegistryEntry) -> ExtensionEvent {
        let Some(catalog) = self.sources.catalog(&source_id) else {
            let reason = "this source publishes no package details".to_owned();
            return ExtensionEvent::Browse(BrowseEvent::Failed { source_id, reason });
        };
        let verified = match catalog.detail(&entry.package_id) {
            Ok(verified) => verified,
            Err(error) => return ExtensionEvent::Browse(BrowseEvent::Failed { source_id, reason: error.to_string() }),
        };
        let latest = verified.listed.to_registry_entry();
        let attestation = match catalog.attest(&latest) {
            Ok(attestation) => Some(attestation),
            Err(error) => {
                tracing::debug!(package_id = %entry.package_id, %error, "no verified attestation for the details view");
                None
            }
        };
        let entitlement = self.entitlement(&source_id, &entry.package_id);
        let detail = StoreDetail::from_verified(&verified, attestation.as_ref(), entitlement);
        ExtensionEvent::Browse(BrowseEvent::Detail { source_id, detail: Box::new(detail) })
    }

    fn entitlement(&self, source_id: &str, package_id: &str) -> EntitlementState {
        self.entitlements.get(&(source_id.to_owned(), package_id.to_owned())).cloned().unwrap_or_default()
    }

    /// Open the checked acquire page with the licensee and device ids appended; the client sends no credential itself.
    fn open_acquire(&self, source_id: String, package_id: String) -> ExtensionEvent {
        let opened = (|| -> Result<(), String> {
            let catalog = self.sources.catalog(&source_id).ok_or("this source sells no licences")?;
            let verified = catalog.detail(&package_id).map_err(|error| error.to_string())?;
            let acquire_url = verified.acquire_url.as_deref().ok_or("this package has no licence page")?;
            let (licensee, device) = self.ports.licence_identity()?;
            let link = acquire_link(acquire_url, &licensee, &device).map_err(|error| error.to_string())?;
            self.ports.open_external(&link)
        })();
        let state = match opened {
            Ok(()) => self.entitlement(&source_id, &package_id),
            Err(reason) => EntitlementState::Refused(reason),
        };
        ExtensionEvent::Browse(BrowseEvent::Entitlement { source_id, package_id, state })
    }

    /// Ask the issuer for a token over the listed build; every outcome answers so the view leaves "Checking".
    fn request_licence(&mut self, source_id: String, package_id: String) -> ExtensionEvent {
        let answer = (|| -> Result<LicenceAnswer, String> {
            let catalog = self.sources.catalog(&source_id).ok_or("this source sells no licences")?;
            let base = self.sources.base(&source_id).ok_or("this source sells no licences")?;
            let verified = catalog.detail(&package_id).map_err(|error| error.to_string())?;
            let request = LicenceRequest {
                source_id: &source_id,
                base,
                package_id: &package_id,
                content_hash: &verified.listed.latest_content_hash,
                config: catalog.config().ok(),
                transport: self.sources.token_transport(&source_id),
            };
            self.ports.request_licence_token(&request)
        })();
        let state = match answer {
            Ok(LicenceAnswer::Held { expires }) => EntitlementState::Held { expires },
            Ok(LicenceAnswer::Missing) => EntitlementState::Missing,
            Err(reason) => EntitlementState::Refused(reason),
        };
        self.entitlements.insert((source_id.clone(), package_id.clone()), state.clone());
        ExtensionEvent::Browse(BrowseEvent::Entitlement { source_id, package_id, state })
    }

    /// Remember listed versions newer than the installed ones (an update is offered from the source that listed it).
    fn note_offers(&mut self, source_id: &str, entries: &[RegistryEntry]) {
        for entry in entries {
            let Some(installed) = self.installed.get(&entry.package_id) else { continue };
            if is_newer(&entry.version, &installed.info.facts.version) {
                self.offers.insert(entry.package_id.clone(), (source_id.to_owned(), entry.clone()));
            } else {
                self.offers.remove(&entry.package_id);
            }
        }
    }

    // ---- Staging ----------------------------------------------------------------------------

    /// Verify sealed bytes end to end (seal, trust, identity, manifest v2) into facts the host judges.
    fn verify(&self, bytes: &[u8], expected_id: Option<&str>) -> Result<(PluginPackage, BuildInfo), String> {
        let package = verify_plugin_package(bytes, expected_id, &self.plugin_trust()).map_err(describe_package_error)?;
        let validated =
            parse_and_validate(&package.extension_json).map_err(|error| format!("package-invalid ({error})"))?;
        if validated.manifest.id != package.package_id {
            return Err("package-invalid (manifest names another package)".into());
        }
        let info = self.build_info(&package, &validated);
        Ok((package, info))
    }

    fn build_info(&self, package: &PluginPackage, validated: &ValidatedManifest) -> BuildInfo {
        // A licence that cannot be read is treated as paid so row 6 asks the entitlement verifier.
        let licensed = match package.license_json.as_deref() {
            None => false,
            Some(raw) => match serde_json::from_slice::<fe_format::license::License>(raw) {
                Ok(license) => matches!(license.license_type, fe_format::license::LicenseType::Paid),
                Err(_) => true,
            },
        };
        let manifest = &validated.manifest;
        let facts = PackageFacts {
            package_id: package.package_id.clone(),
            version: package.version.clone(),
            content_hash: package.content_hash.clone(),
            component_digest: Some(package.component_digest.clone()),
            plaintext_digest: None,
            capability_digest: validated.capability_digest.clone(),
            trust: self.ports.trust_tier(package.trusted_by.as_deref()),
            writes: PackageFacts::manifest_writes(manifest),
            licensed,
            manifest: Some(Arc::new(manifest.clone())),
        };
        let raw: Value = serde_json::from_slice(&package.extension_json).unwrap_or(Value::Null);
        BuildInfo {
            facts,
            name: manifest.name.clone(),
            description: manifest.description.clone().unwrap_or_default(),
            publisher_did: package.publisher_did.clone(),
            trusted_by: package.trusted_by.clone(),
            signer: package.trusted_by.as_deref().map_or_else(|| "unsigned".to_owned(), short_did),
            trusted: package.trusted_by.is_some(),
            sections: grant_sections(&raw),
        }
    }

    fn fetch_and_stage(
        &mut self,
        source_id: &str,
        entry: &RegistryEntry,
        scope: &ActiveScope,
    ) -> Result<Vec<ExtensionEvent>, String> {
        self.fetch_and_stage_with(source_id, entry, scope, ConsentOrigin::Direct)
    }

    /// Fetch through a configured source (anonymous, verified); a web bundle installs through the portal path, an
    /// extension is staged with the dialog's origin.
    fn fetch_and_stage_with(
        &mut self,
        source_id: &str,
        entry: &RegistryEntry,
        scope: &ActiveScope,
        origin: ConsentOrigin,
    ) -> Result<Vec<ExtensionEvent>, String> {
        let fetched = self.fetch_entry(source_id, entry).map_err(|error| error.to_string())?;
        if fetched.kind() == WEB_BUNDLE_KIND {
            self.install_web_bundle(&fetched)?;
            return Ok(vec![self.rows_event(scope)]);
        }
        self.stage_with(fetched.bytes, Some(&entry.package_id), scope, origin)
    }

    /// Verified bytes of `entry` from a configured source (kill list at fetch, anonymous, capped).
    fn fetch_entry(&self, source_id: &str, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        let source = self
            .sources
            .get(source_id)
            .cloned()
            .ok_or_else(|| RegistryError::NotFound(format!("unknown extension source {source_id}")))?;
        source.fetch(entry)
    }

    /// A verified web bundle into the local blob store (P-STORE); nothing starts, admission stays per peer.
    fn install_web_bundle(&self, fetched: &FetchedPackage) -> Result<String, String> {
        use crate::portal_host::install::{install_from_registry, Refusal};
        let installed = install_from_registry(fetched).map_err(|refusal| {
            let code = match &refusal {
                Refusal::KillListed { .. } => "revoked",
                Refusal::NotConfigured => "not-available",
                Refusal::Store(_) => "internal",
                _ => "package-invalid",
            };
            format!("{code}: {refusal}")
        })?;
        self.audit(AuditAction::Install, &installed.package_id);
        tracing::info!(package_id = %installed.package_id, version = %installed.version, "web bundle installed");
        Ok(format!("Installed web bundle {} {}; admit it in a petal before it runs", installed.package_id, installed.version))
    }

    /// Verify, hold the build and ask for consent; an installed id stages an update of that installation.
    fn stage(&mut self, bytes: Vec<u8>, expected_id: Option<&str>, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        self.stage_with(bytes, expected_id, scope, ConsentOrigin::Direct)
    }

    /// [`Self::stage`] with the dialog's origin (an install offer carries the verse it accepts in).
    fn stage_with(
        &mut self,
        bytes: Vec<u8>,
        expected_id: Option<&str>,
        scope: &ActiveScope,
        origin: ConsentOrigin,
    ) -> Result<Vec<ExtensionEvent>, String> {
        let (package, info) = self.verify(&bytes, expected_id)?;
        self.stage_verified(bytes, package, info, scope, origin)
    }

    /// Hold a verified build and build its consent request.
    fn stage_verified(
        &mut self,
        bytes: Vec<u8>,
        package: PluginPackage,
        info: BuildInfo,
        scope: &ActiveScope,
        origin: ConsentOrigin,
    ) -> Result<Vec<ExtensionEvent>, String> {
        let installation_id = package.package_id.clone();
        let is_update = match self.installed.get(&installation_id) {
            Some(installed) if installed.info.facts.content_hash.eq_ignore_ascii_case(&package.content_hash) => {
                return Err(format!("{installation_id} {} is already installed", package.version));
            }
            Some(_) => true,
            None => false,
        };
        let mut request = self.consent_request(&installation_id, &info, is_update, scope);
        request.origin = origin.clone();
        self.staged.insert(installation_id, StagedBuild { bytes, package, info, is_update, origin });
        Ok(vec![ExtensionEvent::ConsentNeeded { request, is_update }])
    }

    /// The dialog's data; the refusal and kill-list state come from R19 as the host would judge an enable now.
    /// An update from another publisher or root carries no consented digest, so the dialog always opens.
    fn consent_request(&self, installation_id: &str, info: &BuildInfo, is_update: bool, scope: &ActiveScope) -> ConsentRequest {
        let manager = lock(&self.manager);
        let (blocked, kill_list, policy_verdict) = match judge(&manager, installation_id, scope, &info.facts) {
            Ok(()) => (None, None, "Admitted by verse and local policy".to_owned()),
            Err(refusal) if refusal.precondition == Precondition::ConsentMissing => {
                (None, None, "Admitted by verse and local policy".to_owned())
            }
            Err(refusal) if refusal.precondition == Precondition::Revoked => {
                if !scope.verse_id.is_empty() && self.ports.is_verse_owner(&scope.verse_id) {
                    let offer = KillListOverrideOffer {
                        reason: refusal.detail.clone(),
                        verse_id: scope.verse_id.clone(),
                        verse_name: scope.verse_name.clone(),
                    };
                    (None, Some(offer), refusal.to_string())
                } else {
                    (Some(refusal.to_string()), None, refusal.to_string())
                }
            }
            Err(refusal) => (Some(refusal.to_string()), None, refusal.to_string()),
        };
        let installed = self.installed.get(installation_id).filter(|_| is_update);
        let publisher_change = installed.and_then(|build| info.publisher_change(&build.info));
        // Steer 2026-11-09 01:50 (3): same digest, other publisher is a new install, never a silent update.
        let consented_digest = installed
            .filter(|_| publisher_change.is_none())
            .and_then(|_| manager.governance().consent(installation_id))
            .map(|record| record.capability_digest.clone());
        let mut sections = info.sections.clone();
        if let Some(change) = publisher_change {
            sections.insert(0, change);
        }
        ConsentRequest {
            installation_id: installation_id.to_owned(),
            package_id: info.facts.package_id.clone(),
            name: info.name.clone(),
            version: info.facts.version.clone(),
            kind: info
                .facts
                .manifest
                .as_ref()
                .and_then(|manifest| serde_json::to_value(&manifest.kind).ok())
                .and_then(|kind| kind.as_str().map(str::to_owned))
                .unwrap_or_default(),
            content_hash: info.facts.content_hash.clone(),
            capability_digest: info.facts.capability_digest.clone(),
            consented_digest,
            previous_sections: installed.map(|build| build.info.sections.clone()).unwrap_or_default(),
            sections,
            signer: info.trusted.then(|| format!("{} (trusted root)", info.signer)),
            policy_verdict,
            licence: if info.facts.licensed { "Paid".to_owned() } else { "Free".to_owned() },
            blocked,
            kill_list,
            origin: ConsentOrigin::Direct,
            // Extension packages only; a web bundle's portal section is the portal track's consent path.
            portal: None,
        }
    }

    // ---- Consent, override, activation ------------------------------------------------------

    /// Digest-pinned consent: durable first, then in memory, then install and (in the open scope) start.
    fn consent(
        &mut self,
        installation_id: &str,
        capability_digest: &str,
        accept_in_verse: Option<String>,
        scope: &ActiveScope,
    ) -> Result<Vec<ExtensionEvent>, String> {
        let staged = self.staged.get(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        if staged.info.facts.capability_digest != capability_digest {
            return Err(format!(
                "stale-capability-digest: the staged build of {installation_id} asks for different grants than the review showed"
            ));
        }
        // R19 below judges the open verse; an acceptance for another verse would skip that verse's policy.
        if accept_in_verse.as_deref().is_some_and(|verse| verse != scope.verse_id) {
            return Err("open the verse the offer is for first; nothing was recorded".into());
        }
        {
            let manager = lock(&self.manager);
            if let Err(refusal) = judge(&manager, installation_id, scope, &staged.info.facts) {
                if refusal.precondition != Precondition::ConsentMissing {
                    return Err(refusal.to_string());
                }
            }
        }
        let record = ConsentRecord {
            installation_id: installation_id.to_owned(),
            package_id: staged.info.facts.package_id.clone(),
            content_hash: staged.info.facts.content_hash.clone(),
            capability_digest: capability_digest.to_owned(),
            actor_did: lock(&self.manager).governance().local_did().to_owned(),
            at: self.ports.now(),
        };
        let package_id = record.package_id.clone();
        self.ports.persist_consent(&record)?;
        lock(&self.manager).governance_mut().record_consent(record);
        // An install offer's "Allow in <verse>" is part of the same confirmation (bindings.md §5.5 step 3).
        if let Some(verse_id) = accept_in_verse.as_deref() {
            self.accept_in_verse(installation_id, verse_id, &package_id)?;
        }
        let staged = self.staged.remove(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        self.promote(staged, accept_in_verse, scope)
    }

    /// Record "Allow in <verse>" durably, then in the manager; the binding resolver starts what it now admits.
    fn accept_in_verse(&self, installation_id: &str, verse_id: &str, package_id: &str) -> Result<(), String> {
        if verse_id.is_empty() {
            return Err("invalid-input: acceptance needs a verse".into());
        }
        self.ports.persist_acceptance(installation_id, verse_id, package_id)?;
        lock(&self.manager).governance_mut().accept_in_verse(installation_id, verse_id);
        Ok(())
    }

    /// An update whose digest is already consented: consent on record must equal the staged digest (R19 row 9),
    /// and the staged build must come from the installed build's publisher and root (else it is a new install).
    fn activate_update(
        &mut self,
        installation_id: &str,
        capability_digest: &str,
        scope: &ActiveScope,
    ) -> Result<Vec<ExtensionEvent>, String> {
        let staged = self.staged.get(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        if staged.info.facts.capability_digest != capability_digest {
            return Err("stale-capability-digest: the staged update asks for different grants".into());
        }
        let installed = self
            .installed
            .get(installation_id)
            .filter(|_| staged.is_update)
            .ok_or_else(|| format!("consent-missing: {installation_id} is not an update of an installed build; review it first"))?;
        if staged.info.publisher_change(&installed.info).is_some() {
            return Err(format!(
                "consent-missing: the update of {installation_id} comes from another publisher; review and consent again"
            ));
        }
        {
            let manager = lock(&self.manager);
            judge(&manager, installation_id, scope, &staged.info.facts).map_err(|refusal| refusal.to_string())?;
        }
        let staged = self.staged.remove(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        self.promote(staged, None, scope)
    }

    /// Store the consented build and run it: update beside a running version, start otherwise.
    fn promote(&mut self, staged: StagedBuild, accept_in_verse: Option<String>, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        let StagedBuild { bytes, package, info, is_update, origin } = staged;
        // The install or update a CLI asked for is audited on its channel; the consent itself was a GUI click.
        let channel = if origin == ConsentOrigin::Cli { CHANNEL_CLI } else { self.audit_channel };
        let installation_id = info.facts.package_id.clone();
        // R19 already decided revocation (including the Owner's override), so the store takes no second opinion.
        self.store
            .install(&bytes, Some(&installation_id), &self.plugin_trust(), &|_| false)
            .map_err(|error| format!("package store: {error}"))?;
        let previous = self.installed.get(&installation_id).map(|build| (build.info.facts.content_hash.clone(), build.activated_once));
        let was_running = lock(&self.manager).governance().running_in(&scope.verse_id).contains(&installation_id);
        // An install offer only installs, consents and accepts: the binding resolver starts its binding instance
        // (keyed `(petal, package)`), so a local start here would race it for the same context key.
        let may_start = !scope.petal_id.is_empty() && accept_in_verse.is_none();
        let version = info.facts.version.clone();
        let content_hash = info.facts.content_hash.clone();
        let mut activated = previous.as_ref().is_some_and(|(_, once)| *once);
        let mut events = Vec::new();
        let mut start_failure: Option<String> = None;

        if is_update && was_running {
            match self.run_build(&package, &info.facts, false, scope, RunMode::Update) {
                Ok(()) => {
                    activated = true;
                    events.push(ExtensionEvent::UpdateStage {
                        installation_id: installation_id.clone(),
                        stage: UpdateStage::Active { version },
                    });
                }
                Err(RunError::RolledBack(reason)) => {
                    // The previous build still runs and stays installed; drop the new build's file.
                    self.drop_stored(&installation_id, &content_hash, previous.as_ref().map(|(hash, _)| hash.as_str()));
                    return Ok(vec![ExtensionEvent::UpdateStage {
                        installation_id,
                        stage: UpdateStage::RolledBack { reason },
                    }]);
                }
                Err(RunError::Failed(message)) => {
                    self.drop_stored(&installation_id, &content_hash, previous.as_ref().map(|(hash, _)| hash.as_str()));
                    return Err(message);
                }
            }
        } else if may_start {
            match self.run_build(&package, &info.facts, !activated, scope, RunMode::Start) {
                Ok(()) => activated = true,
                Err(RunError::RolledBack(message) | RunError::Failed(message)) => start_failure = Some(message),
            }
        }

        if let Some((old_hash, _)) = &previous {
            if !old_hash.eq_ignore_ascii_case(&content_hash) {
                if let Err(error) = self.store.remove(&installation_id, old_hash) {
                    tracing::warn!(%installation_id, %error, "previous extension build not removed");
                }
            }
        }
        let change = match &previous {
            None => Some(AuditAction::Install),
            Some((old_hash, _)) if !old_hash.eq_ignore_ascii_case(&content_hash) => Some(AuditAction::Update),
            Some(_) => None,
        };
        if let Some(action) = change {
            self.ports.audit_change_via(action, &installation_id, channel);
        }
        self.offers.remove(&installation_id);
        self.installed.insert(installation_id, InstalledBuild { info, activated_once: activated });
        events.push(self.rows_event(scope));
        // Installed and consented, but the start was refused: the row shows why and Enable can retry.
        if let Some(message) = start_failure {
            events.push(ExtensionEvent::Failed { message });
        }
        Ok(events)
    }

    fn drop_stored(&self, package_id: &str, new_hash: &str, keep: Option<&str>) {
        if keep.is_some_and(|hash| hash.eq_ignore_ascii_case(new_hash)) {
            return;
        }
        if let Err(error) = self.store.remove(package_id, new_hash) {
            tracing::warn!(%package_id, %error, "discarded extension build not removed");
        }
    }

    /// R19, stage the launch through the binary, then start or update on the worker's `TickHost`.
    fn run_build(
        &self,
        package: &PluginPackage,
        facts: &PackageFacts,
        first_activation: bool,
        scope: &ActiveScope,
        mode: RunMode,
    ) -> Result<(), RunError> {
        let build = BuildToRun { installation_id: &facts.package_id, package, facts, first_activation };
        let staged = self.ports.stage_launch(&build, scope).map_err(RunError::Failed)?;
        let check = PackageCheck::Verified(facts.clone());
        let mut manager = lock(&self.manager);
        let did = manager.governance().local_did().to_owned();
        let request = ActivationRequest {
            actor_did: &did,
            installation_id: &facts.package_id,
            verse_id: &scope.verse_id,
            petal_id: &scope.petal_id,
            package: &check,
        };
        let outcome = match mode {
            RunMode::Start => manager.start_instance(&request, staged),
            RunMode::Update => manager.update_instance(&request, staged),
        };
        outcome.map_err(|error| match error {
            LifecycleError::RolledBack(reason) => RunError::RolledBack(reason),
            other => RunError::Failed(other.to_string()),
        })
    }

    /// Lift only the kill-list refusal for the Owner's own verse; consent is still owed (fresh `ConsentNeeded`).
    fn kill_list_override(
        &mut self,
        installation_id: &str,
        verse_id: &str,
        package_id: &str,
        content_hash: &str,
        confirm: &str,
        scope: &ActiveScope,
    ) -> Result<Vec<ExtensionEvent>, String> {
        let staged = self.staged.get(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        if staged.info.facts.package_id != package_id || !staged.info.facts.content_hash.eq_ignore_ascii_case(content_hash) {
            return Err("the override names another build than the one staged".into());
        }
        if verse_id != scope.verse_id {
            return Err("open the verse the override is for first".into());
        }
        if confirm != package_id {
            return Err("confirmation-mismatch: type the package id exactly to override the kill list".into());
        }
        self.ports.record_kill_list_override(verse_id, package_id, content_hash, confirm)?;
        lock(&self.manager).governance_mut().add_kill_list_override(verse_id, content_hash);
        let mut request = self.consent_request(installation_id, &staged.info, staged.is_update, scope);
        request.origin = staged.origin.clone();
        Ok(vec![ExtensionEvent::ConsentNeeded { request, is_update: staged.is_update }])
    }

    /// The stored build of an installation, re-verified; its bytes must still be the build on record.
    fn stored_build(&self, installation_id: &str) -> Result<(Vec<u8>, PluginPackage, BuildInfo, bool), String> {
        let installed = self.installed.get(installation_id).ok_or_else(|| format!("{installation_id} is not installed"))?;
        let facts = &installed.info.facts;
        let bytes = self
            .store
            .get(&facts.package_id, &facts.content_hash)
            .map_err(|error| format!("package store: {error}"))?
            .ok_or_else(|| format!("the stored build of {installation_id} is missing"))?;
        let (package, info) = self.verify(&bytes, Some(&facts.package_id))?;
        if !info.facts.content_hash.eq_ignore_ascii_case(&facts.content_hash) {
            return Err(format!("the stored build of {installation_id} changed on disk"));
        }
        Ok((bytes, package, info, installed.activated_once))
    }

    /// Hold the installed build for a fresh review (consent missing, Owner override); nothing runs until consent.
    fn restage_installed(&mut self, installation_id: &str, scope: &ActiveScope, origin: ConsentOrigin) -> Result<Vec<ExtensionEvent>, String> {
        let (bytes, package, info, _) = self.stored_build(installation_id)?;
        let mut request = self.consent_request(installation_id, &info, false, scope);
        request.origin = origin.clone();
        self.staged.insert(installation_id.to_owned(), StagedBuild { bytes, package, info, is_update: false, origin });
        Ok(vec![ExtensionEvent::ConsentNeeded { request, is_update: false }])
    }

    /// Enable in the open petal; no consent for the digest re-stages the stored build and asks first.
    fn enable(&mut self, installation_id: &str, binding_id: &str, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        if binding_id != scope.petal_id {
            return Err(format!("{binding_id} is not the open petal"));
        }
        let (_, package, info, activated_once) = self.stored_build(installation_id)?;
        let consented = lock(&self.manager)
            .governance()
            .consent(installation_id)
            .is_some_and(|record| record.capability_digest == info.facts.capability_digest);
        if !consented {
            return self.restage_installed(installation_id, scope, ConsentOrigin::Direct);
        }
        self.run_build(&package, &info.facts, !activated_once, scope, RunMode::Start)
            .map_err(|error| match error {
                RunError::RolledBack(message) | RunError::Failed(message) => message,
            })?;
        if let Some(installed) = self.installed.get_mut(installation_id) {
            installed.activated_once = true;
        }
        self.audit(AuditAction::Enable, installation_id);
        Ok(vec![self.rows_event(scope)])
    }

    /// Remove consent and stop the instance after its in-flight writes (OD-E1: rows stay).
    fn revoke_consent(&mut self, installation_id: &str, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        let package_id = self
            .installed
            .get(installation_id)
            .map(|build| build.info.facts.package_id.clone())
            .ok_or_else(|| format!("{installation_id} is not installed"))?;
        self.ports.persist_consent_revoke(installation_id, &package_id)?;
        {
            let mut manager = lock(&self.manager);
            manager.governance_mut().revoke_consent(installation_id);
            manager.disable_instance(installation_id);
        }
        Ok(vec![self.rows_event(scope)])
    }

    fn uninstall(&mut self, installation_id: &str, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        let build = self.installed.remove(installation_id).ok_or_else(|| format!("{installation_id} is not installed"))?;
        self.staged.remove(installation_id);
        self.offers.remove(installation_id);
        let had_consent = {
            let mut manager = lock(&self.manager);
            manager.uninstall_instance(installation_id);
            manager.governance_mut().revoke_consent(installation_id).is_some()
        };
        if had_consent {
            if let Err(error) = self.ports.persist_consent_revoke(installation_id, &build.info.facts.package_id) {
                tracing::warn!(%installation_id, %error, "consent revoke not recorded on uninstall");
            }
        }
        // A never-consented build skips the revoke above, so the activator is told directly.
        self.ports.forget_installation(installation_id);
        if let Err(error) = self.store.remove(&build.info.facts.package_id, &build.info.facts.content_hash) {
            tracing::warn!(%installation_id, %error, "uninstalled extension build not removed");
        }
        self.audit(AuditAction::Uninstall, installation_id);
        Ok(vec![self.rows_event(scope)])
    }

    // ---- Bindings (bindings.md §5, §9) ------------------------------------------------------

    /// The Bindings view of the open petal: the host's plane, this worker's providers, offers only where they can run.
    fn fresh_bindings(&mut self, scope: &ActiveScope) -> Result<BindingsSnapshot, String> {
        if scope.petal_id.is_empty() {
            return Err("Open a petal to see its bindings".into());
        }
        let mut snapshot = self.ports.bindings_snapshot(scope)?;
        self.note_offer_packages(&snapshot, &scope.verse_id);
        snapshot.providers = self.providers();
        self.add_unbound_points(&mut snapshot);
        self.filter_offers(&mut snapshot);
        Ok(snapshot)
    }

    fn bindings_event(&mut self, scope: &ActiveScope) -> Result<ExtensionEvent, String> {
        self.fresh_bindings(scope).map(ExtensionEvent::Bindings)
    }

    /// Every installed build's `provides` entries (the edit form's provider picker).
    fn providers(&self) -> Vec<ProviderOption> {
        let mut providers: Vec<ProviderOption> = Vec::new();
        for build in self.installed.values() {
            let info = &build.info;
            for provided in info.facts.manifest.iter().flat_map(|manifest| manifest.provides.iter()) {
                providers.push(ProviderOption {
                    hook: provided.hook.clone(),
                    package_id: info.facts.package_id.clone(),
                    name: info.name.clone(),
                    version: info.facts.version.clone(),
                    config_schema: provided.config_schema.clone(),
                });
            }
        }
        providers.sort_by(|a, b| (&a.hook, &a.package_id).cmp(&(&b.hook, &b.package_id)));
        providers
    }

    /// Unbound points a binding can be written for: every served keyless hook, and each exact key a provider declares.
    fn add_unbound_points(&self, snapshot: &mut BindingsSnapshot) {
        let mut wanted: Vec<(String, String)> = served_hook_ids()
            .filter(|hook| hook_spec(hook).is_some_and(|spec| spec.key_kind == HookKeyKind::None))
            .map(|hook| (hook.to_owned(), String::new()))
            .collect();
        for build in self.installed.values() {
            for provided in build.info.facts.manifest.iter().flat_map(|manifest| manifest.provides.iter()) {
                let Some(spec) = hook_spec(&provided.hook) else { continue };
                if spec.key_kind == HookKeyKind::None {
                    continue;
                }
                for key in provided.keys.iter().flatten().filter(|key| !key.ends_with('*') && key_allowed(spec, key)) {
                    wanted.push((provided.hook.clone(), key.clone()));
                }
            }
        }
        for (hook, key) in wanted {
            if !snapshot.points.iter().any(|row| row.hook == hook && row.key == key) {
                snapshot.points.push(BindingPointRow {
                    hook,
                    key,
                    scope_label: String::new(),
                    target: None,
                    mode: BindingMode::Inherit,
                    package_id: None,
                    version_req: None,
                    config: Value::Object(serde_json::Map::new()),
                    runs_on: None,
                    resolution: None,
                });
            }
        }
        snapshot.points.sort_by(|a, b| (&a.hook, &a.key).cmp(&(&b.hook, &b.key)));
    }

    /// Install and Update stay only where a configured source lists a fitting build (bindings.md §5.2): no dead control.
    fn filter_offers(&self, snapshot: &mut BindingsSnapshot) {
        let mut available: HashMap<(String, Option<String>, bool), bool> = HashMap::new();
        for row in &mut snapshot.points {
            let version_req = row.version_req.clone();
            let Some(BindingResolution::Unresolved { package_id, offer, .. }) = &mut row.resolution else { continue };
            let newer = match offer {
                Some(BindingOffer::Install) => false,
                Some(BindingOffer::Update) => true,
                _ => continue,
            };
            let package_id = package_id.clone();
            let fits = *available
                .entry((package_id.clone(), version_req.clone(), newer))
                .or_insert_with(|| self.find_build(&package_id, version_req.as_deref(), newer).is_some());
            if !fits {
                *offer = None;
            }
        }
    }

    /// Remember the packages the open petal's published resolutions could be offered; a new one refreshes at once.
    fn note_offer_packages(&mut self, snapshot: &BindingsSnapshot, verse_id: &str) {
        self.offer_packages = offer_packages_of(snapshot);
        if !self.offer_packages.is_subset(&self.offers_covered) {
            self.refresh_registry_offers(verse_id);
        }
    }

    /// Re-read the configured sources for the offer packages; a change re-resolves the verse.
    fn refresh_registry_offers(&mut self, verse_id: &str) {
        let changed = self.registry_offers.refresh(&self.sources, &self.offer_packages, &self.trust.is_killed);
        self.offers_covered = self.offer_packages.clone();
        self.offers_next_refresh = Instant::now() + OFFER_REFRESH_PERIOD;
        if changed {
            self.ports.registry_offers_changed(verse_id);
        }
    }

    /// The idle refresh: re-read the open petal's bindings (when one is open), then the sources.
    fn periodic_offer_refresh(&mut self) {
        let scope = self.gui_scope.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if !scope.petal_id.is_empty() {
            match self.ports.bindings_snapshot(&scope) {
                Ok(snapshot) => self.offer_packages = offer_packages_of(&snapshot),
                Err(reason) => tracing::debug!(%reason, "bindings unreadable; registry offers keep the last packages"),
            }
        }
        self.refresh_registry_offers(&scope.verse_id);
    }

    /// The highest listed build of `package_id` fitting `version_req` (and newer than the installed one for an update).
    fn find_build(&self, package_id: &str, version_req: Option<&str>, newer: bool) -> Option<(String, RegistryEntry)> {
        let installed = self.installed.get(package_id).map(|build| build.info.facts.version.clone());
        let mut best: Option<(String, RegistryEntry)> = None;
        for source in self.sources.browse_sources() {
            let Some(registry) = self.sources.get(&source.id) else { continue };
            let Ok(listing) = registry.list(Some(package_id)) else { continue };
            for entry in listing.entries.into_iter().filter(|entry| entry.package_id == package_id) {
                let fits = version_req.map_or(true, |requirement| version_matches(requirement, &entry.version));
                let newer_ok = !newer || installed.as_deref().map_or(true, |current| is_newer(&entry.version, current));
                let better = best.as_ref().map_or(true, |(_, known)| is_newer(&entry.version, &known.version));
                if fits && newer_ok && better {
                    best = Some((source.id.clone(), entry));
                }
            }
        }
        best
    }

    /// Write as the local DID through the DB plane (Manager+ at the scope and the verse policy judged there).
    fn write_binding(&mut self, write: BindingWrite, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        let BindingWrite { target, hook, key, mode, package_id, version_req, config, runs_on } = write;
        if target.verse_id != scope.verse_id {
            return Err("open the verse the binding is in first".into());
        }
        if target.petal_id.is_some() && target.fractal_id.is_none() {
            return Err("invalid-input: a petal scope needs its fractal".into());
        }
        let spec = hook_spec(&hook).ok_or_else(|| format!("unknown-hook: {hook}"))?;
        if !key_allowed(spec, &key) {
            return Err(format!("invalid-input: '{key}' is not a key of {hook}"));
        }
        let author_did = lock(&self.manager).governance().local_did().to_owned();
        let template = BindingRecord {
            schema_version: BINDING_SCHEMA_VERSION,
            scope: fe_database::build_scope(&target.verse_id, target.fractal_id.as_deref(), target.petal_id.as_deref()),
            hook,
            key,
            mode,
            package_id,
            version_req,
            config,
            runs_on,
            author_did,
            // Stamped with the write's own HLC by the DB plane.
            written_at: String::new(),
        };
        self.ports.write_binding(template)?;
        Ok(vec![self.bindings_event(scope)?])
    }

    /// Run the one offer the Bindings view showed; it must still be this host's offer on a fresh plane.
    fn take_binding_offer(
        &mut self,
        offer: BindingOffer,
        verse_id: &str,
        hook: &str,
        key: &str,
        package_id: &str,
        scope: &ActiveScope,
    ) -> Result<Vec<ExtensionEvent>, String> {
        if verse_id != scope.verse_id {
            return Err("open the verse the binding is in first".into());
        }
        if offer == BindingOffer::EditBinding {
            return Err("Edit binding opens the form in the Bindings view".into());
        }
        let snapshot = self.fresh_bindings(scope)?;
        let row = snapshot
            .points
            .iter()
            .find(|row| row.hook == hook && row.key == key)
            .ok_or("this binding is no longer at the open petal; refresh the bindings")?;
        let shown = matches!(
            &row.resolution,
            Some(BindingResolution::Unresolved { package_id: bound, offer: Some(current), .. })
                if bound == package_id && *current == offer
        );
        if !shown {
            return Err("this offer is no longer shown for the binding; refresh the bindings".into());
        }
        // The consent dialog names the verse and the scope; approval also records "Allow in <verse>" (§5.5).
        let origin = ConsentOrigin::InstallOffer {
            verse_id: verse_id.to_owned(),
            verse_name: scope.verse_name.clone(),
            binding_scope: row.scope_label.clone(),
        };
        let version_req = row.version_req.clone();
        let local_is_owner = snapshot.local_is_owner;
        match offer {
            BindingOffer::Install | BindingOffer::Update => {
                let newer = offer == BindingOffer::Update;
                let (source_id, entry) = self
                    .find_build(package_id, version_req.as_deref(), newer)
                    .ok_or("no configured source offers a build that fits this binding")?;
                self.fetch_and_stage_with(&source_id, &entry, scope, origin)
            }
            BindingOffer::Override => {
                if !local_is_owner {
                    return Err("role-denied: only the verse Owner, on the Owner's own host, may override the kill list".into());
                }
                self.restage_installed(package_id, scope, origin)
            }
            BindingOffer::ReviewPermissions => self.restage_installed(package_id, scope, origin),
            BindingOffer::AllowInVerse => {
                let (_, _, info, _) = self.stored_build(package_id)?;
                let consented = lock(&self.manager)
                    .governance()
                    .consent(package_id)
                    .is_some_and(|record| record.capability_digest == info.facts.capability_digest);
                if !consented {
                    return self.restage_installed(package_id, scope, origin);
                }
                self.accept_in_verse(package_id, verse_id, package_id)?;
                Ok(vec![self.rows_event(scope), self.bindings_event(scope)?])
            }
            BindingOffer::Enable => {
                if !self.installed.contains_key(package_id) {
                    return Err(format!("{package_id} is not installed"));
                }
                lock(&self.manager).enable_installation(package_id);
                self.audit(AuditAction::Enable, package_id);
                Ok(vec![self.rows_event(scope), self.bindings_event(scope)?])
            }
            BindingOffer::EditBinding => Err("Edit binding opens the form in the Bindings view".into()),
        }
    }

    // ---- Rows -------------------------------------------------------------------------------

    fn rows_event(&self, scope: &ActiveScope) -> ExtensionEvent {
        ExtensionEvent::Rows(self.rows(scope, true))
    }

    /// The installed list in `scope`; `details` adds counters and audit lines (the panel's details view only).
    fn rows(&self, scope: &ActiveScope, details: bool) -> Vec<InstalledRow> {
        let manager = lock(&self.manager);
        let running = manager.governance().running_in(&scope.verse_id);
        let mut rows: Vec<InstalledRow> = self
            .installed
            .iter()
            .map(|(installation_id, build)| {
                let info = &build.info;
                let enabled = running.contains(installation_id);
                let status = if enabled {
                    RowStatus::Enabled
                } else {
                    match judge(&manager, installation_id, scope, &info.facts) {
                        Ok(()) => RowStatus::Disabled,
                        Err(refusal) if refusal.precondition == Precondition::ConsentMissing => RowStatus::NeedsConsent,
                        Err(refusal) => RowStatus::Blocked(refusal.to_string()),
                    }
                };
                let consent = manager
                    .governance()
                    .consent(installation_id)
                    .filter(|record| record.capability_digest == info.facts.capability_digest)
                    .map(|record| ConsentSummary {
                        consented_at: record.at.clone(),
                        consented_by: record.actor_did.clone(),
                        capability_digest: record.capability_digest.clone(),
                        sections: info.sections.clone(),
                    });
                let bindings = if scope.petal_id.is_empty() {
                    Vec::new()
                } else {
                    vec![BindingRow {
                        binding_id: scope.petal_id.clone(),
                        label: format!("{} in {}", info.name, scope.verse_name),
                        enabled,
                    }]
                };
                InstalledRow {
                    installation_id: installation_id.clone(),
                    package_id: info.facts.package_id.clone(),
                    name: info.name.clone(),
                    version: info.facts.version.clone(),
                    publisher: short_did(&info.publisher_did),
                    signer: info.signer.clone(),
                    trust: if info.trusted { TrustState::Trusted } else { TrustState::Unsigned },
                    status,
                    update_available: self.offers.get(installation_id).map(|(_, entry)| entry.version.clone()),
                    description: info.description.clone(),
                    bindings,
                    counters: if details { self.ports.counters(installation_id) } else { Vec::new() },
                    audit: if details { self.ports.audit(installation_id) } else { Vec::new() },
                    consent,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.package_id.cmp(&b.package_id));
        rows
    }

    // ---- Admin channel (admin-channel.md §4-§7) -----------------------------------------------

    /// Serve one CLI request in the GUI's current scope; nothing here can record consent or an acceptance.
    fn handle_admin(&mut self, job: AdminJob) -> Vec<ExtensionEvent> {
        let AdminJob { request, replies } = job;
        let scope = self.gui_scope.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let action = verb_action(&request.verb);
        let target = match &request.verb {
            AdminVerb::InstallFile { path } => path.clone(),
            AdminVerb::InstallRegistry { package_id, .. } => package_id.clone(),
            verb => verb.installation_id().unwrap_or("*").to_owned(),
        };
        self.audit_channel = CHANNEL_CLI;
        let mut events = Vec::new();
        let outcome = self.admin(request.verb, &scope, &mut events);
        self.audit_channel = CHANNEL_GUI;
        let reply = match outcome {
            AdminOutcome::Done(result) => AdminReply::ok(&request.id, result),
            AdminOutcome::Pending(consent) => {
                let installation_id = consent.installation_id.clone();
                self.pending_admin.insert(
                    installation_id,
                    PendingAdmin { replies: replies.clone(), action: action.unwrap_or(AuditAction::Install) },
                );
                AdminReply::pending_consent(&request.id, consent)
            }
            AdminOutcome::Refused(refusal) => {
                if let Some(action) = action {
                    self.ports.audit_admin_refusal(action, &target, refusal.code.as_str(), &refusal.message);
                }
                AdminReply::refused(&request.id, refusal.code, refusal.message)
            }
        };
        // A CLI that already left is no error: the job and any dialog belong to the app now.
        let _ = replies.send(reply);
        if action.is_some() {
            events.push(self.rows_event(&scope));
        }
        events
    }

    fn admin(&mut self, verb: AdminVerb, scope: &ActiveScope, events: &mut Vec<ExtensionEvent>) -> AdminOutcome {
        let result = match verb {
            AdminVerb::List {} => Ok(self.admin_result(scope, None, None)),
            AdminVerb::Status { installation_id } => {
                if self.installed.contains_key(&installation_id) {
                    Ok(self.admin_result(scope, Some(installation_id.as_str()), None))
                } else {
                    Err(AdminRefusal::new(RefusalCode::NotFound, format!("{installation_id} is not installed")))
                }
            }
            AdminVerb::InstallFile { path } => {
                return match read_package_file(&path) {
                    Ok(bytes) => self.admin_stage(bytes, None, scope, events),
                    Err(message) => AdminOutcome::Refused(AdminRefusal::new(RefusalCode::InvalidInput, message)),
                };
            }
            AdminVerb::InstallRegistry { source_id, package_id, version } => {
                return self.admin_install_registry(&source_id, &package_id, version.as_deref(), scope, events);
            }
            AdminVerb::Enable { installation_id, binding_id } => self.admin_enable(&installation_id, binding_id, scope),
            AdminVerb::Disable { installation_id, .. } => {
                // Every binding row of an installation is its one local instance, so either form stops the installation.
                if self.installed.contains_key(&installation_id) {
                    lock(&self.manager).disable_instance(&installation_id);
                    self.audit(AuditAction::Disable, &installation_id);
                    Ok(self.admin_result(scope, Some(installation_id.as_str()), None))
                } else {
                    Err(AdminRefusal::new(RefusalCode::NotFound, format!("{installation_id} is not installed")))
                }
            }
            AdminVerb::Update { installation_id } => return self.admin_update(&installation_id, scope, events),
            AdminVerb::Unbind { installation_id, verse_id } => self.admin_unbind(&installation_id, &verse_id, scope),
            AdminVerb::Uninstall { installation_id } => {
                if self.installed.contains_key(&installation_id) {
                    self.uninstall(&installation_id, scope).map_err(refusal_of).map(|_| {
                        self.settle_removed(&installation_id);
                        AdminResult { installations: Vec::new(), message: Some(format!("Uninstalled {installation_id}")) }
                    })
                } else {
                    Err(AdminRefusal::new(RefusalCode::NotFound, format!("{installation_id} is not installed")))
                }
            }
            AdminVerb::DisableAll {} => match self.set_host_switch(true, CHANNEL_CLI) {
                Ok(stopped) => Ok(self.admin_result(scope, None, Some(format!("Disabled all extensions ({stopped} stopped)")))),
                Err(message) => Err(AdminRefusal::new(RefusalCode::HostOff, message)),
            },
            AdminVerb::EnableAll {} => match self.set_host_switch(false, CHANNEL_CLI) {
                Ok(_) => {
                    let message = "Cleared Disable all; each installation starts again when enabled".to_owned();
                    Ok(self.admin_result(scope, None, Some(message)))
                }
                Err(message) => Err(AdminRefusal::new(RefusalCode::HostOff, message)),
            },
        };
        match result {
            Ok(result) => AdminOutcome::Done(result),
            Err(refusal) => AdminOutcome::Refused(refusal),
        }
    }

    /// Disable all (`true`) or Enable all from the panel or the CLI; returns how many instances stopped.
    /// `FE_EXT_DISABLE_ALL=1` at startup keeps the host off, so Enable all is refused while it is set.
    fn set_host_switch(&mut self, disabled: bool, channel: &str) -> Result<usize, String> {
        if !disabled && std::env::var(EXT_DISABLE_ALL_ENV).is_ok_and(|value| value == "1") {
            return Err(format!("{EXT_DISABLE_ALL_ENV}=1 was set at startup and still wins"));
        }
        let stopped = if disabled {
            lock(&self.manager).disable_all().len()
        } else {
            lock(&self.manager).enable_all();
            0
        };
        self.ports.host_switch_changed(disabled, channel);
        Ok(stopped)
    }

    /// Every installation (or one) as the CLI shows it.
    fn admin_result(&self, scope: &ActiveScope, only: Option<&str>, message: Option<String>) -> AdminResult {
        let installations = self
            .rows(scope, false)
            .into_iter()
            .filter(|row| only.is_none_or(|id| row.installation_id == id))
            .map(summary_of)
            .collect();
        AdminResult { installations, message }
    }

    /// Resolve the build a CLI names: the catalogue's pinned detail, else the source's listing (kill list at offer).
    fn resolve_entry(&self, source_id: &str, package_id: &str, version: Option<&str>) -> Result<RegistryEntry, AdminRefusal> {
        let not_found = |what: String| AdminRefusal::new(RefusalCode::NotFound, what);
        if let Some(catalog) = self.sources.catalog(source_id) {
            let verified = catalog.detail(package_id).map_err(registry_refusal)?;
            if verified.revoked && version.is_none() {
                return Err(AdminRefusal::new(RefusalCode::Revoked, format!("{package_id}: the listed build is on a kill list")));
            }
            let wanted = version.unwrap_or(&verified.listed.latest_version).to_owned();
            let found = verified
                .detail
                .versions
                .iter()
                .find(|candidate| candidate.version == wanted && !candidate.yanked)
                .ok_or_else(|| not_found(format!("{package_id} {wanted} is not offered by {source_id}")))?;
            return Ok(RegistryEntry {
                version: found.version.clone(),
                content_hash: found.content_hash.clone(),
                url: found.download_url.clone(),
                size: found.size_bytes,
                ..verified.listed.to_registry_entry()
            });
        }
        let source = self.sources.get(source_id).ok_or_else(|| not_found(format!("unknown extension source {source_id}")))?;
        let listing = source.list(Some(package_id)).map_err(registry_refusal)?;
        let listed = listing.entries.iter().any(|entry| entry.package_id == package_id);
        if let Some(refused) = listing.refused.iter().find(|refused| refused.what == package_id).filter(|_| !listed) {
            let code = if refused.reason.contains("revoked") { RefusalCode::Revoked } else { RefusalCode::NotAvailable };
            return Err(AdminRefusal::new(code, format!("{package_id}: {}", refused.reason)));
        }
        let mut best: Option<RegistryEntry> = None;
        for entry in listing.entries.into_iter().filter(|entry| entry.package_id == package_id) {
            let fits = version.is_none_or(|wanted| entry.version == wanted);
            if fits && best.as_ref().is_none_or(|known| is_newer(&entry.version, &known.version)) {
                best = Some(entry);
            }
        }
        best.ok_or_else(|| not_found(format!("{package_id} is not offered by {source_id}")))
    }

    fn admin_install_registry(
        &mut self,
        source_id: &str,
        package_id: &str,
        version: Option<&str>,
        scope: &ActiveScope,
        events: &mut Vec<ExtensionEvent>,
    ) -> AdminOutcome {
        let entry = match self.resolve_entry(source_id, package_id, version) {
            Ok(entry) => entry,
            Err(refusal) => return AdminOutcome::Refused(refusal),
        };
        let fetched = match self.fetch_entry(source_id, &entry) {
            Ok(fetched) => fetched,
            Err(error) => return AdminOutcome::Refused(registry_refusal(error)),
        };
        if fetched.kind() == WEB_BUNDLE_KIND {
            return match self.install_web_bundle(&fetched) {
                Ok(message) => AdminOutcome::Done(AdminResult { installations: Vec::new(), message: Some(message) }),
                Err(message) => AdminOutcome::Refused(refusal_of(message)),
            };
        }
        self.admin_stage(fetched.bytes, Some(package_id), scope, events)
    }

    /// Stage for consent in the GUI (never here): a blocked build is refused at once, a review already open is not replaced,
    /// and an update whose digest and publisher are already consented activates with no prompt.
    fn admin_stage(
        &mut self,
        bytes: Vec<u8>,
        expected_id: Option<&str>,
        scope: &ActiveScope,
        events: &mut Vec<ExtensionEvent>,
    ) -> AdminOutcome {
        let (package, info) = match self.verify(&bytes, expected_id) {
            Ok(verified) => verified,
            Err(message) => return AdminOutcome::Refused(refusal_of(message)),
        };
        let installation_id = package.package_id.clone();
        if self.staged.contains_key(&installation_id) || self.pending_admin.contains_key(&installation_id) {
            return AdminOutcome::Refused(AdminRefusal::new(
                RefusalCode::NotAvailable,
                format!("a review of {installation_id} is already open in the app; finish it there first"),
            ));
        }
        let staged_events = match self.stage_verified(bytes, package, info, scope, ConsentOrigin::Cli) {
            Ok(staged_events) => staged_events,
            Err(message) => return AdminOutcome::Refused(refusal_of(message)),
        };
        let Some((request, is_update)) = staged_events.iter().find_map(|event| match event {
            ExtensionEvent::ConsentNeeded { request, is_update } => Some((request.clone(), *is_update)),
            _ => None,
        }) else {
            return AdminOutcome::Refused(AdminRefusal::new(RefusalCode::Internal, "staging produced no review"));
        };
        if let Some(reason) = &request.blocked {
            // The dialog could only decline: refuse here and drop the staged build.
            self.staged.remove(&installation_id);
            return AdminOutcome::Refused(refusal_of(reason.clone()));
        }
        let consented = request.consented_digest.as_deref() == Some(request.capability_digest.as_str());
        if is_update && consented && request.kill_list.is_none() {
            return match self.activate_update(&installation_id, &request.capability_digest, scope) {
                Ok(update_events) => {
                    let outcome = self.promoted_outcome(&installation_id, scope, &update_events);
                    events.extend(update_events);
                    outcome
                }
                Err(message) => {
                    self.staged.remove(&installation_id);
                    AdminOutcome::Refused(refusal_of(message))
                }
            };
        }
        events.extend(staged_events);
        AdminOutcome::Pending(PendingConsent {
            installation_id,
            package_id: request.package_id,
            name: request.name,
            version: request.version,
            capability_digest: request.capability_digest,
            is_update,
        })
    }

    /// The final answer once a staged build was promoted: a rolled-back update or a refused start is said plainly.
    fn promoted_outcome(&self, installation_id: &str, scope: &ActiveScope, events: &[ExtensionEvent]) -> AdminOutcome {
        let mut message = None;
        for event in events {
            match event {
                ExtensionEvent::UpdateStage { stage: UpdateStage::RolledBack { reason }, .. } => {
                    return AdminOutcome::Refused(AdminRefusal::new(
                        RefusalCode::NotAvailable,
                        format!("the update failed to start, the previous version kept running: {reason}"),
                    ));
                }
                ExtensionEvent::Failed { message: failure } => message = Some(format!("Installed; not started: {failure}")),
                _ => {}
            }
        }
        AdminOutcome::Done(self.admin_result(scope, Some(installation_id), message))
    }

    fn admin_update(&mut self, installation_id: &str, scope: &ActiveScope, events: &mut Vec<ExtensionEvent>) -> AdminOutcome {
        let Some(installed) = self.installed.get(installation_id) else {
            return AdminOutcome::Refused(AdminRefusal::new(RefusalCode::NotFound, format!("{installation_id} is not installed")));
        };
        let package_id = installed.info.facts.package_id.clone();
        let offer = self.offers.get(installation_id).cloned().or_else(|| self.find_build(&package_id, None, true));
        let Some((source_id, entry)) = offer else {
            return AdminOutcome::Refused(AdminRefusal::new(
                RefusalCode::NotFound,
                format!("no configured source offers a newer version of {installation_id}"),
            ));
        };
        let fetched = match self.fetch_entry(&source_id, &entry) {
            Ok(fetched) => fetched,
            Err(error) => return AdminOutcome::Refused(registry_refusal(error)),
        };
        self.admin_stage(fetched.bytes, Some(&package_id), scope, events)
    }

    /// Enable never raises the dialog: no consent in force answers `consent-missing`.
    fn admin_enable(&mut self, installation_id: &str, binding_id: Option<String>, scope: &ActiveScope) -> Result<AdminResult, AdminRefusal> {
        let Some(installed) = self.installed.get(installation_id) else {
            return Err(AdminRefusal::new(RefusalCode::NotFound, format!("{installation_id} is not installed")));
        };
        let digest = installed.info.facts.capability_digest.clone();
        let consented = lock(&self.manager)
            .governance()
            .consent(installation_id)
            .is_some_and(|record| record.capability_digest == digest);
        if !consented {
            return Err(AdminRefusal::new(
                RefusalCode::ConsentMissing,
                format!("{installation_id} has no consent in force; review it in the app"),
            ));
        }
        let binding_id = binding_id.unwrap_or_else(|| scope.petal_id.clone());
        if binding_id.is_empty() {
            return Err(AdminRefusal::new(RefusalCode::InvalidInput, "open a petal in the app, or name its binding_id"));
        }
        self.enable(installation_id, &binding_id, scope).map_err(refusal_of)?;
        Ok(self.admin_result(scope, Some(installation_id), None))
    }

    /// Withdraw "Allow in <verse>": durable `unbind` first, then the manager stops the binding instances there.
    fn admin_unbind(&mut self, installation_id: &str, verse_id: &str, scope: &ActiveScope) -> Result<AdminResult, AdminRefusal> {
        let Some(installed) = self.installed.get(installation_id) else {
            return Err(AdminRefusal::new(RefusalCode::NotFound, format!("{installation_id} is not installed")));
        };
        let package_id = installed.info.facts.package_id.clone();
        let accepted = lock(&self.manager)
            .governance()
            .acceptances()
            .contains(&(installation_id.to_owned(), verse_id.to_owned()));
        if !accepted {
            let message = format!("{installation_id} was not allowed in {verse_id}; nothing changed");
            return Ok(self.admin_result(scope, Some(installation_id), Some(message)));
        }
        self.ports.persist_withdrawal(installation_id, verse_id, &package_id).map_err(refusal_of)?;
        let stopped = lock(&self.manager).withdraw_acceptance(installation_id, verse_id);
        let message = format!("Withdrew {installation_id} from {verse_id} ({} stopped)", stopped.len());
        Ok(self.admin_result(scope, Some(installation_id), Some(message)))
    }

    /// A panel op answered (or ended) the dialog a CLI request is waiting on.
    fn settle_admin(&mut self, installation_id: &str, settle: Settle, result: &Result<Vec<ExtensionEvent>, String>, scope: &ActiveScope) {
        if !self.pending_admin.contains_key(installation_id) {
            return;
        }
        let outcome = match (settle, result) {
            (Settle::Promote, Ok(events)) => self.promoted_outcome(installation_id, scope, events),
            (Settle::Promote | Settle::Override, Err(message)) => AdminOutcome::Refused(refusal_of(message.clone())),
            (Settle::Override, Ok(_)) => return,
            (Settle::Declined, _) => AdminOutcome::Refused(AdminRefusal::new(RefusalCode::ConsentDeclined, "Declined in the app")),
            (Settle::Removed, _) => {
                AdminOutcome::Refused(AdminRefusal::new(RefusalCode::NotFound, "the installation was removed in the app"))
            }
        };
        let Some(pending) = self.pending_admin.remove(installation_id) else { return };
        let reply = match outcome {
            AdminOutcome::Done(result) => AdminReply::ok("", result),
            AdminOutcome::Refused(refusal) => {
                self.ports.audit_admin_refusal(pending.action, installation_id, refusal.code.as_str(), &refusal.message);
                AdminReply::refused("", refusal.code, refusal.message)
            }
            AdminOutcome::Pending(_) => return,
        };
        // The listener stamps the request id; a CLI that left is no error.
        let _ = pending.replies.send(reply);
    }

    /// A CLI uninstall removed a build another CLI request was reviewing.
    fn settle_removed(&mut self, installation_id: &str) {
        if let Some(pending) = self.pending_admin.remove(installation_id) {
            let _ = pending.replies.send(AdminReply::refused("", RefusalCode::NotFound, "the installation was removed"));
        }
    }
}

// ---- Bevy wiring ----------------------------------------------------------------------------

/// One batch of ops with the scope they were queued in.
pub struct OpsJob {
    pub ops: Vec<ExtensionOp>,
    pub scope: ActiveScope,
}

/// Channels between the Bevy world and the ops worker thread.
#[derive(Resource)]
pub struct ExtensionOpsBridge {
    jobs: Sender<WorkerInput>,
    events: Receiver<ExtensionEvent>,
    /// The navigation cursor admin jobs run in (written here, read by the worker).
    scope: Arc<Mutex<ActiveScope>>,
}

type WorkerInit = Box<dyn FnOnce() -> OpsWorker + Send>;

/// Spawns the ops worker (built on its own thread) and pumps the panel's ops to it and its events back.
pub struct ExtensionOpsPlugin {
    init: Mutex<Option<WorkerInit>>,
    jobs: Sender<WorkerInput>,
    inbox: Mutex<Option<Receiver<WorkerInput>>>,
}

impl ExtensionOpsPlugin {
    /// `init` runs on the worker thread: build the sources, hydrate, return the worker.
    pub fn new(init: impl FnOnce() -> OpsWorker + Send + 'static) -> Self {
        let (jobs, inbox) = unbounded::<WorkerInput>();
        Self { init: Mutex::new(Some(Box::new(init))), jobs, inbox: Mutex::new(Some(inbox)) }
    }

    /// The admin channel's queue into this worker; take it before `add_plugins` to spawn the listener.
    pub fn admin_jobs(&self) -> AdminJobSender {
        AdminJobSender(self.jobs.clone())
    }
}

impl Plugin for ExtensionOpsPlugin {
    fn build(&self, app: &mut App) {
        let Some(init) = self.init.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() else {
            return;
        };
        let Some(inbox) = self.inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() else {
            return;
        };
        let (event_tx, event_rx) = unbounded::<ExtensionEvent>();
        let scope = Arc::new(Mutex::new(ActiveScope::default()));
        let worker_scope = scope.clone();
        let spawned = std::thread::Builder::new().name("extension-ops".into()).spawn(move || {
            let mut worker = init();
            worker.gui_scope = worker_scope;
            worker.hydrate();
            worker.run(inbox, event_tx);
        });
        if let Err(error) = spawned {
            warn!(%error, "extension ops worker did not start; the Extensions panel stays inert");
        }
        app.insert_resource(ExtensionOpsBridge { jobs: self.jobs.clone(), events: event_rx, scope })
            .insert_resource(self.admin_jobs())
            .add_systems(Update, pump_extension_ops);
    }
}

/// The navigation cursor as the worker judges it.
fn current_scope(navigation: Option<&NavigationManager>, verses: Option<&VerseManager>) -> ActiveScope {
    navigation
        .map(|nav| {
            let petal_id = nav.active_petal_id.clone().unwrap_or_default();
            let petal_name = verses
                .and_then(|verses| verses.find_petal(&petal_id))
                .map(|petal| petal.name.clone())
                .unwrap_or_default();
            ActiveScope {
                verse_id: nav.active_verse_id.clone().unwrap_or_default(),
                verse_name: nav.active_verse_name.clone(),
                petal_id,
                fractal_name: nav.active_fractal_name.clone(),
                petal_name,
            }
        })
        .unwrap_or_default()
}

/// Hand queued ops to the worker and apply its answers; a dead worker answers `Failed` so the panel never hangs.
/// The cursor admin jobs run in is refreshed only when navigation changed or ops were queued (no per-frame allocation).
fn pump_extension_ops(
    mut panel: ResMut<ExtensionsPanelState>,
    mut consent: ResMut<ConsentState>,
    bridge: Res<ExtensionOpsBridge>,
    navigation: Option<Res<NavigationManager>>,
    verses: Option<Res<VerseManager>>,
    mut seeded: Local<bool>,
) {
    let ops = panel.take_ops();
    let moved = navigation.as_ref().is_some_and(|nav| nav.is_changed());
    if !ops.is_empty() || moved || !*seeded {
        *seeded = true;
        let scope = current_scope(navigation.as_deref(), verses.as_deref());
        *bridge.scope.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = scope.clone();
        if !ops.is_empty() {
            if let Err(error) = bridge.jobs.send(WorkerInput::Panel(OpsJob { ops, scope })) {
                warn!(%error, "extension ops worker is gone");
                panel.apply_event(ExtensionEvent::Failed { message: "the extension host is not running".into() }, &mut consent);
            }
        }
    }
    while let Ok(event) = bridge.events.try_recv() {
        panel.apply_event(event, &mut consent);
    }
}
