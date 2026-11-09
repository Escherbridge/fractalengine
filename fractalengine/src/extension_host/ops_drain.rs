//! Drains the Extensions panel's queued `ExtensionOp`s on a worker thread against the `PluginManager` and
//! answers with `ExtensionEvent`s (docs/extensions/extensions-ui.md §Operations seam, §Consent cannot be bypassed).

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex, MutexGuard};

use bevy::prelude::*;
use crossbeam::channel::{unbounded, Receiver, Sender};
use fe_hexon::package::{verify_plugin_package, PluginPackage, PluginPackageError, PluginPackageStore, PluginTrust};
use fe_hexon::registry_source::{RegistryEntry, SourceTrust, PACKAGE_MAX_BYTES};
use fe_plugin::lifecycle::{
    ActivationRefusal, ActivationRequest, ConsentRecord, LifecycleError, PackageCheck, PackageFacts, PluginManager,
    Precondition, StagedInstance, TrustTier,
};
use fe_sdk::manifest_v2::{parse_and_validate, ValidatedManifest};
use fe_ui::extensions::browse::BrowseEvent;
use fe_ui::extensions::consent::{ConsentRequest, ConsentSection, ConsentState, KillListOverrideOffer};
use fe_ui::extensions::details::UpdateStage;
use fe_ui::extensions::panel::{
    AuditLine, BindingRow, ConsentSummary, ExtensionEvent, ExtensionOp, ExtensionsPanelState, InstalledRow, RowStatus,
    TrustState,
};
use fe_ui::navigation_manager::NavigationManager;
use serde_json::Value;

use super::sources::SourceSet;

/// The verse and petal an op runs in: the navigation cursor when the op was queued.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveScope {
    pub verse_id: String,
    pub verse_name: String,
    pub petal_id: String,
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
}

/// What the panel and consent dialog show about one verified build.
struct BuildInfo {
    facts: PackageFacts,
    name: String,
    description: String,
    publisher_did: String,
    signer: String,
    trusted: bool,
    sections: Vec<ConsentSection>,
}

/// Sealed bytes verified and held until the dialog answers; nothing of it is stored or run yet.
struct StagedBuild {
    bytes: Vec<u8>,
    package: PluginPackage,
    info: BuildInfo,
    is_update: bool,
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

fn semver_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((semver_triplet(candidate), semver_triplet(current)), (Some(a), Some(b)) if a > b)
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
        }
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

    /// Serve jobs until the bridge drops its sender.
    pub fn run(mut self, jobs: Receiver<OpsJob>, events: Sender<ExtensionEvent>) {
        while let Ok(job) = jobs.recv() {
            for op in job.ops {
                for event in self.handle(op, &job.scope) {
                    if events.send(event).is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Execute one op; every outcome ends in an event that clears the panel's working state.
    pub fn handle(&mut self, op: ExtensionOp, scope: &ActiveScope) -> Vec<ExtensionEvent> {
        match self.dispatch(op, scope) {
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
                vec![ExtensionEvent::Browse(BrowseEvent::Listing { source_id, listing }), self.rows_event(scope)]
            }
            Err(error) => {
                vec![ExtensionEvent::Browse(BrowseEvent::Failed { source_id, reason: error.to_string() })]
            }
        }
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
        let source = self.sources.get(source_id).cloned().ok_or_else(|| "unknown extension source".to_owned())?;
        let fetched = source.fetch(entry).map_err(|error| error.to_string())?;
        self.stage(fetched.bytes, Some(&entry.package_id), scope)
    }

    /// Verify, hold the build and ask for consent; an installed id stages an update of that installation.
    fn stage(&mut self, bytes: Vec<u8>, expected_id: Option<&str>, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        let (package, info) = self.verify(&bytes, expected_id)?;
        let installation_id = package.package_id.clone();
        let is_update = match self.installed.get(&installation_id) {
            Some(installed) if installed.info.facts.content_hash.eq_ignore_ascii_case(&package.content_hash) => {
                return Err(format!("{installation_id} {} is already installed", package.version));
            }
            Some(_) => true,
            None => false,
        };
        let request = self.consent_request(&installation_id, &info, is_update, scope);
        self.staged.insert(installation_id, StagedBuild { bytes, package, info, is_update });
        Ok(vec![ExtensionEvent::ConsentNeeded { request, is_update }])
    }

    /// The dialog's data; the refusal and kill-list state come from R19 as the host would judge an enable now.
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
        let consented_digest = installed
            .and_then(|_| manager.governance().consent(installation_id))
            .map(|record| record.capability_digest.clone());
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
            sections: info.sections.clone(),
            signer: info.trusted.then(|| format!("{} (trusted root)", info.signer)),
            policy_verdict,
            licence: if info.facts.licensed { "Paid".to_owned() } else { "Free".to_owned() },
            blocked,
            kill_list,
            origin: Default::default(),
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
        self.ports.persist_consent(&record)?;
        lock(&self.manager).governance_mut().record_consent(record);
        let staged = self.staged.remove(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        self.promote(staged, accept_in_verse, scope)
    }

    /// An update whose digest is already consented: consent on record must equal the staged digest (R19 row 9).
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
        {
            let manager = lock(&self.manager);
            judge(&manager, installation_id, scope, &staged.info.facts).map_err(|refusal| refusal.to_string())?;
        }
        let staged = self.staged.remove(installation_id).ok_or_else(|| format!("no staged package for {installation_id}"))?;
        self.promote(staged, None, scope)
    }

    /// Store the consented build and run it: update beside a running version, start otherwise.
    fn promote(&mut self, staged: StagedBuild, accept_in_verse: Option<String>, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        let StagedBuild { bytes, package, info, is_update } = staged;
        let installation_id = info.facts.package_id.clone();
        // R19 already decided revocation (including the Owner's override), so the store takes no second opinion.
        self.store
            .install(&bytes, Some(&installation_id), &self.plugin_trust(), &|_| false)
            .map_err(|error| format!("package store: {error}"))?;
        let previous = self.installed.get(&installation_id).map(|build| (build.info.facts.content_hash.clone(), build.activated_once));
        let was_running = lock(&self.manager).governance().running_in(&scope.verse_id).contains(&installation_id);
        let may_start = !scope.petal_id.is_empty() && accept_in_verse.as_deref().map_or(true, |verse| verse == scope.verse_id);
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
        let request = self.consent_request(installation_id, &staged.info, staged.is_update, scope);
        Ok(vec![ExtensionEvent::ConsentNeeded { request, is_update: staged.is_update }])
    }

    /// Enable in the open petal; no consent for the digest re-stages the stored build and asks first.
    fn enable(&mut self, installation_id: &str, binding_id: &str, scope: &ActiveScope) -> Result<Vec<ExtensionEvent>, String> {
        if binding_id != scope.petal_id {
            return Err(format!("{binding_id} is not the open petal"));
        }
        let installed = self.installed.get(installation_id).ok_or_else(|| format!("{installation_id} is not installed"))?;
        let facts = installed.info.facts.clone();
        let activated_once = installed.activated_once;
        let bytes = self
            .store
            .get(&facts.package_id, &facts.content_hash)
            .map_err(|error| format!("package store: {error}"))?
            .ok_or_else(|| format!("the stored build of {installation_id} is missing"))?;
        let (package, info) = self.verify(&bytes, Some(&facts.package_id))?;
        if !info.facts.content_hash.eq_ignore_ascii_case(&facts.content_hash) {
            return Err(format!("the stored build of {installation_id} changed on disk"));
        }
        let consented = lock(&self.manager)
            .governance()
            .consent(installation_id)
            .is_some_and(|record| record.capability_digest == info.facts.capability_digest);
        if !consented {
            let request = self.consent_request(installation_id, &info, false, scope);
            self.staged.insert(installation_id.to_owned(), StagedBuild { bytes, package, info, is_update: false });
            return Ok(vec![ExtensionEvent::ConsentNeeded { request, is_update: false }]);
        }
        self.run_build(&package, &info.facts, !activated_once, scope, RunMode::Start)
            .map_err(|error| match error {
                RunError::RolledBack(message) | RunError::Failed(message) => message,
            })?;
        if let Some(installed) = self.installed.get_mut(installation_id) {
            installed.activated_once = true;
        }
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
        if let Err(error) = self.store.remove(&build.info.facts.package_id, &build.info.facts.content_hash) {
            tracing::warn!(%installation_id, %error, "uninstalled extension build not removed");
        }
        Ok(vec![self.rows_event(scope)])
    }

    // ---- Rows -------------------------------------------------------------------------------

    fn rows_event(&self, scope: &ActiveScope) -> ExtensionEvent {
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
                    counters: self.ports.counters(installation_id),
                    audit: self.ports.audit(installation_id),
                    consent,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.package_id.cmp(&b.package_id));
        ExtensionEvent::Rows(rows)
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
    jobs: Sender<OpsJob>,
    events: Receiver<ExtensionEvent>,
}

type WorkerInit = Box<dyn FnOnce() -> OpsWorker + Send>;

/// Spawns the ops worker (built on its own thread) and pumps the panel's ops to it and its events back.
pub struct ExtensionOpsPlugin {
    init: Mutex<Option<WorkerInit>>,
}

impl ExtensionOpsPlugin {
    /// `init` runs on the worker thread: build the sources, hydrate, return the worker.
    pub fn new(init: impl FnOnce() -> OpsWorker + Send + 'static) -> Self {
        Self { init: Mutex::new(Some(Box::new(init))) }
    }
}

impl Plugin for ExtensionOpsPlugin {
    fn build(&self, app: &mut App) {
        let Some(init) = self.init.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() else {
            return;
        };
        let (job_tx, job_rx) = unbounded::<OpsJob>();
        let (event_tx, event_rx) = unbounded::<ExtensionEvent>();
        let spawned = std::thread::Builder::new().name("extension-ops".into()).spawn(move || {
            let mut worker = init();
            worker.hydrate();
            worker.run(job_rx, event_tx);
        });
        if let Err(error) = spawned {
            warn!(%error, "extension ops worker did not start; the Extensions panel stays inert");
        }
        app.insert_resource(ExtensionOpsBridge { jobs: job_tx, events: event_rx })
            .add_systems(Update, pump_extension_ops);
    }
}

/// Hand queued ops to the worker and apply its answers; a dead worker answers `Failed` so the panel never hangs.
fn pump_extension_ops(
    mut panel: ResMut<ExtensionsPanelState>,
    mut consent: ResMut<ConsentState>,
    bridge: Res<ExtensionOpsBridge>,
    navigation: Option<Res<NavigationManager>>,
) {
    let ops = panel.take_ops();
    if !ops.is_empty() {
        let scope = navigation
            .as_deref()
            .map(|nav| ActiveScope {
                verse_id: nav.active_verse_id.clone().unwrap_or_default(),
                verse_name: nav.active_verse_name.clone(),
                petal_id: nav.active_petal_id.clone().unwrap_or_default(),
            })
            .unwrap_or_default();
        if let Err(error) = bridge.jobs.send(OpsJob { ops, scope }) {
            warn!(%error, "extension ops worker is gone");
            panel.apply_event(ExtensionEvent::Failed { message: "the extension host is not running".into() }, &mut consent);
        }
    }
    while let Ok(event) = bridge.events.try_recv() {
        panel.apply_event(event, &mut consent);
    }
}
