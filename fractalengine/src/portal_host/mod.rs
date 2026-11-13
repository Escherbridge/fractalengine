//! Portal host wiring (P-S1, P-S2, P-S4, P-S6): the bundle resolver, the bus broker, and the binary's drains behind the
//! Repository section (summary, Admit, Revoke) and the revoke listener's signals. Rationale: src/AGENTS.md §portal-host.

pub(crate) mod broker;
pub(crate) mod install;
pub(crate) mod resolver;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bevy::ecs::change_detection::DetectChangesMut;
use bevy::prelude::{App, IntoScheduleConfigs, Local, MessageWriter, Res, ResMut, Resource, Update};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use crossbeam::channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use fe_api::portal_bundles::{inspect_stored, InspectedBundle, StoredBundleError};
use fe_database::handlers::extension_policy::{AuditAction, AuditEntry, AuditOutcome};
use fe_database::repo::Db;
use fe_database::RoleLevel;
use fe_plugin::bus::sources::TracingAudit;
use fe_plugin::bus::{AuditSink, RoleSource};
use fe_plugin::lifecycle::{ConsentRecord, PackageFacts, TrustTier};
use fe_runtime::blob_store::{hash_from_hex, BlobHash, BlobStoreHandle};
use fe_runtime::messages::{DbCommand, SceneChange};
use fe_runtime::service_lifecycle::{OwnedService, StopSignal};
use fe_ui::actions::UiManager;
use fe_ui::extensions::consent::{self, ConsentDecision, ConsentOrigin, ConsentRequest, ConsentState, KillListOverrideOffer};
use fe_ui::notice::NoticeClock;
use fe_ui::portal::repository_view::parse_summary;
use fe_ui::portal::BundleAdmissionRequest;
use fe_ui::verse_manager::VerseManager;
use fe_webview::ipc::BundleState;
use fe_webview::plugin::{apply_portal_admission, PortalBindingChanged, PortalBundleChanged};
use fe_webview::portal_bundle::admission::{
    self, AdmissionEvent, AdmissionSignal, GovernanceView, KillListStatus, RecordReason, RecordState,
};
use fe_webview::portal_bundle::{AdmissionVerdict, BundleIdentity, BundleResolver, BundleTarget};
use serde_json::{json, Value};
use tokio::runtime::Runtime;
use tokio::sync::broadcast::error::TryRecvError;

use crate::extension_host::governance::ExtensionGovernance;
use crate::render_wake::PendingWork;
use broker::{BusPortalBroker, InstallationLookup, PortalScopeIndex};
use resolver::{AdmissionWriters, GovernanceSource, PortalGovernanceConfig, PortalResolver};

/// Jobs the worker holds at once; a full queue leaves the request for the next frame.
const JOB_QUEUE: usize = 64;
/// Nodes tracked per map (bundle assets, awaited blobs, retries); past it the map starts over.
const MAX_TRACKED_NODES: usize = 256;
/// Re-judge cadence of an open bundle portal against governance, and of awaited-blob checks.
const REJUDGE_EVERY: Duration = Duration::from_millis(500);
/// Bound on one worker DB round trip.
const DB_REPLY: Duration = Duration::from_secs(5);
/// Worker poll interval while idle (stop latency).
const WORKER_POLL: Duration = Duration::from_millis(100);
/// Startup consent reads attempted before the portal view gives up (fails closed: bundles open static).
const HYDRATE_ATTEMPTS: u32 = 6;
const HYDRATE_RETRY: Duration = Duration::from_secs(2);
/// Scene-change events read per frame.
const MAX_SCENE_EVENTS_PER_FRAME: usize = 256;
/// The node property naming a repository node's bundle asset (README §9.1).
const BUNDLE_ASSET_KEY: &str = "portal.bundle_asset";
const NODE_KIND_KEY: &str = "node_kind";
/// The installation kind of an admitted web bundle (A-14).
const WEB_BUNDLE_KIND: &str = "web-bundle";
const NO_GOVERNANCE: &str = "Extension governance is not running on this host.";
const ASSET_SQL: &str = "SELECT content_hash FROM asset WHERE asset_id = $asset LIMIT 1";
const VERSE_OWNER_SQL: &str = "SELECT created_by FROM verse WHERE verse_id = $verse AND tombstone = NONE LIMIT 1";

/// What `main.rs` hands the portal host.
pub(crate) struct PortalHostConfig {
    /// Clone of the DB thread's shared handle; read only.
    pub db: Db,
    pub blobs: BlobStoreHandle,
    /// The committed-scene broadcast (`entity_change_tx`): a repository node's `portal.bundle_asset` changes.
    pub scene_changes: tokio::sync::broadcast::Receiver<SceneChange>,
    /// The client DID of this host's dev API tokens (fe-api's summary `admission` block); `None` serves it to nobody.
    pub local_client_did: Option<String>,
}

/// Install the resolver, the broker and the drains; call after the extension host installs the bus and governance and
/// after `WebViewPlugin`. The returned worker joins the other host services on exit.
pub(crate) fn install(app: &mut App, config: PortalHostConfig, render_work: &PendingWork) -> OwnedService {
    let governance = app.world().get_resource::<ExtensionGovernance>().cloned();
    if governance.is_none() {
        tracing::error!("portal host: extension governance is not installed; bundles render in the static sandbox only");
    }
    let mut governance_config = PortalGovernanceConfig::from_env();
    if let Some(admission) = app.world().get_resource::<fe_plugin::PackageAdmission>() {
        governance_config.admission_enabled = admission.0;
    }
    let source: Arc<dyn GovernanceSource> = match &governance {
        Some(governance) => Arc::new(governance.clone()),
        None => Arc::new(NoGovernance),
    };
    let resolver = Arc::new(PortalResolver::new(config.db.clone(), config.blobs.clone(), source, governance_config));
    resolver::install(app, Arc::clone(&resolver), config.local_client_did);
    // Feeds the binding facts (admitted set) and Disable all into the plugin bridge (R-X-04, R-X-33).
    crate::plugin_bridge::link_portal_resolver(Arc::clone(resolver.view()));
    configure_install(governance.as_ref(), &config.blobs);
    install_broker(app, &resolver, governance.as_ref());

    let (jobs, job_rx) = crossbeam::channel::bounded(JOB_QUEUE);
    let (outcome_tx, outcomes) = crossbeam::channel::unbounded();
    render_work.watch(outcomes.clone());
    if jobs.try_send(Job::Hydrate).is_err() {
        tracing::warn!("portal host: consent hydration was not queued");
    }
    let worker = Worker {
        resolver: Arc::clone(&resolver),
        governance,
        admission_enabled: governance_config.admission_enabled,
        db: config.db,
        db_tx: app.world().resource::<fe_runtime::app::DbCommandSender>().0.clone(),
        blobs: config.blobs.clone(),
    };
    app.insert_resource(PortalHost {
        jobs,
        outcomes,
        resolver,
        blobs: config.blobs,
        scene_changes: Mutex::new(config.scene_changes),
        pending: None,
        assets: HashMap::new(),
        awaiting: HashMap::new(),
        reading: HashSet::new(),
        stale: HashSet::new(),
        retry: Vec::new(),
        next_tick: None,
        governance_seen: None,
    });
    app.init_resource::<PortalConsent>();
    app.add_systems(Update, (drive_portal_host, follow_binding_changes).in_set(AdmissionWriters));
    app.add_systems(EguiPrimaryContextPass, render_portal_consent);
    OwnedService::spawn("portal-host", move |stop| worker.run(&job_rx, &outcome_tx, &stop))
}

/// Registry installs of web bundles check the accepted kill list; without governance the path stays unconfigured (refuses).
fn configure_install(governance: Option<&ExtensionGovernance>, blobs: &BlobStoreHandle) {
    let Some(governance) = governance.cloned() else {
        tracing::warn!("portal host: no governance, so registry web-bundle installs are refused");
        return;
    };
    let kill_list: install::KillListPredicate = Arc::new(move |digest: &str| governance.is_revoked_any(&[digest]));
    if !install::configure(install::InstallContext { blobs: blobs.clone(), kill_list }) {
        tracing::debug!("portal host: the install context was already configured");
    }
}

/// The broker over the integration bus, its one-op write sink and the scope tracker; no bus means no IPC data.
fn install_broker(app: &mut App, resolver: &Arc<PortalResolver>, governance: Option<&ExtensionGovernance>) {
    let scopes = PortalScopeIndex::default();
    app.insert_resource(scopes.clone());
    app.add_systems(Update, broker::track_portal_scopes.before(apply_portal_admission));
    let view: Arc<dyn GovernanceView> = resolver.view().clone();
    // P-BROKER: share the extension host's bus, role source and audit sink when it built them.
    let shared = app.world().get_resource::<crate::plugin_bridge::ExtensionBusParts>().cloned();
    let (bus, broker) = match shared {
        Some(parts) => (parts.bus.clone(), BusPortalBroker::from_parts(&parts, scopes, view)),
        None => {
            let Some(bus) = app.world().get_resource::<fe_plugin::BusHandle>().map(|handle| handle.0.clone()) else {
                tracing::error!("portal host: no integration bus; admitted bundles answer every call `unavailable`");
                return;
            };
            let roles: Arc<dyn RoleSource> = Arc::new(NodeDidRoles(bus.local_did().to_owned()));
            let audit: Arc<dyn AuditSink> = match governance {
                Some(governance) => governance.audit_sink(),
                None => Arc::new(TracingAudit),
            };
            let broker = BusPortalBroker::new(bus.clone(), roles, audit, scopes).with_governance(view);
            (bus, broker)
        }
    };
    // A-14: an admitted bundle is the installation keyed by its package id (the ops drain's convention).
    let installations: Arc<InstallationLookup> = Arc::new(|package_id: &str, _content_hash: &str| Some(package_id.to_owned()));
    let broker = broker.with_installations(installations);
    let db = app.world().resource::<fe_runtime::app::DbCommandSender>().0.clone();
    // Before `into_resource`: the sink shares the broker's binding table.
    if !bus.install_portal_writes(Arc::new(broker.write_sink(db))) {
        tracing::warn!("portal host: the bus already had a portal write sink; this broker's writes go to that one");
    }
    app.insert_resource(broker.into_resource());
}

/// The bus role view for the only DID a portal binding names, the node DID (Owner, as `plugin_bridge`'s bus roles);
/// the binding's real cap is fe-ui's resolved local role and the viewer token (`broker::BusPortalBroker::ceiling`).
struct NodeDidRoles(String);

impl RoleSource for NodeDidRoles {
    fn role_at(&self, did: &str, _petal_id: &str) -> RoleLevel {
        if did == self.0 {
            RoleLevel::Owner
        } else {
            RoleLevel::None
        }
    }
}

/// Governance when the extension host did not install it: no admission, no list in force (bundles render static).
struct NoGovernance;

impl GovernanceSource for NoGovernance {
    fn listed(&self, _digests: &[&str]) -> bool {
        false
    }
    fn overridden_in(&self, _verse_id: &str, _content_hash: &str) -> bool {
        false
    }
    fn tier_of(&self, _publisher_did: &str) -> Option<TrustTier> {
        None
    }
    fn policy_admits(&self, _verse_id: &str, _facts: &PackageFacts) -> Result<(), String> {
        Err(NO_GOVERNANCE.into())
    }
    fn may_read(&self, _petal_id: &str) -> Result<(), String> {
        Err(NO_GOVERNANCE.into())
    }
}

// ---------------------------------------------------------------------------
// Worker: DB reads, archive inspection and governance writes, off the frame
// ---------------------------------------------------------------------------

enum Job {
    /// Learn this host's recorded consents into the portal view (startup).
    Hydrate,
    Summary { node_id: String },
    /// `verse` is `(verse_id, verse_name)` from the in-memory tree, for the kill-list override offer.
    Admit { node_id: String, verse: Option<(String, String)> },
    Consent { node_id: String, package_id: String, content_hash: String, capability_digest: String },
    Override { node_id: String, request: Value },
    Revoke { node_id: String },
}

enum Outcome {
    /// `(package_id, content_hash, capability_digest)` of every local consent.
    Hydrated(Vec<(String, String, String)>),
    Summary { node_id: String, asset_id: String, data: Value },
    Waiting { node_id: String, asset_id: String, blob: BlobHash },
    Unreadable { node_id: String },
    ConsentOffer { node_id: String, request: Box<ConsentRequest> },
    Consented { node_id: String, package_id: String, content_hash: String, capability_digest: String },
    Overridden { node_id: String },
    /// Admission is withdrawn in-session even when `warning` says the durable record is uncertain (fails closed).
    Revoked { node_id: String, package_id: String, content_hash: String, warning: Option<String> },
    Failed { node_id: String, message: String },
}

fn failed(node_id: String, message: impl Into<String>) -> Outcome {
    Outcome::Failed { node_id, message: message.into() }
}

/// A repository node's current build.
enum BuildRead {
    Ready { asset_id: String, blob_hex: String, inspected: Box<InspectedBundle> },
    Waiting { asset_id: String, blob: BlobHash },
    Unreadable,
}

struct Worker {
    resolver: Arc<PortalResolver>,
    governance: Option<ExtensionGovernance>,
    admission_enabled: bool,
    db: Db,
    db_tx: Sender<DbCommand>,
    blobs: BlobStoreHandle,
}

impl Worker {
    fn run(self, jobs: &Receiver<Job>, outcomes: &Sender<Outcome>, stop: &StopSignal) {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error, "portal host worker did not start; bundle summaries and admission stay unavailable");
                return;
            }
        };
        while !stop.is_requested() {
            let job = match jobs.recv_timeout(WORKER_POLL) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let outcome = match job {
                Job::Hydrate => self.hydrate(stop),
                Job::Summary { node_id } => Some(self.summary(&runtime, node_id)),
                Job::Admit { node_id, verse } => Some(self.admit(&runtime, node_id, verse)),
                Job::Consent { node_id, package_id, content_hash, capability_digest } => {
                    Some(self.consent(node_id, package_id, content_hash, capability_digest))
                }
                Job::Override { node_id, request } => Some(self.override_kill_list(node_id, &request)),
                Job::Revoke { node_id } => Some(self.revoke(&runtime, node_id)),
            };
            if outcome.is_some_and(|outcome| outcomes.send(outcome).is_err()) {
                return;
            }
        }
    }

    /// The portal view learns this host's consents (governance hands them only to the plugin manager).
    fn hydrate(&self, stop: &StopSignal) -> Option<Outcome> {
        let governance = self.governance.as_ref()?;
        for attempt in 1..=HYDRATE_ATTEMPTS {
            let (reply, answer) = crossbeam::channel::bounded(1);
            if self.db_tx.send_timeout(DbCommand::LoadExtensionGovernance { reply }, DB_REPLY).is_ok() {
                match answer.recv_timeout(DB_REPLY) {
                    Ok(Ok(snapshot)) => return Some(Outcome::Hydrated(local_consents(&snapshot, governance.local_did()))),
                    Ok(Err(error)) => tracing::warn!(attempt, %error, "portal consent hydration read failed"),
                    Err(_) => tracing::warn!(attempt, "portal consent hydration read did not answer"),
                }
            }
            if stop.is_requested() {
                return None;
            }
            std::thread::sleep(HYDRATE_RETRY);
        }
        tracing::error!("portal consents were not hydrated; admitted bundles open in the static sandbox until re-admitted");
        None
    }

    /// One text column of the first row of a parameterised SELECT; `None` on a DB error, timeout or no row.
    fn text(&self, runtime: &Runtime, sql: &'static str, name: &'static str, value: &str, column: &str) -> Option<String> {
        let query = self.db.query(sql).bind((name, value.to_owned()));
        let rows = runtime.block_on(async move {
            let response = tokio::time::timeout(DB_REPLY, async move { query.await }).await.ok()?.ok()?;
            let mut checked = response.check().ok()?;
            checked.take::<Vec<Value>>(0).ok()
        })?;
        rows.into_iter().next()?.get(column)?.as_str().map(str::to_owned)
    }

    /// The resolver's chain (it remembers the node's scope and checks the kill list before bytes), then the archive.
    fn read_build(&self, runtime: &Runtime, node_id: &str) -> BuildRead {
        // Called outside `block_on`: the resolver refuses to block on a runtime thread.
        let Ok(source) = self.resolver.resolve(node_id) else {
            return BuildRead::Unreadable;
        };
        let Some(blob_hex) = self
            .text(runtime, ASSET_SQL, "asset", &source.asset_id, "content_hash")
            .map(|hex| hex.to_ascii_lowercase())
        else {
            return BuildRead::Unreadable;
        };
        let Ok(blob) = hash_from_hex(&blob_hex) else {
            return BuildRead::Unreadable;
        };
        match inspect_stored(&self.blobs, &blob_hex) {
            Ok(inspected) => BuildRead::Ready { asset_id: source.asset_id, blob_hex, inspected: Box::new(inspected) },
            Err(StoredBundleError::BlobMissing) => BuildRead::Waiting { asset_id: source.asset_id, blob },
            Err(error) => {
                tracing::debug!(node_id, ?error, "portal bundle summary: build unreadable");
                BuildRead::Unreadable
            }
        }
    }

    fn summary(&self, runtime: &Runtime, node_id: String) -> Outcome {
        match self.read_build(runtime, &node_id) {
            BuildRead::Ready { asset_id, blob_hex, inspected } => {
                let data = self.summary_data(&node_id, &asset_id, &blob_hex, &inspected);
                Outcome::Summary { node_id, asset_id, data }
            }
            BuildRead::Waiting { asset_id, blob } => Outcome::Waiting { node_id, asset_id, blob },
            BuildRead::Unreadable => Outcome::Unreadable { node_id },
        }
    }

    /// README §8.5 as the local identity, judged at this node's scope (so `cannot_admit` reflects policy and role).
    fn summary_data(&self, node_id: &str, asset_id: &str, blob_hex: &str, inspected: &InspectedBundle) -> Value {
        let identity = build_identity(blob_hex, inspected);
        let view: &dyn GovernanceView = &**self.resolver.view();
        let verdict = admission::verdict(view, &BundleTarget::Node(node_id.to_owned()), &identity);
        let (state, mut reason) = admission::summary_codes(&verdict);
        if matches!(verdict, AdmissionVerdict::Unadmitted { can_admit: false }) {
            reason = Some("cannot_admit");
        }
        let portal = &inspected.validated.portal;
        json!({
            "asset_id": asset_id,
            "title": portal.title,
            "package_id": identity.package_id,
            "version": inspected.validated.version,
            "content_hash": identity.content_hash,
            "sealed_by": identity.publisher_did,
            "capability_digest": identity.capability_digest,
            "capabilities": broker::granted_capabilities(portal),
            "admission": { "state": state, "reason": reason },
        })
    }

    /// Governance's consent dialog for this node's exact build; refusals and the Owner's kill-list override are in it.
    fn admit(&self, runtime: &Runtime, node_id: String, verse: Option<(String, String)>) -> Outcome {
        let BuildRead::Ready { blob_hex, inspected, .. } = self.read_build(runtime, &node_id) else {
            return failed(node_id, "Admit needs this bundle's bytes on this peer; try again once they arrive.");
        };
        let identity = build_identity(&blob_hex, &inspected);
        let target = BundleTarget::Node(node_id.clone());
        let view: &dyn GovernanceView = &**self.resolver.view();
        let listed = admission::kill_list_status(view, &target, &identity.digests(), &identity.content_hash)
            == KillListStatus::Listed;
        let kill_list = match verse {
            Some((verse_id, verse_name)) if listed && self.is_verse_owner(runtime, &verse_id) => Some(KillListOverrideOffer {
                reason: "Listed on an accepted revocation list".into(),
                verse_id,
                verse_name,
            }),
            _ => None,
        };
        let blocked = self.refusal(view, &target, &identity, listed && kill_list.is_none());
        let policy_verdict = match (&blocked, &kill_list) {
            (None, None) => "Admitted by verse and local policy",
            (None, Some(_)) => "Refused: on an accepted revocation list",
            (Some(_), _) => "Refused",
        };
        let portal = &inspected.validated.portal;
        let request = ConsentRequest {
            installation_id: identity.package_id.clone(),
            package_id: identity.package_id.clone(),
            name: portal.title.clone(),
            version: inspected.validated.version.clone(),
            kind: WEB_BUNDLE_KIND.into(),
            content_hash: identity.content_hash.clone(),
            capability_digest: identity.capability_digest.clone(),
            signer: self.signer(identity.publisher_did.as_deref()),
            policy_verdict: policy_verdict.into(),
            licence: "Not required".into(),
            blocked,
            kill_list,
            origin: ConsentOrigin::Direct,
            // The dialog renders this as its grant list (A-10); `sections` stays empty so nothing shows twice.
            portal: serde_json::to_value(portal).ok(),
            ..ConsentRequest::default()
        };
        Outcome::ConsentOffer { node_id, request: Box::new(request) }
    }

    /// The first reason the local identity cannot admit this build here, in the order a user can act on it.
    fn refusal(&self, view: &dyn GovernanceView, target: &BundleTarget, identity: &BundleIdentity, listed: bool) -> Option<String> {
        let Some(governance) = &self.governance else {
            return Some(NO_GOVERNANCE.into());
        };
        if !self.admission_enabled {
            return Some(format!("Bundle admission is off on this host ({} is not 1).", fe_plugin::PLUGIN_HOST_ENV));
        }
        if view.host_disabled() {
            return Some("Extensions are disabled on this host (Disable all).".into());
        }
        let Some(publisher) = identity.publisher_did.as_deref() else {
            return Some("Unsealed bundles are never admitted.".into());
        };
        if governance.trust().tier_for(Some(publisher)).is_none() {
            return Some(format!("{publisher} is not a trusted publisher on this host."));
        }
        if listed {
            return Some("This build is on an accepted revocation list; only the verse Owner can override it on their own peer.".into());
        }
        if !view.can_admit(target, identity) {
            return Some("The verse policy or your role at this petal does not allow this bundle.".into());
        }
        None
    }

    fn signer(&self, publisher: Option<&str>) -> Option<String> {
        let did = publisher?;
        let tier = self.governance.as_ref().and_then(|governance| governance.trust().tier_for(Some(did)));
        Some(match tier {
            Some(tier) => format!("{did} (trusted {} root)", tier_label(tier)),
            None => format!("{did} (not a trusted root on this host)"),
        })
    }

    /// The local identity created `verse_id` (the DB re-checks Owner when the override is recorded).
    fn is_verse_owner(&self, runtime: &Runtime, verse_id: &str) -> bool {
        let Some(governance) = &self.governance else {
            return false;
        };
        self.text(runtime, VERSE_OWNER_SQL, "verse", verse_id, "created_by")
            .is_some_and(|owner| owner == governance.local_did())
    }

    /// Durable first (one `consent` audit entry, then the plugin manager); the portal admits only on success.
    fn consent(&self, node_id: String, package_id: String, content_hash: String, capability_digest: String) -> Outcome {
        let Some(governance) = &self.governance else {
            return failed(node_id, NO_GOVERNANCE);
        };
        let record = ConsentRecord {
            installation_id: package_id.clone(),
            package_id: package_id.clone(),
            content_hash: content_hash.clone(),
            capability_digest: capability_digest.clone(),
            actor_did: governance.local_did().to_owned(),
            // Stamped by the DB's audit append.
            at: String::new(),
        };
        match governance.record_consent(record) {
            Ok(()) => Outcome::Consented { node_id, package_id, content_hash, capability_digest },
            Err(reason) => failed(node_id, format!("Admit was not recorded: {reason}")),
        }
    }

    /// The verse Owner's audited override for this build in this verse, on this host only.
    fn override_kill_list(&self, node_id: String, request: &Value) -> Outcome {
        let Some(governance) = &self.governance else {
            return failed(node_id, NO_GOVERNANCE);
        };
        match governance.kill_list_override(governance.local_did(), request) {
            Ok(_) => Outcome::Overridden { node_id },
            Err(refusal) => failed(node_id, format!("Kill-list override refused: {}", refusal.message)),
        }
    }

    /// One durable `consent-revoke` entry (rows written through the bundle stay, OD-E1); admission is withdrawn either way.
    fn revoke(&self, runtime: &Runtime, node_id: String) -> Outcome {
        let BuildRead::Ready { inspected, .. } = self.read_build(runtime, &node_id) else {
            return failed(node_id, "Revoke needs this bundle's bytes on this peer to identify the build.");
        };
        let package_id = inspected.validated.package_id.clone();
        let content_hash = inspected.content_hash.clone();
        let warning = self.record_revoke(&package_id, &content_hash).err();
        Outcome::Revoked { node_id, package_id, content_hash, warning }
    }

    fn record_revoke(&self, package_id: &str, content_hash: &str) -> Result<(), String> {
        let governance = self.governance.as_ref().ok_or_else(|| NO_GOVERNANCE.to_owned())?;
        let entry = AuditEntry {
            action: AuditAction::ConsentRevoke,
            installation: Some(package_id.to_owned()),
            package_id: Some(package_id.to_owned()),
            content_hash: Some(content_hash.to_owned()),
            actor_did: governance.local_did().to_owned(),
            target: package_id.to_owned(),
            outcome: AuditOutcome::Ok,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: String::new(),
        };
        let entry = serde_json::to_value(&entry).map_err(|error| format!("Revoke was not recorded: {error}"))?;
        let (reply, answer) = crossbeam::channel::bounded(1);
        self.db_tx
            .send_timeout(DbCommand::AppendExtensionAudit { entry, reply }, DB_REPLY)
            .map_err(|_| "Revoke was not recorded: the database is not accepting commands.".to_owned())?;
        match answer.recv_timeout(DB_REPLY) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(reason)) => Err(format!("Revoke was not recorded and lasts until restart: {reason}")),
            Err(_) => Err("Revoke may not survive a restart: the database did not answer.".to_owned()),
        }
    }
}

/// Governance's identity for a build: hash under any id (content hash, archive digest, committed member digests).
fn build_identity(blob_hex: &str, inspected: &InspectedBundle) -> BundleIdentity {
    BundleIdentity {
        package_id: inspected.validated.package_id.clone(),
        content_hash: inspected.content_hash.clone(),
        archive_digest: blob_hex.to_owned(),
        publisher_did: inspected.sealed_by.clone(),
        capability_digest: inspected.validated.capability_digest.clone(),
        member_digests: inspected.members.iter().filter_map(|member| member.blake3.clone()).collect(),
    }
}

/// `(package_id, content_hash, capability_digest)` of each consent the local DID gave (a rotated key's never count).
fn local_consents(snapshot: &Value, local_did: &str) -> Vec<(String, String, String)> {
    let field = |record: &Value, key: &str| record.get(key).and_then(Value::as_str).map(str::to_owned);
    snapshot
        .get("consents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|record| record.get("actor_did").and_then(Value::as_str) == Some(local_did))
        .filter_map(|record| {
            Some((field(record, "package_id")?, field(record, "content_hash")?, field(record, "capability_digest")?))
        })
        .collect()
}

fn tier_label(tier: TrustTier) -> &'static str {
    match tier {
        TrustTier::Release => "release",
        TrustTier::Publisher => "publisher",
        TrustTier::Dev | TrustTier::DevUnsigned => "dev",
    }
}

/// The verse holding `node_id` in the in-memory tree, as `(verse_id, verse_name)`.
fn verse_of(tree: &VerseManager, node_id: &str) -> Option<(String, String)> {
    let petal_id = tree.petal_id_of(node_id)?;
    tree.verses
        .iter()
        .find(|verse| verse.fractals.iter().any(|fractal| fractal.petals.iter().any(|petal| petal.id == petal_id)))
        .map(|verse| (verse.id.clone(), verse.name.clone()))
}

// ---------------------------------------------------------------------------
// Main thread: requests out, outcomes and signals in
// ---------------------------------------------------------------------------

/// The Admit awaiting the consent dialog's answer.
struct PendingAdmit {
    node_id: String,
    installation_id: String,
    package_id: String,
    content_hash: String,
    capability_digest: String,
}

/// The portal's own instance of governance's consent dialog: its answers reach only this host, never the ops drain.
#[derive(Resource, Default)]
struct PortalConsent(ConsentState);

#[derive(Resource)]
struct PortalHost {
    jobs: Sender<Job>,
    outcomes: Receiver<Outcome>,
    resolver: Arc<PortalResolver>,
    blobs: BlobStoreHandle,
    scene_changes: Mutex<tokio::sync::broadcast::Receiver<SceneChange>>,
    pending: Option<PendingAdmit>,
    /// node -> the bundle asset last read for it; a different one is a `PortalBundleChanged`.
    assets: HashMap<String, String>,
    /// node -> the blob whose bytes have not arrived.
    awaiting: HashMap<String, BlobHash>,
    /// Nodes with a summary read queued or running.
    reading: HashSet<String>,
    /// Nodes whose running read predates an admission or asset change: read again when it lands.
    stale: HashSet<String>,
    /// Summary reads a full queue turned away.
    retry: Vec<String>,
    next_tick: Option<Instant>,
    /// Governance's change generation at the last re-judge; a move re-judges before the timer (R-POR-T3-08).
    governance_seen: Option<u64>,
}

impl PortalHost {
    fn queue_summary(&mut self, node_id: String) {
        if self.reading.contains(&node_id) {
            self.stale.insert(node_id);
            return;
        }
        match self.jobs.try_send(Job::Summary { node_id: node_id.clone() }) {
            Ok(()) => {
                self.reading.insert(node_id);
            }
            Err(TrySendError::Full(_)) if self.retry.len() < MAX_TRACKED_NODES => self.retry.push(node_id),
            Err(_) => {}
        }
    }

    /// A summary read for `node_id` landed; read again if it went stale meanwhile.
    fn read_done(&mut self, node_id: &str) {
        self.reading.remove(node_id);
        if self.stale.remove(node_id) {
            self.queue_summary(node_id.to_owned());
        }
    }

    /// Remember the node's bundle asset; true when it replaced a different one.
    fn note_asset(&mut self, node_id: &str, asset_id: &str) -> bool {
        match self.assets.get(node_id) {
            Some(known) if known == asset_id => false,
            Some(_) => {
                self.assets.insert(node_id.to_owned(), asset_id.to_owned());
                true
            }
            None => {
                if self.assets.len() >= MAX_TRACKED_NODES {
                    self.assets.clear();
                }
                self.assets.insert(node_id.to_owned(), asset_id.to_owned());
                false
            }
        }
    }

    /// Summary reads, then Admit/Revoke, to the worker; a full queue keeps the rest for the next frame.
    fn queue_requests(&mut self, ui: &mut UiManager, tree: &VerseManager) {
        for node_id in std::mem::take(&mut self.retry) {
            self.queue_summary(node_id);
        }
        for node_id in ui.repository_view.take_wanted() {
            self.queue_summary(node_id);
        }
        if ui.bundle_admission_requests.is_empty() {
            return;
        }
        let mut left = Vec::new();
        for request in std::mem::take(&mut ui.bundle_admission_requests) {
            if !left.is_empty() {
                left.push(request);
                continue;
            }
            let job = match &request {
                BundleAdmissionRequest::Admit { node_id } => {
                    Job::Admit { node_id: node_id.clone(), verse: verse_of(tree, node_id) }
                }
                BundleAdmissionRequest::Revoke { node_id } => Job::Revoke { node_id: node_id.clone() },
            };
            if let Err(TrySendError::Full(_)) = self.jobs.try_send(job) {
                left.push(request);
            }
        }
        ui.bundle_admission_requests = left;
    }

    /// The consent dialog's answer to the pending Admit becomes a governance write on the worker.
    fn route_decision(&mut self, dialog: &mut ConsentState, ui: &mut UiManager, now_secs: f64) {
        let Some(decision) = dialog.take_decision() else {
            return;
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        let job = match decision {
            ConsentDecision::Approved { installation_id, capability_digest, .. }
                if installation_id == pending.installation_id && capability_digest == pending.capability_digest =>
            {
                Job::Consent {
                    node_id: pending.node_id,
                    package_id: pending.package_id,
                    content_hash: pending.content_hash,
                    capability_digest,
                }
            }
            ConsentDecision::KillListOverride { installation_id, verse_id, package_id, content_hash, confirm }
                if installation_id == pending.installation_id =>
            {
                Job::Override {
                    node_id: pending.node_id,
                    request: json!({ "verse_id": verse_id, "package_id": package_id, "content_hash": content_hash, "confirm": confirm }),
                }
            }
            // Declined, or an answer that is not this Admit's.
            _ => return,
        };
        if self.jobs.try_send(job).is_err() {
            ui.show_error("Bundle admission is busy; nothing was recorded. Try Admit again.", now_secs);
        }
    }

    fn apply_outcomes(
        &mut self,
        dialog: &mut ConsentState,
        ui: &mut UiManager,
        signals: &mut MessageWriter<AdmissionSignal>,
        changes: &mut MessageWriter<PortalBundleChanged>,
        now_secs: f64,
    ) {
        while let Ok(outcome) = self.outcomes.try_recv() {
            match outcome {
                Outcome::Hydrated(consents) => {
                    for (package_id, content_hash, capability_digest) in &consents {
                        self.resolver.view().learn_consent(package_id, content_hash, capability_digest);
                    }
                    // A bundle opened before hydration re-judges (Static -> Admitted).
                    signals.write(AdmissionSignal::ListAccepted);
                }
                Outcome::Summary { node_id, asset_id, data } => {
                    self.awaiting.remove(&node_id);
                    if self.note_asset(&node_id, &asset_id) {
                        changes.write(PortalBundleChanged { node_id: node_id.clone() });
                    }
                    if let Some(summary) = parse_summary(&data) {
                        feed_bundle_node(&mut ui.portal_bundle_nodes, &node_id, &summary);
                        ui.repository_view.ingest(&node_id, summary);
                    }
                    self.read_done(&node_id);
                }
                Outcome::Waiting { node_id, asset_id, blob } => {
                    if self.note_asset(&node_id, &asset_id) {
                        changes.write(PortalBundleChanged { node_id: node_id.clone() });
                    }
                    if self.awaiting.len() < MAX_TRACKED_NODES || self.awaiting.contains_key(&node_id) {
                        self.awaiting.insert(node_id.clone(), blob);
                    }
                    self.read_done(&node_id);
                }
                Outcome::Unreadable { node_id } => self.read_done(&node_id),
                Outcome::ConsentOffer { node_id, request } => {
                    self.pending = Some(PendingAdmit {
                        node_id,
                        installation_id: request.installation_id.clone(),
                        package_id: request.package_id.clone(),
                        content_hash: request.content_hash.clone(),
                        capability_digest: request.capability_digest.clone(),
                    });
                    dialog.request(*request);
                }
                Outcome::Consented { node_id, package_id, content_hash, capability_digest } => {
                    // The record (with its digest) first, so the listener's re-judge in this frame sees Admitted.
                    self.resolver.view().learn_consent(&package_id, &content_hash, &capability_digest);
                    signals.write(AdmissionSignal::Event(AdmissionEvent {
                        package_id,
                        content_hash,
                        state: RecordState::Admitted,
                        reason: RecordReason::Consented,
                    }));
                    ui.repository_view.invalidate(&node_id);
                }
                Outcome::Overridden { node_id } => {
                    signals.write(AdmissionSignal::ListAccepted);
                    ui.repository_view.invalidate(&node_id);
                }
                Outcome::Revoked { node_id, package_id, content_hash, warning } => {
                    // bus.md §2.4 pairs `consent-revoked` with `disabled` (rule 5, "admission withdrawn", per pair);
                    // `revoked` is the kill list's alone, which matches by hash under any id.
                    signals.write(AdmissionSignal::Event(AdmissionEvent {
                        package_id,
                        content_hash,
                        state: RecordState::Disabled,
                        reason: RecordReason::ConsentRevoked,
                    }));
                    ui.repository_view.invalidate(&node_id);
                    if let Some(warning) = warning {
                        ui.show_error(warning, now_secs);
                    }
                }
                Outcome::Failed { node_id, message } => {
                    tracing::warn!(%node_id, %message, "portal admission request failed");
                    ui.show_error(message, now_secs);
                }
            }
        }
    }

    /// A repository node's bundle reference changed or the node went away: re-read it (the read decides whether the
    /// open portal reloads), or tell the listener directly when no earlier read exists to compare against.
    fn follow_scene_changes(&mut self, ui: &UiManager, changes: &mut MessageWriter<PortalBundleChanged>) {
        let open = ui.portal.bundle_node_id();
        let mut reread = Vec::new();
        {
            let mut receiver = self.scene_changes.lock().unwrap_or_else(PoisonError::into_inner);
            for _ in 0..MAX_SCENE_EVENTS_PER_FRAME {
                match receiver.try_recv() {
                    Ok(SceneChange::PropertyChanged { node_id, key, .. })
                        if key == BUNDLE_ASSET_KEY || key == NODE_KIND_KEY =>
                    {
                        let is_open = open == Some(node_id.as_str());
                        if is_open && !self.assets.contains_key(&node_id) {
                            changes.write(PortalBundleChanged { node_id: node_id.clone() });
                        }
                        if is_open || self.assets.contains_key(&node_id) {
                            reread.push(node_id);
                        }
                    }
                    Ok(SceneChange::NodeRemoved { node_id, .. }) => {
                        self.assets.remove(&node_id);
                        self.awaiting.remove(&node_id);
                        if open == Some(node_id.as_str()) {
                            changes.write(PortalBundleChanged { node_id });
                        }
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "portal host: scene changes lagged; re-reading the open bundle");
                        reread.extend(open.map(str::to_owned));
                    }
                    Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                }
            }
        }
        for node_id in reread {
            self.queue_summary(node_id);
        }
    }

    /// Every `REJUDGE_EVERY` while a bundle portal is open: re-judge it against governance (list acceptance, policy
    /// rows, roles and overrides change without a `host/admission` event), and report awaited bytes that arrived.
    fn tick(
        &mut self,
        ui: &UiManager,
        signals: &mut MessageWriter<AdmissionSignal>,
        changes: &mut MessageWriter<PortalBundleChanged>,
    ) {
        let bundle_open = ui.portal.is_bundle();
        if !bundle_open && self.awaiting.is_empty() {
            return;
        }
        let now = Instant::now();
        // A governance change (kill list, policy row, acceptance) re-judges now; the timer stays the fallback.
        let governance_moved = self.resolver.view().changed_since(&mut self.governance_seen);
        if !governance_moved && self.next_tick.is_some_and(|at| now < at) {
            return;
        }
        self.next_tick = Some(now + REJUDGE_EVERY);
        if bundle_open {
            signals.write(AdmissionSignal::ListAccepted);
        }
        let arrived: Vec<String> = self
            .awaiting
            .iter()
            .filter(|(_, blob)| self.blobs.has_blob(blob))
            .map(|(node_id, _)| node_id.clone())
            .collect();
        for node_id in arrived {
            self.awaiting.remove(&node_id);
            changes.write(PortalBundleChanged { node_id: node_id.clone() });
            self.queue_summary(node_id);
        }
        // An open portal waiting for bytes no summary read has seen yet (opened without the section).
        if let (Some(node_id), Some(BundleState::WaitingForBytes)) = (ui.portal.bundle_node_id(), ui.portal.bundle_state()) {
            if !self.awaiting.contains_key(node_id) && !self.reading.contains(node_id) {
                self.queue_summary(node_id.to_owned());
            }
        }
    }
}

/// Update, in `AdmissionWriters` (before the revoke listener): requests out, the dialog's answer, outcomes and
/// signals in. Idle frames do one channel poll each and allocate nothing.
fn drive_portal_host(
    mut host: ResMut<PortalHost>,
    mut dialog: ResMut<PortalConsent>,
    mut ui: ResMut<UiManager>,
    tree: Res<VerseManager>,
    clock: Res<NoticeClock>,
    mut signals: MessageWriter<AdmissionSignal>,
    mut changes: MessageWriter<PortalBundleChanged>,
) {
    let host = &mut *host;
    let now_secs = clock.elapsed_secs_f64();
    // Polling must not mark the UI changed every frame; the drains below only touch fe-ui's queues and caches.
    let ui = ui.bypass_change_detection();
    host.queue_requests(ui, &tree);
    host.route_decision(&mut dialog.0, ui, now_secs);
    host.apply_outcomes(&mut dialog.0, ui, &mut signals, &mut changes, now_secs);
    host.follow_scene_changes(ui, &mut changes);
    host.tick(ui, &mut signals, &mut changes);
}

/// `UiManager::portal_bundle_nodes` from one summary read (R-UI3-09): a node whose bundle this host admitted is where
/// the inspector's bound `portal.open` offer opens; any other answer for the node drops it.
fn feed_bundle_node(
    nodes: &mut Vec<(String, String)>,
    node_id: &str,
    summary: &fe_ui::portal::repository_view::BundleSummary,
) {
    nodes.retain(|(_, known)| known != node_id);
    let admitted = summary.admission.as_ref().is_some_and(|(state, _)| state == "admitted");
    if admitted && nodes.len() < MAX_TRACKED_NODES {
        nodes.push((summary.package_id.clone(), node_id.to_owned()));
    }
}

/// The bound portal `follow_binding_changes` last looked at: the view generation, then the open anchor's
/// `(petal_id, package_id)` and its `portal.open` points.
#[derive(Default)]
struct BoundPortalSeen {
    generation: u64,
    anchor: Option<(String, String, Vec<fe_api::bindings::EffectivePoint>)>,
}

/// Update, in `AdmissionWriters`: an open bound portal reloads only when its petal's `portal.open` resolution moved
/// (`BundleHost::refresh_target`); other republishes of the binding view leave the page alone. Idle frames compare
/// one counter and two strings.
fn follow_binding_changes(
    bindings: Option<Res<crate::plugin_bridge::ExtensionBindings>>,
    ui: Res<UiManager>,
    mut seen: Local<BoundPortalSeen>,
    mut changes: MessageWriter<PortalBindingChanged>,
) {
    let Some(bindings) = bindings else { return };
    let generation = bindings.generation();
    // `binding:<petal>:<package>`: a package id never holds ':', a petal id may.
    let open = ui.portal.bundle_node_id().and_then(|anchor| anchor.strip_prefix("binding:")?.rsplit_once(':'));
    let Some((petal_id, package_id)) = open else {
        seen.anchor = None;
        seen.generation = generation;
        return;
    };
    let same_anchor = seen.anchor.as_ref().is_some_and(|(petal, package, _)| petal == petal_id && package == package_id);
    if same_anchor && generation == seen.generation {
        return;
    }
    seen.generation = generation;
    let points: Vec<_> =
        bindings.effective_at(petal_id).into_iter().filter(|point| point.hook == PORTAL_OPEN_HOOK).collect();
    // A newly opened anchor only records its baseline; a reload needs a moved resolution for the same anchor.
    let moved = same_anchor && seen.anchor.as_ref().is_some_and(|(_, _, before)| *before != points);
    seen.anchor = Some((petal_id.to_owned(), package_id.to_owned(), points));
    if moved {
        changes.write(PortalBindingChanged { petal_id: petal_id.to_owned(), package_id: package_id.to_owned() });
    }
}

/// The binding hook a petal's portal surface resolves through (bindings.md §6).
const PORTAL_OPEN_HOOK: &str = "portal.open";

/// The portal consent dialog, drawn only while an Admit awaits an answer.
fn render_portal_consent(mut contexts: EguiContexts, mut dialog: ResMut<PortalConsent>) {
    if dialog.0.pending().is_none() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    consent::render(ctx, &mut dialog.0);
}
