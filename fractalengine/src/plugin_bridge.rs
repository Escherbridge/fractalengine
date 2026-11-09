//! Production extension host: scene DB backend, integration bus, plugin manager and ticks, network broker,
//! governance, ops drain and scoped bindings; rationale in src/AGENTS.md §plugin-bridge.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use bevy::prelude::{App, Local, MessageReader, Res, ResMut, Resource, Time, Update};
use crossbeam::channel::{Receiver, RecvTimeoutError, Sender};
use fe_api::bindings::{BindingHost, BindingListReply, BindingWriteReply, EffectivePoint};
use fe_api::extensions::{
    AdminOp, AdminRefusal, AdminReply, AdminVerb, ApiCaller, ApiCallerKind, CommandFailure,
    CommandReply, ExtensionHost, HostRefusal, IngressAnswer, IngressCall, IngressRefusal,
    WsAcceptCall, WsData, WsHostSession, WsOut, MAX_AUDIT_ROWS,
};
use fe_database::handlers::device_payload::{set_accepted_payload_sink, AcceptedPayloadEvent};
use fe_database::handlers::extension_binding::BINDING_TABLE;
use fe_database::handlers::extension_policy::{
    AuditAction as PolicyAction, AuditEntry as PolicyEntry, AuditOutcome as PolicyOutcome,
    POLICY_TABLE,
};
use fe_database::RoleLevel;
use fe_hexon::package::{verify_plugin_package, PluginPackage, PluginPackageStore, PluginTrust};
use fe_hexon::registry_source::SourceTrust;
use fe_plugin::bindings::activation::{
    ActivationPlan, BindingActivator, BindingLauncher, CallTarget, ConfigurePlan, InstancePlan,
    Need, Trigger,
};
use fe_plugin::bindings::decode::{
    decode_payload, DecodeCall, DecodeCallError, DecodeFile, DecodeHost, DecodeJob, DecodeOutcome,
    DecodeReply, DecodeStamp, DECODE_HOOK, MAX_DECODE_BODY_BYTES,
};
use fe_plugin::bindings::resolver::{
    effective_binding, BindingSnapshot, BuildFacts, BuildIntegrity, LocalFacts, PackagePolicy,
    PetalAddress, PolicyVerdict, ResolvedPoint, StoredRow,
};
use fe_plugin::bus::{
    role_level, AuditAction, AuditEntry, AuditSink, Bus, BusPolicy, BusRefusal, BusSources, Caller,
    CallerKind, ClientGrants, CommandError, CommandOutcome, HostError, InstanceKey, InstanceSpec,
    PreparedQuery, QueryRequest, QueryRows, QuerySource, ReadingEvent, RoleSource,
    SceneSnapshotSource,
};
use fe_plugin::capability::{CapabilityToken, V2Binding, V2Grants};
use fe_plugin::lifecycle::{
    binding_instance_id, ConsentRecord, Governance, GovernanceGates, InstanceOrigin, PackageCheck,
    PackageFacts, PluginManager, RunningInstance, StagedInstance, TrustTier,
};
use fe_plugin::net::{
    HttpIngressRequest, IngressRefusal as NetRefusal, IngressReply, Net, WsAcceptRequest, WsFrame,
    WsOutbound,
};
use fe_plugin::package::UnsignedPolicy;
use fe_plugin::registry::PluginRegistry;
use fe_plugin::scene_host::{PluginContexts, SceneBackendFactory};
use fe_plugin::tick::{
    ContextTickSink, InstanceSeat, Launch, NetSeat, SharedWindow, TickHost, TickScheduler,
};
use fe_plugin::transaction::{StagedCommitSink, StagedFlush};
use fe_plugin::wasm::component::hooks::{HookCallError, HookRefusal, HookReply, HookRequest};
use fe_plugin::wasm::component::net_bridge::{HostBroker, NetBroker};
use fe_plugin::wasm::component::{
    window_for_token, Binding, BusBroker, ComponentHost, InstanceHandle, InstanceSetup, KvSlot,
    MemoryKv,
};
use fe_plugin::wasm::WasmEngine;
use fe_plugin::{
    NodeQuery, NodeReader, NodeSnapshot, PluginCommand, TransactionReceipt, TransactionSink,
};
use fe_runtime::extension_binding::{
    resolution_scopes, BindingMode, BindingPoint, BindingRecord, BindingResolution, BindingScopeLevel,
};
use fe_runtime::messages::{
    BindingRefusal, DbCommand, DbResult, ReplicatedRowOutcome, StoredBinding, VerseBindings,
};
use fe_runtime::plugin_host::{
    PackageIdentity, PluginAdmission, PluginRead, PluginRefusal, PluginReply, PluginResult,
    ReadingsAdmission,
};
use fe_runtime::scene_hook_gate::{
    AugmentOp, SceneBinding, SceneHook, SceneHookBackend, SceneHookCall, SceneHookReply,
    SceneHookReplySender,
};
use fe_runtime::service_lifecycle::{OwnedService, StopSignal};
use fe_sdk::manifest_v2::{parse_and_validate, ExtensionManifest, Ingress, RoleFloor, SceneWrite};
use fe_sdk::plugin_host::{PendingOp, PluginLease, StagedReading};
use fe_sdk::source_class::SourceClass;
use fe_terrain::extension::TERRAIN_EXTENSION_ID;
use fe_ui::extensions::bindings::{BindingPointRow, BindingsSnapshot, ScopeOption, ScopeTarget};
use fe_ui::extensions::panel::{AuditLine, ExtensionEvent, ExtensionsPanelState};
use fe_ui::extensions::{BoundPanel, BoundSlotsState, ConsentState, SnippetRequest, SnippetTarget};
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::verse_manager::VerseManager;
use serde_json::{json, Value};

use crate::extension_host::governance::{
    self, ExtensionGovernance, GovernanceConfig, ManagerControl,
};
use crate::extension_host::ops_drain::{
    ActiveScope, BuildToRun, ExtensionOpsPlugin, OpsPorts, OpsWorker,
};
use crate::extension_host::sources::{load_source_configs, SourceSet};
use crate::extension_host::{net_sources, revocation_pull};

/// Bound on enqueueing one DB command from a host thread.
const DB_SEND_TIMEOUT: Duration = Duration::from_secs(1);
/// Bound on one governance or binding DB round trip.
const DB_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on a plugin read or commit; a write past it is an unknown outcome.
const PLUGIN_DB_REPLY: Duration = Duration::from_secs(15);
/// Longest the binding worker sleeps between turns.
const WORKER_TURN: Duration = Duration::from_millis(500);
/// Spacing of the lazy-instance idle sweep (bindings.md §5.6).
const IDLE_SWEEP: Duration = Duration::from_secs(30);
/// Spacing of the installed-package store scan.
const CATALOG_SCAN: Duration = Duration::from_secs(2);
/// Wait before re-verifying a stored build that failed (it may have been mid-write).
const REFUSAL_RETRY: Duration = Duration::from_secs(30);
/// Spacing of the petal directory mirror.
const PETAL_MIRROR: Duration = Duration::from_secs(1);
/// How long one verse-owner answer is reused.
const OWNER_CACHE: Duration = Duration::from_secs(60);
/// Bound on one verse-owner lookup (the caller may hold the manager lock).
const OWNER_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
/// Wait before re-reading a verse whose binding snapshot failed to load.
const SNAPSHOT_RETRY: Duration = Duration::from_secs(5);
/// Accepted payloads queued for decode before new ones are dropped.
const MAX_PAYLOAD_BACKLOG: usize = 256;
/// Audit entries one acceptance hydration reads.
const ACCEPTANCE_SCAN_LIMIT: usize = 10_000;
/// Audit lines the details view shows.
const DETAILS_AUDIT_LINES: usize = 10;
/// Sealed builds in the extension data dir.
const PACKAGES_DIR: &str = "packages";
/// The 1.0 core-module AOT cache in the extension data dir.
const AOT_DIR: &str = "aot";
/// Configured registry sources in the extension data dir (F-OPS `SourceConfig` array).
const SOURCES_FILE: &str = "extension_sources.json";
/// Replicated role statements (fe-database `role` table).
const ROLE_TABLE: &str = "role";
/// Catalogue ids the bound-slot cache reads.
const PANEL_HOOK: &str = "ui.panel";
const SNIPPET_HOOK: &str = "egress.snippet";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// RFC 3339 UTC with milliseconds; civil date by Howard Hinnant's `civil_from_days`
/// (<http://howardhinnant.github.io/date_algorithms.html#civil_from_days>).
fn rfc3339_utc(epoch_ms: u64) -> String {
    let secs = epoch_ms / 1_000;
    let (hour, minute, second) = ((secs % 86_400) / 3_600, (secs % 3_600) / 60, secs % 60);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        epoch_ms % 1_000
    )
}

/// `role` capped by a token's `max_role` (a delegated token of this node stays at its ceiling, decision #8).
fn capped(role: RoleLevel, ceiling: RoleLevel) -> RoleLevel {
    if role.is_at_least(ceiling) {
        ceiling
    } else {
        role
    }
}

// ---------------------------------------------------------------------------
// Scene DB backend: reads, 1.0 commits and tick flushes over the typed plugin commands
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DbSceneBackend {
    sender: Sender<DbCommand>,
    admission: PluginAdmission,
}

/// A refusal as the sinks' callers read it: `UnknownOutcome` must classify as unknown in `ContextTickSink`.
fn refusal_text(refusal: PluginRefusal) -> String {
    match refusal {
        PluginRefusal::UnknownOutcome(detail) => {
            format!("unknown outcome, requires reconciliation: {detail}")
        }
        other => other.to_string(),
    }
}

impl DbSceneBackend {
    /// One read, or one atomic commit of `ops` plus `readings` (pinned `DbCommand::PluginCommit`).
    fn request(
        &self,
        read: Option<PluginRead>,
        ops: &[PendingOp],
        readings: &[StagedReading],
    ) -> Result<PluginReply, String> {
        self.admission.require_active()?;
        let (reply, receiver) = crossbeam::channel::bounded::<PluginResult>(1);
        let is_write = read.is_none();
        let command = match read {
            Some(request) => DbCommand::PluginRead {
                admission: self.admission.clone(),
                request,
                reply,
            },
            None => DbCommand::PluginCommit {
                admission: self.admission.clone(),
                ops: ops.to_vec(),
                readings: readings.to_vec(),
                reply,
            },
        };
        self.sender
            .send_timeout(command, DB_SEND_TIMEOUT)
            .map_err(|_| "plugin DB command was not enqueued".to_string())?;
        let answer = receiver.recv_timeout(PLUGIN_DB_REPLY).map_err(|_| {
            if is_write {
                "plugin commit has an unknown outcome; requires reconciliation before retrying"
                    .to_string()
            } else {
                "plugin DB read reply timed out or disconnected".to_string()
            }
        })?;
        let response = answer.map_err(refusal_text)?;
        if !is_write {
            self.admission.require_active()?;
        }
        Ok(response)
    }

    fn require_petal(&self, petal_id: &str) -> Result<(), String> {
        if petal_id != self.admission.petal_id() {
            return Err("plugin backend is bound to another petal".into());
        }
        Ok(())
    }
}

impl NodeReader for DbSceneBackend {
    fn petal_scope(&self, petal_id: &str) -> Result<String, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::PetalScope), &[], &[])? {
            PluginReply::Scope(scope) => Ok(scope),
            _ => Err("unrelated plugin scope reply".into()),
        }
    }
    fn get_node(&self, petal_id: &str, node_id: &str) -> Result<Option<NodeSnapshot>, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::Node(node_id.into())), &[], &[])? {
            PluginReply::Node(node) => Ok(node),
            _ => Err("unrelated plugin node reply".into()),
        }
    }
    fn query_nodes(&self, petal_id: &str, query: &NodeQuery) -> Result<Vec<NodeSnapshot>, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::Nodes(query.clone())), &[], &[])? {
            PluginReply::Nodes(nodes) => Ok(nodes),
            _ => Err("unrelated plugin query reply".into()),
        }
    }
}

impl TransactionSink for DbSceneBackend {
    fn commit(&mut self, petal_id: &str, ops: &[PendingOp]) -> Result<TransactionReceipt, String> {
        self.require_petal(petal_id)?;
        match self.request(None, ops, &[])? {
            PluginReply::Committed(receipt) => Ok(receipt),
            _ => Err("unrelated plugin commit reply; outcome requires reconciliation".into()),
        }
    }
}

impl StagedCommitSink for DbSceneBackend {
    fn commit_staged(
        &mut self,
        petal_id: &str,
        flush: &StagedFlush,
    ) -> Result<TransactionReceipt, String> {
        self.require_petal(petal_id)?;
        match self.request(None, &flush.ops, &flush.readings)? {
            PluginReply::Committed(receipt) if receipt.reading_count == flush.readings.len() => Ok(receipt),
            PluginReply::Committed(_) => {
                Err("plugin flush landed another reading count; unknown outcome, requires reconciliation".into())
            }
            _ => Err("unrelated plugin commit reply; outcome requires reconciliation".into()),
        }
    }
}

/// Context id -> the sealed build the host verified and staged under it; read when the factory mints an admission.
#[derive(Clone, Default)]
struct VerifiedBuilds(Arc<RwLock<HashMap<String, PackageIdentity>>>);

impl VerifiedBuilds {
    fn record(&self, plugin_id: &str, identity: PackageIdentity) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(plugin_id.to_string(), identity);
    }

    fn get(&self, plugin_id: &str) -> Option<PackageIdentity> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(plugin_id)
            .cloned()
    }
}

/// A v2 token admits only a build this host verified: stamp `{package_id, content_hash}`, narrow the write kinds and
/// bind the readings grant (G6-C6). No verified build fails closed.
fn sealed_admission(
    admission: PluginAdmission,
    plugin_id: &str,
    grants: &V2Grants,
    verified: &VerifiedBuilds,
) -> Result<PluginAdmission, String> {
    let identity = verified
        .get(plugin_id)
        .ok_or_else(|| format!("{plugin_id}: no verified build is staged under this instance"))?;
    if identity.package_id() != grants.package_id {
        return Err(format!(
            "{plugin_id}: the verified build is another package"
        ));
    }
    let mut admission = admission
        .with_package(identity)
        .with_scene_writes(grants.scene_write.clone())?;
    if let Some(class) = grants.readings_class {
        admission = admission.with_readings(ReadingsAdmission::new(
            SourceClass::from(class),
            grants.metrics.clone(),
        )?);
    }
    Ok(admission)
}

struct DbSceneFactory {
    sender: Sender<DbCommand>,
    verified: VerifiedBuilds,
}

impl SceneBackendFactory for DbSceneFactory {
    fn bind(
        &self,
        context: fe_plugin::context::PluginContext,
    ) -> Result<fe_plugin::context::PluginContext, String> {
        let token = context.capabilities();
        let mut admission = PluginAdmission::new(
            context.plugin_id().into(),
            context.petal_id().into(),
            token.capabilities.clone(),
            token.property_scope.clone(),
            token.verse_scope.clone(),
            token.petal_scope.clone(),
        )?
        .with_lease(context.revocation_lease());
        if let Some(grants) = &token.grants_v2 {
            admission = sealed_admission(admission, context.plugin_id(), grants, &self.verified)?;
        }
        let mut backend = DbSceneBackend {
            sender: self.sender.clone(),
            admission,
        };
        let scope = backend.petal_scope(context.petal_id())?;
        backend.admission = backend.admission.bind_scope(scope)?;
        let backend = Arc::new(Mutex::new(backend));
        Ok(context
            .with_node_reader(backend.clone())
            .with_transaction_sink(backend.clone())
            .with_staged_sink(backend))
    }
}

// ---------------------------------------------------------------------------
// Bus inputs (roles, external policy, query and snapshot sources, feeds)
// ---------------------------------------------------------------------------

/// Inputs `main.rs` supplies to the integration bus; every field is optional and the default fails closed.
#[derive(Default)]
pub(crate) struct ExtensionBridgeConfig {
    /// The host's local DID (the identity every extension instance writes as).
    pub local_did: String,
    /// The API command channel the bus query and scene sources use.
    pub api_cmd_tx: Option<Sender<fe_runtime::messages::ApiCommand>>,
    /// The committed-scene change broadcast (`entity_change_tx`) feeding `host/node-changes`.
    pub scene_changes: Option<tokio::sync::broadcast::Receiver<fe_runtime::messages::SceneChange>>,
}

/// Late-bound API command channel; the bus sources answer `not-available` until it is set.
#[derive(Resource, Clone, Default)]
pub(crate) struct ExtensionApiSlot(Arc<OnceLock<Sender<fe_runtime::messages::ApiCommand>>>);

impl ExtensionApiSlot {
    /// Bind once; later calls are ignored.
    pub(crate) fn bind(&self, sender: Sender<fe_runtime::messages::ApiCommand>) {
        let _ = self.0.set(sender);
    }

    fn get(&self) -> Result<&Sender<fe_runtime::messages::ApiCommand>, HostError> {
        self.0
            .get()
            .ok_or_else(|| HostError::NotAvailable("the database bridge is not bound yet".into()))
    }
}

/// Late-bound blob store the decode path reads accepted payload files from.
#[derive(Resource, Clone, Default)]
pub(crate) struct ExtensionBlobSlot(Arc<OnceLock<fe_database::BlobStoreHandle>>);

impl ExtensionBlobSlot {
    /// Bind once; later calls are ignored.
    pub(crate) fn bind(&self, store: fe_database::BlobStoreHandle) {
        let _ = self.0.set(store);
    }
}

/// Send readings the readings door accepted; a drain thread feeds `host/readings`.
#[derive(Resource, Clone)]
pub(crate) struct ReadingFeed(pub Sender<ReadingEvent>);

/// Role view the bus judges callers by: the local DID is Owner, every other DID needs a published grant.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionRoleView(Arc<ExtensionRoles>);

impl ExtensionRoleView {
    /// Replace one DID's role at a petal (`RoleLevel::None` removes it).
    pub(crate) fn set_role(&self, did: &str, petal_id: &str, role: RoleLevel) {
        let mut grants = self
            .0
            .grants
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let key = (did.to_string(), petal_id.to_string());
        if role == RoleLevel::None {
            grants.remove(&key);
        } else {
            grants.insert(key, role);
        }
    }
}

/// Verses whose policy row opens `external: true` streams and commands to remote DIDs.
#[derive(Resource, Clone, Default)]
pub(crate) struct ExtensionExternalPolicy(Arc<RwLock<std::collections::HashSet<String>>>);

impl ExtensionExternalPolicy {
    /// Open or close external bus access for one verse.
    pub(crate) fn set_verse(&self, verse_id: &str, allowed: bool) {
        let mut verses = self.0.write().unwrap_or_else(|error| error.into_inner());
        if allowed {
            verses.insert(verse_id.to_string());
        } else {
            verses.remove(verse_id);
        }
    }
}

impl BusPolicy for ExtensionExternalPolicy {
    fn external_access_allowed(&self, verse_id: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .contains(verse_id)
    }
}

struct ExtensionRoles {
    local_did: String,
    grants: RwLock<HashMap<(String, String), RoleLevel>>,
}

impl RoleSource for ExtensionRoles {
    fn role_at(&self, did: &str, petal_id: &str) -> RoleLevel {
        if did == self.local_did {
            return RoleLevel::Owner;
        }
        let grants = self
            .grants
            .read()
            .unwrap_or_else(|error| error.into_inner());
        grants
            .get(&(did.to_string(), petal_id.to_string()))
            .copied()
            .unwrap_or(RoleLevel::None)
    }
}

/// Bus DB round trips allowed at once across query and snapshot sources.
const MAX_BUS_DB_IN_FLIGHT: usize = 8;
/// Deadline for a bus DB round trip (statement timeout plus slack, as REST).
const BUS_DB_TIMEOUT: Duration =
    Duration::from_secs(fe_api::query_guard::STATEMENT_TIMEOUT_SECS + 1);
/// Largest `host/query` result in bytes (REST's cap).
const QUERY_RESULT_BYTE_CAP: usize = 8 * 1024 * 1024;

/// Releases one in-flight slot on drop.
struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    fn acquire(counter: &Arc<AtomicUsize>) -> Option<Self> {
        if counter.fetch_add(1, Ordering::AcqRel) >= MAX_BUS_DB_IN_FLIGHT {
            counter.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self(counter.clone()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One correlated RawQuery over the API channel, polled until `timeout`.
fn db_rows_within(
    api: &Sender<fe_runtime::messages::ApiCommand>,
    sql: String,
    vars: HashMap<String, Value>,
    timeout: Duration,
) -> Result<Vec<Value>, HostError> {
    use fe_runtime::messages::ApiCommand;
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    api.send(ApiCommand::DbRequest {
        cmd: DbCommand::RawQuery {
            sql,
            vars,
            correlation_id: Some(correlation_id.clone()),
        },
        reply_tx,
    })
    .map_err(|_| HostError::NotAvailable("the database bridge is closed".into()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match reply_rx.try_recv() {
            Ok(DbResult::QueryResult {
                data,
                correlation_id: Some(echoed),
            }) if echoed == correlation_id => return Ok(data),
            Ok(DbResult::QueryFailed {
                error,
                correlation_id: echoed,
            }) if echoed == correlation_id => {
                return Err(HostError::Backend(format!("query failed: {error}")))
            }
            Ok(DbResult::Error(error)) => {
                return Err(HostError::Backend(format!("query failed: {error}")))
            }
            Ok(_) => {
                return Err(HostError::Backend(
                    "query failed: uncorrelated reply".into(),
                ))
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err(HostError::Backend(
                    "the database bridge dropped the request".into(),
                ))
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(HostError::UnknownOutcome("query timed out".into()));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// [`db_rows_within`] at the bus deadline.
fn db_rows(
    api: &Sender<fe_runtime::messages::ApiCommand>,
    sql: String,
    vars: HashMap<String, Value>,
) -> Result<Vec<Value>, HostError> {
    db_rows_within(api, sql, vars, BUS_DB_TIMEOUT)
}

/// `host/query` over fe-api's guard core (`prepare_scoped_sql`) and the API DB channel.
struct GuardedQuerySource {
    api: ExtensionApiSlot,
    in_flight: Arc<AtomicUsize>,
    rate: Mutex<HashMap<String, (u32, Instant)>>,
}

impl QuerySource for GuardedQuerySource {
    fn prepare(&self, request: &QueryRequest) -> Result<PreparedQuery, HostError> {
        // An extension query's scope is its own petal, so no scope lookup is needed.
        let prepared = fe_api::query_guard::prepare_scoped_sql(
            &request.sql,
            std::slice::from_ref(&request.petal_id),
            fe_api::query_guard::GuardMode::Query,
        )
        .map_err(HostError::InvalidInput)?;
        Ok(PreparedQuery {
            tables: prepared.tables,
            guarded_sql: prepared.sql,
        })
    }

    fn run(
        &self,
        request: &QueryRequest,
        prepared: &PreparedQuery,
        reply: Sender<Result<QueryRows, HostError>>,
    ) {
        let api = match self.api.get() {
            Ok(api) => api.clone(),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let Some(slot) = InFlight::acquire(&self.in_flight) else {
            let _ = reply.send(Err(HostError::RateLimited(
                "too many bus queries in flight".into(),
            )));
            return;
        };
        let sql = fe_api::query_guard::with_statement_timeout(&prepared.guarded_sql);
        let vars: HashMap<String, Value> = request.vars.clone().into_iter().collect();
        let spawned = std::thread::Builder::new()
            .name("fe-bus-query".into())
            .spawn(move || {
                let _slot = slot;
                let outcome = db_rows(&api, sql, vars).map(cap_rows);
                let _ = reply.send(outcome);
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "bus query thread did not start");
        }
    }

    fn try_acquire_rate(&self, identity_did: &str) -> bool {
        // Same sliding 1 s window as the REST query limiter (fe_api::limits::QUERY_RATE_PER_SEC).
        let now = Instant::now();
        let mut windows = self.rate.lock().unwrap_or_else(|error| error.into_inner());
        windows.retain(|_, (_, started)| now.duration_since(*started) < Duration::from_secs(10));
        let entry = windows.entry(identity_did.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) > Duration::from_secs(1) {
            *entry = (1, now);
            return true;
        }
        entry.0 += 1;
        entry.0 <= fe_api::limits::QUERY_RATE_PER_SEC
    }
}

/// Apply REST's 10 000-row and 8 MiB caps, marking the result truncated instead of failing.
fn cap_rows(rows: Vec<Value>) -> QueryRows {
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    for row in rows {
        bytes += row.to_string().len();
        if kept.len() >= fe_api::limits::QUERY_ROW_CAP || bytes > QUERY_RESULT_BYTE_CAP {
            truncated = true;
            break;
        }
        kept.push(row);
    }
    QueryRows {
        rows: kept,
        truncated,
    }
}

/// `host/node-changes` snapshots: the petal's node rows through the same API DB channel.
struct DbSceneSnapshots {
    api: ExtensionApiSlot,
    in_flight: Arc<AtomicUsize>,
}

impl SceneSnapshotSource for DbSceneSnapshots {
    fn snapshot(&self, petal_id: &str, reply: Sender<Result<Vec<Value>, HostError>>) {
        let api = match self.api.get() {
            Ok(api) => api.clone(),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let Some(slot) = InFlight::acquire(&self.in_flight) else {
            let _ = reply.send(Err(HostError::RateLimited(
                "too many bus snapshots in flight".into(),
            )));
            return;
        };
        let mut vars = HashMap::new();
        vars.insert("pid".to_string(), json!(petal_id));
        // Capped at read time to REST's row cap, so a huge petal is never loaded whole (F-BUS note).
        let sql = fe_api::query_guard::with_statement_timeout(&format!(
            "SELECT * FROM node WHERE petal_id = $pid LIMIT {}",
            fe_api::limits::QUERY_ROW_CAP
        ));
        let spawned = std::thread::Builder::new()
            .name("fe-bus-snapshot".into())
            .spawn(move || {
                let _slot = slot;
                let _ = reply.send(db_rows(&api, sql, vars));
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "bus snapshot thread did not start");
        }
    }
}

/// Forward committed scene changes into the bus; a lagged receiver resyncs every subscription.
fn spawn_scene_feed(
    bus: Bus,
    mut changes: tokio::sync::broadcast::Receiver<fe_runtime::messages::SceneChange>,
) {
    use tokio::sync::broadcast::error::RecvError;
    let spawned = std::thread::Builder::new()
        .name("fe-bus-scene-feed".into())
        .spawn(move || loop {
            match changes.blocking_recv() {
                Ok(change) => bus.feed_node_changes(std::slice::from_ref(&change)),
                Err(RecvError::Lagged(_)) => bus.feed_node_changes_lagged(None),
                Err(RecvError::Closed) => break,
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "bus scene feed did not start");
    }
}

/// Drain accepted readings into `host/readings` in batches.
fn spawn_reading_feed(bus: Bus, readings: Receiver<ReadingEvent>) {
    let spawned = std::thread::Builder::new()
        .name("fe-bus-reading-feed".into())
        .spawn(move || {
            while let Ok(first) = readings.recv() {
                let mut batch = vec![first];
                batch.extend(
                    readings
                        .try_iter()
                        .take(fe_plugin::bus::READINGS_PER_EVENT - 1),
                );
                bus.feed_readings(&batch);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "bus reading feed did not start");
    }
}

// ---------------------------------------------------------------------------
// Shared host pieces
// ---------------------------------------------------------------------------

/// The bus with the role source and audit sink it was built with; the portal broker shares all three (P-BROKER).
#[derive(Resource, Clone)]
pub(crate) struct ExtensionBusParts {
    pub bus: Bus,
    pub roles: Arc<dyn RoleSource>,
    pub audit: Arc<dyn AuditSink>,
}

/// The runtime the network broker's dial-outs and egress run on; dropped with the world, never inside a task.
#[derive(Resource)]
struct ExtensionNetRuntime(#[allow(dead_code)] tokio::runtime::Runtime);

/// The binding worker thread, stopped with the other host services on exit.
#[derive(Resource)]
pub(crate) struct ExtensionBindingService(pub Mutex<Option<OwnedService>>);

/// Petal id -> its `(verse, fractal, petal)` address, mirrored from the loaded hierarchy.
#[derive(Clone, Default)]
struct PetalDirectory(Arc<RwLock<HashMap<String, PetalAddress>>>);

impl PetalDirectory {
    fn get(&self, petal_id: &str) -> Option<PetalAddress> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(petal_id)
            .cloned()
    }

    fn contains(&self, petal_id: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(petal_id)
    }

    fn of_verse(&self, verse_id: &str) -> Vec<PetalAddress> {
        let mut petals: Vec<PetalAddress> = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|petal| petal.verse_id == verse_id)
            .cloned()
            .collect();
        petals.sort_by(|left, right| left.petal_id.cmp(&right.petal_id));
        petals
    }

    /// Replace the map; the verses whose petal set changed.
    fn replace(&self, next: HashMap<String, PetalAddress>) -> BTreeSet<String> {
        let mut current = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if *current == next {
            return BTreeSet::new();
        }
        let mut changed: BTreeSet<String> = current
            .iter()
            .filter(|(petal_id, address)| next.get(*petal_id) != Some(*address))
            .map(|(_, address)| address.verse_id.clone())
            .collect();
        changed.extend(
            next.iter()
                .filter(|(petal_id, address)| current.get(*petal_id) != Some(*address))
                .map(|(_, address)| address.verse_id.clone()),
        );
        *current = next;
        changed
    }
}

/// One staged instance: its guest, window, key, manifest and lease (live while staged or admitted).
struct LiveEntry {
    handle: InstanceHandle,
    /// The window its scheduler flushes; scene augments stage here without locking the guest.
    window: SharedWindow,
    key: InstanceKey,
    manifest: Arc<ExtensionManifest>,
    lease: PluginLease,
    staged_at: Instant,
}

/// Why a host-initiated hook call produced no reply.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HookFailure {
    /// The guest refused (`hook-error::refused`): a `scene.*` refusal blocks the write.
    Refused(String),
    /// The guest rejected the input or failed: the built-in serves.
    Declined(String),
    /// Not running, a trap, a timeout or bad output.
    Fault(String),
}

/// Whether a `scene.property_scope` admits `key` (`*`, `prefix*` or exact; an empty scope admits nothing).
fn property_allowed(scope: &[String], key: &str) -> bool {
    scope.iter().any(|pattern| match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => pattern == key,
    })
}

/// Instances this host staged, by context id; the serving build is the earliest one whose lease still holds, so a
/// rolled-back update keeps serving from the previous build and a retired one is never called.
#[derive(Clone, Default)]
struct LiveInstances(Arc<Mutex<HashMap<String, Vec<LiveEntry>>>>);

impl LiveInstances {
    fn insert(&self, plugin_id: &str, entry: LiveEntry) {
        lock(&self.0)
            .entry(plugin_id.to_string())
            .or_default()
            .push(entry);
    }

    fn serving(entries: &[LiveEntry]) -> Option<&LiveEntry> {
        entries
            .iter()
            .filter(|entry| entry.lease.require_active().is_ok())
            .min_by_key(|entry| entry.staged_at)
    }

    fn handle(&self, plugin_id: &str) -> Option<InstanceHandle> {
        let entries = lock(&self.0);
        Self::serving(entries.get(plugin_id)?).map(|entry| entry.handle.clone())
    }

    /// The serving manifest at `key` (ingress role floors).
    fn manifest_at(&self, key: &InstanceKey) -> Option<Arc<ExtensionManifest>> {
        let entries = lock(&self.0);
        entries
            .values()
            .filter_map(|candidates| Self::serving(candidates))
            .filter(|entry| entry.key == *key)
            .min_by_key(|entry| entry.staged_at)
            .map(|entry| entry.manifest.clone())
    }

    /// Drop retired builds (revoked leases).
    fn prune(&self) {
        let mut entries = lock(&self.0);
        for candidates in entries.values_mut() {
            candidates.retain(|entry| entry.lease.require_active().is_ok());
        }
        entries.retain(|_, candidates| !candidates.is_empty());
    }

    /// One host-initiated hook call (`configure`, `ingest.decode`, `egress.snippet`, `petal.map`) on the serving build.
    fn call(
        &self,
        plugin_id: &str,
        request: &HookRequest,
        stamp: Option<&DecodeStamp>,
    ) -> Result<HookReply, HookFailure> {
        let handle = self
            .handle(plugin_id)
            .ok_or_else(|| HookFailure::Fault(format!("{plugin_id} is not running")))?;
        handle
            .call_hook(request, stamp)
            .map_err(|error| match error {
                HookCallError::Guest(HookRefusal::Refused(reason)) => HookFailure::Refused(reason),
                HookCallError::Guest(
                    HookRefusal::InvalidInput(reason) | HookRefusal::Failed(reason),
                ) => HookFailure::Declined(reason),
                other => HookFailure::Fault(other.to_string()),
            })
    }

    /// Stage an allowing `scene.*` guest's `set_property` augments in the serving instance's window, all or none;
    /// every key must sit inside the build's property scope (the flush re-authorizes each op against its token).
    fn stage_augment(
        &self,
        key: &InstanceKey,
        node_id: &str,
        ops: Vec<AugmentOp>,
    ) -> Result<(), String> {
        let (window, manifest) = {
            let entries = lock(&self.0);
            entries
                .values()
                .filter_map(|candidates| Self::serving(candidates))
                .filter(|entry| entry.key == *key)
                .min_by_key(|entry| entry.staged_at)
                .map(|entry| (entry.window.clone(), entry.manifest.clone()))
                .ok_or("no running instance serves this binding")?
        };
        let scene = manifest.scene.as_ref();
        let writes_properties = scene
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| kinds.contains(&SceneWrite::SetProperty));
        if !writes_properties || manifest.tick.is_none() {
            return Err("the bound build cannot stage property writes".into());
        }
        let scope = scene
            .and_then(|scene| scene.property_scope.clone())
            .unwrap_or_default();
        let pending = ops
            .into_iter()
            .map(|op| {
                if property_allowed(&scope, &op.key) {
                    Ok(PendingOp::SetProperty {
                        node_id: node_id.to_string(),
                        key: op.key,
                        value: op.value,
                    })
                } else {
                    Err(format!(
                        "property {} is outside the build's property scope",
                        op.key
                    ))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bytes: usize = pending
            .iter()
            .map(|op| serde_json::to_vec(op).map_or(usize::MAX, |encoded| encoded.len()))
            .fold(0, usize::saturating_add);
        let mut window = lock(&window);
        let info = window.info();
        if info.ops + pending.len() > info.max_ops
            || info.bytes.saturating_add(bytes) > info.max_bytes
        {
            return Err("the instance's staged window has no room for the augment".into());
        }
        for op in pending {
            window.stage_op(op).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// The floor an accept-mode ingress endpoint declares (default Editor); `None` when it is not declared.
fn endpoint_floor(
    manifest: &ExtensionManifest,
    endpoint: &str,
    websocket: bool,
) -> Option<RoleLevel> {
    manifest
        .ingress
        .iter()
        .find_map(|ingress| match (ingress, websocket) {
            (Ingress::HttpServer(declared), false) if declared.id == endpoint => {
                Some(declared.role_floor)
            }
            (Ingress::WebsocketAccept(declared), true) if declared.id == endpoint => {
                Some(declared.role_floor)
            }
            _ => None,
        })
        .map(|floor| role_level(floor.unwrap_or(RoleFloor::Editor)))
}

// ---------------------------------------------------------------------------
// Staging: verified build -> component instance, context, scheduler and seat
// ---------------------------------------------------------------------------

/// Everything one start or update needs to stage a 2.0 build; shared by the ops drain and the binding launcher.
#[derive(Clone)]
struct InstanceStager {
    ticks: TickHost,
    bus: Bus,
    net: Net,
    components: Option<Arc<ComponentHost>>,
    commands: Sender<PluginCommand>,
    local_did: String,
    verified: VerifiedBuilds,
    petals: PetalDirectory,
    live: LiveInstances,
}

/// One build to stage at one petal under one context id.
struct StageRequest<'a> {
    plugin_id: &'a str,
    installation_id: &'a str,
    package: &'a PluginPackage,
    facts: &'a PackageFacts,
    petal: &'a PetalAddress,
    first_activation: bool,
}

impl InstanceStager {
    /// Instantiate the verified component and stage its context (keyed `(petal, package)`), scheduler and seat;
    /// nothing runs until `TickHost::launch`/`update`. A failure after staging discards the context.
    fn stage(&self, request: &StageRequest<'_>) -> Result<StagedInstance, String> {
        let StageRequest {
            plugin_id,
            package,
            facts,
            petal,
            ..
        } = *request;
        let components = self
            .components
            .as_ref()
            .ok_or("the component host did not start on this host")?;
        let manifest = facts
            .manifest
            .clone()
            .ok_or("a web bundle has no instance")?;
        let judged = facts.component_digest.as_deref().unwrap_or_default();
        if !package.component_digest.eq_ignore_ascii_case(judged)
            || !package
                .content_hash
                .eq_ignore_ascii_case(&facts.content_hash)
        {
            return Err(format!(
                "{plugin_id}: the stored build is not the one governance judged"
            ));
        }
        let identity = PackageIdentity::new(
            facts.package_id.clone(),
            facts.content_hash.to_ascii_lowercase(),
        )?;
        let scopes = V2Binding {
            verse_scope: format!("VERSE#{}", petal.verse_id),
            petal_scope: fe_database::build_scope(
                &petal.verse_id,
                Some(petal.fractal_id.as_str()),
                Some(petal.petal_id.as_str()),
            ),
            // A manifest offering several classes needs the operator's pick; until the bind dialog records it, refused.
            source_class: None,
        };
        let token = CapabilityToken::mint_v2(plugin_id, &manifest, &scopes)?;
        let component = components
            .compile(&package.component)
            .map_err(|error| format!("{} ({error})", error.code()))?;
        if !component
            .digest()
            .eq_ignore_ascii_case(&package.component_digest)
        {
            return Err(format!(
                "{plugin_id}: the component digest differs from the sealed build"
            ));
        }
        // A manifest that `provides` hooks needs the `hooks` export (bindings.md §7.3; R-HOOKS-02).
        component
            .check_provides(&manifest)
            .map_err(|error| format!("{} ({error})", error.code()))?;
        // The factory reads this while staging below: the admission is stamped with exactly this build.
        self.verified.record(plugin_id, identity);
        let key = InstanceKey::new(petal.petal_id.clone(), facts.package_id.clone());
        let context = self.ticks.contexts().stage_instance(
            &key,
            plugin_id.to_string(),
            token,
            self.commands.clone(),
        )?;
        self.assemble(
            request,
            &manifest,
            components,
            &component,
            key,
            context.clone(),
        )
        .map_err(|error| {
            self.ticks.contexts().discard(&context);
            error
        })
    }

    fn assemble(
        &self,
        request: &StageRequest<'_>,
        manifest: &Arc<ExtensionManifest>,
        components: &ComponentHost,
        component: &fe_plugin::wasm::component::ExtensionComponent,
        key: InstanceKey,
        context: fe_plugin::context::PluginContext,
    ) -> Result<StagedInstance, String> {
        let StageRequest {
            plugin_id,
            installation_id,
            facts,
            petal,
            first_activation,
            ..
        } = *request;
        let clock = self.ticks.clock();
        let window = window_for_token(context.capabilities());
        let source_class = context
            .capabilities()
            .grants_v2
            .as_ref()
            .and_then(|grants| grants.readings_class)
            .and_then(|class| serde_json::to_value(class).ok())
            .and_then(|value| value.as_str().map(str::to_string));
        let binding = Binding {
            installation_id: installation_id.to_string(),
            package_id: facts.package_id.clone(),
            content_hash: facts.content_hash.clone(),
            verse_id: petal.verse_id.clone(),
            fractal_id: petal.fractal_id.clone(),
            petal_id: petal.petal_id.clone(),
            identity_did: self.local_did.clone(),
            source_class,
        };
        let broker = HostBroker::new(
            BusBroker::new(self.bus.clone()),
            NetBroker::new(self.net.clone()),
        );
        let mut setup = InstanceSetup::new(binding, context.clone(), window.clone(), clock.clone())
            .with_broker(Arc::new(broker));
        if let Some(kv) = &manifest.kv {
            setup = setup.with_kv(KvSlot::new(Box::new(MemoryKv::new()), kv.max_bytes));
        }
        let instance = components
            .instantiate(component, setup)
            .map_err(|error| format!("{} ({error})", error.code()))?;
        let handle = InstanceHandle::new(instance);
        let budget = context.capabilities().tick_budget().copied();
        let scheduler = TickScheduler::for_budget(
            budget,
            clock,
            window.clone(),
            Box::new(handle.clone()),
            Box::new(ContextTickSink::new(context.clone())),
        );
        let spec = InstanceSpec {
            installation_id: installation_id.to_string(),
            content_hash: facts.content_hash.clone(),
            verse_id: petal.verse_id.clone(),
            petal_id: petal.petal_id.clone(),
            identity_did: self.local_did.clone(),
            manifest: manifest.clone(),
        };
        let seat = InstanceSeat {
            bus: self.bus.clone(),
            spec,
            bus_hooks: Box::new(handle.clone()),
            net: Some(NetSeat {
                net: self.net.clone(),
                hooks: Box::new(handle.clone()),
            }),
        };
        self.live.insert(
            plugin_id,
            LiveEntry {
                handle: handle.clone(),
                key,
                manifest: manifest.clone(),
                window,
                lease: context.revocation_lease(),
                staged_at: Instant::now(),
            },
        );
        Ok(StagedInstance {
            plugin_id: plugin_id.to_string(),
            component_digest: component.digest().to_string(),
            launch: Launch {
                scheduler,
                seat: Some(seat),
                guest: Box::new(handle),
                context,
                first_activation,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Installed builds (the package store as R19 judges it)
// ---------------------------------------------------------------------------

/// One verified build in the package store.
#[derive(Clone)]
struct CatalogBuild {
    facts: PackageFacts,
    name: String,
    /// `None` when no loaded root vouches (row 4 would refuse it).
    tier: Option<TrustTier>,
}

/// The package store, re-scanned on a timer; builds are verified once per `(package_id, content_hash)`.
struct InstalledCatalog {
    store: PluginPackageStore,
    governance: ExtensionGovernance,
    trust_root: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    allow_dev: bool,
    builds: BTreeMap<String, CatalogBuild>,
    /// Every listed `(package_id, content_hash)` checked so far: its build, or when a refused one is re-tried
    /// (the ops drain may still be writing it).
    checked: HashMap<(String, String), Result<CatalogBuild, Instant>>,
    last_scan: Option<Instant>,
}

impl InstalledCatalog {
    fn new(dir: &Path, governance: ExtensionGovernance, allow_dev: bool) -> Self {
        Self {
            store: PluginPackageStore::new(dir.join(PACKAGES_DIR)),
            trust_root: governance.trust_root_predicate(),
            governance,
            allow_dev,
            builds: BTreeMap::new(),
            checked: HashMap::new(),
            last_scan: None,
        }
    }

    /// Re-verify sealed bytes end to end; the package the bytes name must be `package_id`.
    fn verify(
        &self,
        package_id: &str,
        content_hash: &str,
    ) -> Result<(PluginPackage, CatalogBuild), String> {
        let bytes = self
            .store
            .get(package_id, content_hash)
            .map_err(|error| format!("package store: {error}"))?
            .ok_or_else(|| format!("the stored build of {package_id} is missing"))?;
        let trust = PluginTrust {
            is_trust_root: &*self.trust_root,
            allow_unsigned_dev: self.allow_dev,
        };
        let package = verify_plugin_package(&bytes, Some(package_id), &trust)
            .map_err(|error| error.to_string())?;
        if !package.content_hash.eq_ignore_ascii_case(content_hash) {
            return Err(format!("the stored build of {package_id} changed on disk"));
        }
        let validated = parse_and_validate(&package.extension_json)
            .map_err(|error| format!("package-invalid ({error})"))?;
        if validated.manifest.id != package.package_id {
            return Err("package-invalid (manifest names another package)".into());
        }
        let licensed = match package.license_json.as_deref() {
            None => false,
            Some(raw) => serde_json::from_slice::<fe_format::license::License>(raw)
                .map_or(true, |license| {
                    matches!(license.license_type, fe_format::license::LicenseType::Paid)
                }),
        };
        let tier = self
            .governance
            .trust()
            .tier_for(package.trusted_by.as_deref());
        let facts = PackageFacts {
            package_id: package.package_id.clone(),
            version: package.version.clone(),
            content_hash: package.content_hash.clone(),
            component_digest: Some(package.component_digest.clone()),
            plaintext_digest: None,
            capability_digest: validated.capability_digest.clone(),
            // Untrusted builds are refused by `check` anyway; never let the facts read as first-party.
            trust: tier.unwrap_or(TrustTier::Publisher),
            writes: PackageFacts::manifest_writes(&validated.manifest),
            licensed,
            manifest: Some(Arc::new(validated.manifest.clone())),
        };
        let build = CatalogBuild {
            facts,
            name: validated.manifest.name.clone(),
            tier,
        };
        Ok((package, build))
    }

    /// List the store; new hashes are verified, the highest version per package id is the installed build.
    fn refresh_if_due(&mut self) {
        if self.last_scan.is_some_and(|at| at.elapsed() < CATALOG_SCAN) {
            return;
        }
        self.last_scan = Some(Instant::now());
        let listed = match self.store.list() {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(%error, "extension package store unreadable");
                return;
            }
        };
        let now = Instant::now();
        let mut next: BTreeMap<String, CatalogBuild> = BTreeMap::new();
        let mut listed_keys = std::collections::HashSet::new();
        for (package_id, content_hash) in listed {
            let key = (package_id.clone(), content_hash.clone());
            let due = match self.checked.get(&key) {
                None => true,
                Some(Err(retry_at)) => *retry_at <= now,
                Some(Ok(_)) => false,
            };
            if due {
                let outcome = self
                    .verify(&package_id, &content_hash)
                    .map(|(_, build)| build)
                    .map_err(|reason| {
                        tracing::warn!(%package_id, %reason, "stored extension failed re-verification");
                        now + REFUSAL_RETRY
                    });
                self.checked.insert(key.clone(), outcome);
            }
            if let Some(Ok(build)) = self.checked.get(&key) {
                let newer = next.get(&package_id).map_or(true, |current| {
                    semver_after(&build.facts.version, &current.facts.version)
                });
                if newer {
                    next.insert(package_id, build.clone());
                }
            }
            listed_keys.insert(key);
        }
        self.checked.retain(|key, _| listed_keys.contains(key));
        self.builds = next;
    }

    /// The installed build as R19 judges it (the launcher's `package`).
    fn check(&self, installation_id: &str) -> Option<PackageCheck> {
        let build = self.builds.get(installation_id)?;
        Some(match build.tier {
            Some(_) => PackageCheck::Verified(build.facts.clone()),
            None => PackageCheck::Untrusted(format!(
                "no loaded trust root vouches for {installation_id}"
            )),
        })
    }

    /// Fresh sealed bytes of the installed build, verified again before they run.
    fn package(&self, installation_id: &str) -> Result<PluginPackage, String> {
        let build = self
            .builds
            .get(installation_id)
            .ok_or_else(|| format!("{installation_id} is not installed"))?;
        self.verify(&build.facts.package_id, &build.facts.content_hash)
            .map(|(package, _)| package)
    }
}

/// Whether SemVer `candidate` sorts after `current` (unparsable versions never win).
fn semver_after(candidate: &str, current: &str) -> bool {
    let triplet = |version: &str| -> Option<(u64, u64, u64)> {
        let mut parts = version.split(['-', '+']).next()?.split('.');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    };
    matches!((triplet(candidate), triplet(current)), (Some(next), Some(now)) if next > now)
}

/// One build's resolver facts in one verse; instance counts are patched in by the activator.
fn build_facts(
    build: &CatalogBuild,
    verse_id: &str,
    petals: &[PetalAddress],
    local: &Governance,
    governance: &ExtensionGovernance,
) -> BuildFacts {
    let facts = &build.facts;
    let manifest = facts.manifest.as_deref();
    let installation_id = facts.package_id.clone();
    let hashes: Vec<&str> = facts.hashes().collect();
    let integrity = if build.tier.is_none() {
        BuildIntegrity::UntrustedSigner
    } else if facts.licensed && governance.entitlement(facts).is_err() {
        BuildIntegrity::EntitlementRequired
    } else {
        BuildIntegrity::Ok
    };
    let policy = match governance.evaluate(verse_id, facts) {
        Ok(()) => PolicyVerdict::Admits,
        Err(reason) if reason.contains("this device") => PolicyVerdict::LocalDenies,
        Err(_) => PolicyVerdict::VerseDenies,
    };
    BuildFacts {
        installation_id: installation_id.clone(),
        package_id: facts.package_id.clone(),
        version: facts.version.clone(),
        display_name: Some(build.name.clone()),
        integrity,
        revoked: governance.is_revoked_any(&hashes),
        kill_override: governance.is_overridden(verse_id, &facts.content_hash),
        policy,
        provides: manifest
            .map(|manifest| manifest.provides.clone())
            .unwrap_or_default(),
        writes_scene: manifest
            .and_then(|manifest| manifest.scene.as_ref())
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| !kinds.is_empty()),
        consent_recorded: local
            .consent(&installation_id)
            .is_some_and(|consent| consent.capability_digest == facts.capability_digest),
        disabled: local.is_locally_disabled(&installation_id, None),
        disabled_petals: petals
            .iter()
            .filter(|petal| local.is_locally_disabled(&installation_id, Some(&petal.petal_id)))
            .map(|petal| petal.petal_id.clone())
            .collect(),
        degraded: false,
        binding_instances: 0,
        instance_at_petals: BTreeSet::new(),
    }
}

/// The binding activator's hands: the installed builds, the stager and the live guests.
struct BridgeLauncher<'a> {
    catalog: &'a InstalledCatalog,
    stager: &'a InstanceStager,
    live: &'a LiveInstances,
}

impl BindingLauncher for BridgeLauncher<'_> {
    fn package(&self, installation_id: &str) -> Option<PackageCheck> {
        self.catalog.check(installation_id)
    }

    fn stage(
        &mut self,
        plan: &InstancePlan,
        package: &PackageFacts,
        first_activation: bool,
    ) -> Result<StagedInstance, String> {
        let sealed = self.catalog.package(&plan.installation_id)?;
        self.stager.stage(&StageRequest {
            plugin_id: &plan.plugin_id,
            installation_id: &plan.installation_id,
            package: &sealed,
            facts: package,
            petal: &plan.petal,
            first_activation,
        })
    }

    fn configure(&mut self, plugin_id: &str, call: &ConfigurePlan) -> Result<(), String> {
        let request = HookRequest::configure(
            &call.point.hook,
            &call.point.key,
            &call.binding_scope,
            &call.config.to_string(),
        );
        self.live
            .call(plugin_id, &request, None)
            .map(|_| ())
            .map_err(|failure| format!("{failure:?}"))
    }
}

// ---------------------------------------------------------------------------
// Accepted-payload decode (BND-10b)
// ---------------------------------------------------------------------------

/// Describes and reads accepted payload files for `decode::read_within_cap`, which sums the declared sizes against
/// the cap BEFORE any read and checks each file's length and BLAKE3 (steer 02:25; fe-plugin `src/AGENTS.md`).
#[derive(Clone)]
struct PayloadReader {
    api: ExtensionApiSlot,
    blobs: ExtensionBlobSlot,
}

impl PayloadReader {
    /// Each asset's stored row (size, type, BLAKE3) in `asset_ids` order; reads no blob byte.
    fn declared(&self, asset_ids: &[String]) -> Result<Vec<DecodeFile>, String> {
        let api = self.api.get().map_err(|error| error.to_string())?.clone();
        let mut vars = HashMap::new();
        vars.insert("ids".to_string(), json!(asset_ids));
        let sql = fe_api::query_guard::with_statement_timeout(
            "SELECT asset_id, content_type, size_bytes, content_hash FROM asset WHERE asset_id IN $ids",
        );
        let rows =
            db_rows_within(&api, sql, vars, DB_REPLY_TIMEOUT).map_err(|error| error.to_string())?;
        let mut declared = Vec::with_capacity(asset_ids.len());
        for asset_id in asset_ids {
            let row = rows
                .iter()
                .find(|row| row.get("asset_id").and_then(Value::as_str) == Some(asset_id.as_str()))
                .ok_or_else(|| format!("asset {asset_id} is not stored on this peer"))?;
            let size = row
                .get("size_bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("asset {asset_id} has no size"))?;
            let content_hash = row
                .get("content_hash")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
                .ok_or_else(|| format!("asset {asset_id} has no content hash"))?;
            let content_type = row
                .get("content_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();
            declared.push(DecodeFile {
                asset_id: asset_id.clone(),
                content_type,
                size,
                content_hash,
            });
        }
        Ok(declared)
    }

    /// At most `limit` bytes of the file's local blob; the caller compares length and hash.
    fn read_bounded(&self, file: &DecodeFile, limit: u64) -> Result<Vec<u8>, String> {
        let store = self
            .blobs
            .0
            .get()
            .ok_or("the blob store is not bound")?
            .clone();
        let hash = fe_runtime::blob_store::hash_from_hex(&file.content_hash)
            .map_err(|error| error.to_string())?;
        let path = store
            .get_blob_path(&hash)
            .ok_or_else(|| format!("the blob of {} is not on this peer", file.asset_id))?;
        let handle = std::fs::File::open(&path)
            .map_err(|error| format!("{}: {error}", file.asset_id))?;
        let capacity = limit.min(MAX_DECODE_BODY_BYTES as u64 + 1);
        let mut bytes = Vec::with_capacity(usize::try_from(capacity).unwrap_or(0));
        handle
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{}: {error}", file.asset_id))?;
        Ok(bytes)
    }
}

/// `DecodeHost` over the payload reader and the live guests.
struct DecodePort<'a> {
    reader: &'a PayloadReader,
    live: &'a LiveInstances,
}

impl DecodeHost for DecodePort<'_> {
    fn declared_files(&mut self, asset_ids: &[String]) -> Result<Vec<DecodeFile>, String> {
        self.reader.declared(asset_ids)
    }

    fn read_bounded(&mut self, file: &DecodeFile, limit: u64) -> Result<Vec<u8>, String> {
        self.reader.read_bounded(file, limit)
    }

    fn call_decode(
        &mut self,
        target: &CallTarget,
        call: &DecodeCall,
    ) -> Result<DecodeReply, DecodeCallError> {
        let request = HookRequest {
            hook: DECODE_HOOK.to_string(),
            key: call.key.clone(),
            binding_scope: call.binding_scope.clone(),
            config: call.config.to_string(),
            input: call.input.to_string(),
            body: call.body.clone(),
        };
        match self
            .live
            .call(&target.plugin_id, &request, Some(&call.stamp))
        {
            Ok(reply) => serde_json::from_str(&reply.output)
                .map(|output| DecodeReply { output })
                .map_err(|error| DecodeCallError::Fault(format!("decode output: {error}"))),
            Err(HookFailure::Refused(reason) | HookFailure::Declined(reason)) => {
                Err(DecodeCallError::Declined(reason))
            }
            Err(HookFailure::Fault(reason)) => Err(DecodeCallError::Fault(reason)),
        }
    }
}

// ---------------------------------------------------------------------------
// Published views (resolution cache, installations, verse owners)
// ---------------------------------------------------------------------------

/// One bound point at one petal: this host's answer and the row that decided it.
#[derive(Clone)]
struct PublishedPoint {
    point: BindingPoint,
    resolved: ResolvedPoint,
    winner: Option<BindingRecord>,
    /// The installed build of a resolved package (its `scene` grant shapes the scene gate).
    manifest: Option<Arc<ExtensionManifest>>,
}

/// One petal's published points and its full scope string.
#[derive(Clone)]
struct PublishedPetal {
    scope: String,
    points: Vec<PublishedPoint>,
}

impl PublishedPoint {
    /// The API shape (`GET .../bindings/effective`).
    fn effective(&self, petal_scope: &str) -> EffectivePoint {
        let winner = self.winner.as_ref();
        EffectivePoint {
            hook: self.point.hook.clone(),
            key: self.point.key.clone(),
            scope: winner.map_or_else(|| petal_scope.to_string(), |record| record.scope.clone()),
            mode: winner.map_or(BindingMode::Builtin, |record| record.mode),
            package_id: winner.and_then(|record| record.package_id.clone()),
            version_req: winner.and_then(|record| record.version_req.clone()),
            resolution: Some(self.resolved.resolution.clone()),
        }
    }

    /// The winning row when it binds the compiled-in terrain package, which serves from its config, no guest.
    fn native_terrain(&self) -> Option<&BindingRecord> {
        self.winner.as_ref().filter(|record| {
            record.mode == BindingMode::Extension
                && record.package_id.as_deref() == Some(TERRAIN_EXTENSION_ID)
        })
    }
}

#[derive(Default)]
struct BindingViewInner {
    generation: u64,
    petals: HashMap<String, PublishedPetal>,
    status: Vec<(String, u64)>,
}

/// This host's effective bindings per petal, as the worker last resolved them.
#[derive(Clone, Default)]
struct BindingView(Arc<RwLock<BindingViewInner>>);

impl BindingView {
    fn generation(&self) -> u64 {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
    }

    fn points_at(&self, petal_id: &str) -> Option<Vec<EffectivePoint>> {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let petal = inner.petals.get(petal_id)?;
        Some(
            petal
                .points
                .iter()
                .map(|point| point.effective(&petal.scope))
                .collect(),
        )
    }

    /// Who serves `point` at `petal_id`, from the published cache (the terrain bridge's `petal.map` input). A resolved
    /// point answers `Call` even before its lazy instance runs: `ExtensionBindings::call_hook` starts it.
    fn need_at(&self, petal_id: &str, point: &BindingPoint) -> Need {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let Some(published) = inner.petals.get(petal_id).and_then(|petal| {
            petal
                .points
                .iter()
                .find(|candidate| candidate.point == *point)
        }) else {
            return Need::Builtin(None);
        };
        if let Some(record) = published.native_terrain() {
            return Need::Call(CallTarget {
                plugin_id: TERRAIN_EXTENSION_ID.to_string(),
                installation_id: TERRAIN_EXTENSION_ID.to_string(),
                key: InstanceKey::new(petal_id.to_string(), TERRAIN_EXTENSION_ID.to_string()),
                binding_scope: record.scope.clone(),
                config: record.config.clone(),
            });
        }
        match &published.resolved.resolution {
            BindingResolution::Resolved { package_id, scope } => Need::Call(CallTarget {
                plugin_id: binding_instance_id(package_id, petal_id),
                installation_id: package_id.clone(),
                key: InstanceKey::new(petal_id.to_string(), package_id.clone()),
                binding_scope: scope.clone(),
                config: published
                    .winner
                    .as_ref()
                    .map_or_else(|| json!({}), |record| record.config.clone()),
            }),
            _ => Need::Builtin(Some(published.resolved.clone())),
        }
    }

    /// The extension resolving `hook` (keyless) at `petal_id`, as the `scene.*` gate reads it; never blocks.
    fn scene_binding(&self, petal_id: &str, hook: &str) -> Option<SceneBinding> {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let published = inner
            .petals
            .get(petal_id)?
            .points
            .iter()
            .find(|candidate| candidate.point.hook == hook && candidate.point.key.is_empty())?;
        let BindingResolution::Resolved { package_id, scope } = &published.resolved.resolution
        else {
            return None;
        };
        let scene = published
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.scene.clone());
        let writes_properties = scene
            .as_ref()
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| kinds.contains(&SceneWrite::SetProperty));
        let ticks = published
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.tick.is_some());
        Some(SceneBinding {
            package_id: package_id.clone(),
            binding_scope: scope.clone(),
            config: published
                .winner
                .as_ref()
                .map_or_else(|| json!({}), |record| record.config.clone()),
            property_scope: scene
                .and_then(|scene| scene.property_scope)
                .unwrap_or_default(),
            may_augment: writes_properties && ticks,
        })
    }

    fn status_rows(&self) -> Vec<(String, u64)> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .status
            .clone()
    }

    /// The `ui.panel` and `egress.snippet` points that resolve at `petal_id` (B10c cache input).
    fn slots_at(&self, petal_id: &str) -> (Vec<BoundPanel>, Vec<SnippetTarget>) {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let mut panels = Vec::new();
        let mut targets = Vec::new();
        let points = inner
            .petals
            .get(petal_id)
            .map(|petal| petal.points.as_slice());
        for published in points.unwrap_or_default() {
            let BindingResolution::Resolved { package_id, .. } = &published.resolved.resolution
            else {
                continue;
            };
            let point = &published.point;
            match point.hook.as_str() {
                PANEL_HOOK => panels.push(BoundPanel {
                    panel: point.key.clone(),
                    package_id: package_id.clone(),
                }),
                SNIPPET_HOOK => targets.push(SnippetTarget {
                    package_id: package_id.clone(),
                    key: point.key.clone(),
                }),
                _ => {}
            }
        }
        (panels, targets)
    }

    fn replace(&self, petals: HashMap<String, PublishedPetal>, status: Vec<(String, u64)>) {
        let mut inner = self.0.write().unwrap_or_else(PoisonError::into_inner);
        inner.generation += 1;
        inner.petals = petals;
        inner.status = status;
    }
}

/// One installation as the status routes show it.
#[derive(Clone)]
struct IndexRow {
    package_id: String,
    version: String,
    name: String,
    revoked: bool,
    disabled: bool,
    consented: bool,
    instances: Vec<RunningInstance>,
}

/// Installations and their instances, refreshed by the worker; status reads never take the manager lock.
#[derive(Clone, Default)]
struct InstallationIndex(Arc<RwLock<BTreeMap<String, IndexRow>>>);

impl InstallationIndex {
    fn rows(&self) -> BTreeMap<String, IndexRow> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn row(&self, installation_id: &str) -> Option<IndexRow> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(installation_id)
            .cloned()
    }

    fn replace(&self, rows: BTreeMap<String, IndexRow>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = rows;
    }
}

/// Whether the local DID is a verse's Owner (`verse.created_by`), cached; offers only, the DB re-checks overrides.
#[derive(Clone)]
struct VerseOwners {
    api: ExtensionApiSlot,
    local_did: String,
    cache: Arc<Mutex<HashMap<String, (bool, Instant)>>>,
}

impl VerseOwners {
    /// Never takes the manager lock: callers may hold it (ops drain `consent_request`).
    fn is_owner(&self, verse_id: &str) -> bool {
        if let Some((owner, at)) = lock(&self.cache).get(verse_id).copied() {
            if at.elapsed() < OWNER_CACHE {
                return owner;
            }
        }
        let Ok(api) = self.api.get().cloned() else {
            return false;
        };
        let mut vars = HashMap::new();
        vars.insert("v".to_string(), json!(verse_id));
        let sql = fe_api::query_guard::with_statement_timeout(
            "SELECT created_by FROM verse WHERE verse_id = $v LIMIT 1",
        );
        let owner = match db_rows_within(&api, sql, vars, OWNER_LOOKUP_TIMEOUT) {
            Ok(rows) => {
                rows.first()
                    .and_then(|row| row.get("created_by"))
                    .and_then(Value::as_str)
                    == Some(self.local_did.as_str())
            }
            Err(error) => {
                tracing::debug!(verse_id, %error, "verse owner lookup failed");
                return false;
            }
        };
        lock(&self.cache).insert(verse_id.to_string(), (owner, Instant::now()));
        owner
    }
}

// ---------------------------------------------------------------------------
// The binding worker: resolution, activation, decode, snippets and governance work
// ---------------------------------------------------------------------------

/// Plugin-manager work governance hands off (never run on the Bevy thread).
enum ManagerWork {
    Consent(ConsentRecord),
    Override {
        verse_id: String,
        content_hash: String,
    },
    Reevaluate(String),
}

/// A host-initiated hook call's answer (e.g. `petal.map` from the terrain bridge).
#[derive(Debug, Clone)]
pub(crate) enum HookAnswer {
    /// The bound instance answered with this output JSON.
    Output { package_id: String, output: String },
    /// No instance serves the point here; the built-in serves.
    Builtin,
    /// The call failed (fail open: the built-in serves).
    Failed(String),
}

enum BindingSignal {
    Dirty(String, Trigger),
    AllDirty(Trigger),
    Navigate(Option<PetalAddress>),
    Forget(String),
    Payload(DecodeJob),
    Snippets {
        petal_id: String,
        requests: Vec<SnippetRequest>,
    },
    Call {
        petal_id: String,
        point: BindingPoint,
        input: Value,
        reply: Sender<HookAnswer>,
    },
    /// One `scene.*` gate call; the gate stops waiting at the hook's deadline (fail open).
    Scene {
        call: SceneHookCall,
        reply: SceneHookReplySender,
    },
    Manager(ManagerWork),
}

/// A provider answer for the bound snippet cache.
struct SnippetAnswer {
    target: SnippetTarget,
    label: String,
    text: String,
}

/// Queue into the binding worker; never blocks (DB-thread and Bevy callers).
#[derive(Clone)]
struct BindingSignals {
    sender: Sender<BindingSignal>,
    backlog: Arc<AtomicUsize>,
}

impl BindingSignals {
    fn send(&self, signal: BindingSignal) {
        if self.sender.send(signal).is_err() {
            tracing::debug!("extension binding worker is gone");
        }
    }

    fn dirty(&self, verse_id: &str, trigger: Trigger) {
        self.send(BindingSignal::Dirty(verse_id.to_string(), trigger));
    }

    /// Queue one accepted payload; past the backlog cap it stays opaque (counted by the warning).
    fn payload(&self, job: DecodeJob) {
        if self.backlog.fetch_add(1, Ordering::AcqRel) >= MAX_PAYLOAD_BACKLOG {
            self.backlog.fetch_sub(1, Ordering::AcqRel);
            tracing::warn!(payload_id = %job.payload_id, "decode backlog full; payload stays opaque");
            return;
        }
        self.send(BindingSignal::Payload(job));
    }
}

/// Governance's hand into the plugin manager: revocation lists apply inline (pull thread, startup), every other
/// change queues to the binding worker so the Bevy thread never takes the manager lock.
struct ManagerControlHandle {
    manager: Arc<Mutex<PluginManager>>,
    signals: BindingSignals,
}

impl ManagerControl for ManagerControlHandle {
    fn record_consent(&self, record: ConsentRecord) {
        self.signals
            .send(BindingSignal::Manager(ManagerWork::Consent(record)));
    }

    fn add_kill_list_override(&self, verse_id: &str, content_hash: &str) {
        self.signals
            .send(BindingSignal::Manager(ManagerWork::Override {
                verse_id: verse_id.to_string(),
                content_hash: content_hash.to_string(),
            }));
    }

    fn reevaluate_verse_policy(&self, verse_id: &str) -> Vec<String> {
        self.signals
            .send(BindingSignal::Manager(ManagerWork::Reevaluate(
                verse_id.to_string(),
            )));
        Vec::new()
    }

    fn accept_revocation_list(
        &self,
        list: &fe_plugin::trust_roots::RevocationList,
    ) -> Result<Vec<String>, String> {
        let stopped = lock(&self.manager)
            .accept_revocation_list(list)
            .map_err(|error| error.to_string());
        self.signals
            .send(BindingSignal::AllDirty(Trigger::KillList));
        stopped
    }
}

struct BindingWorker {
    manager: Arc<Mutex<PluginManager>>,
    governance: ExtensionGovernance,
    activator: BindingActivator,
    catalog: InstalledCatalog,
    stager: InstanceStager,
    live: LiveInstances,
    petals: PetalDirectory,
    db: Sender<DbCommand>,
    view: BindingView,
    index: InstallationIndex,
    reader: PayloadReader,
    owners: VerseOwners,
    answers: Sender<SnippetAnswer>,
    backlog: Arc<AtomicUsize>,
    admission: bool,
    local_did: String,
    snapshots: HashMap<String, VerseBindings>,
    retry: HashMap<String, Instant>,
    /// When "Allow in <verse>" hydration runs next; `None` once it landed.
    acceptance_retry: Option<Instant>,
    last_sweep: Instant,
    changed: bool,
    /// `(instance id, degraded)` of every governed instance at the last publish.
    running: Vec<(String, bool)>,
}

/// One `LoadBindingSnapshot` round trip (pinned I-DB command).
fn load_binding_snapshot(db: &Sender<DbCommand>, verse_id: &str) -> Result<VerseBindings, String> {
    let (reply, receiver) = crossbeam::channel::bounded(1);
    db.send_timeout(
        DbCommand::LoadBindingSnapshot {
            verse_id: verse_id.to_string(),
            reply,
        },
        DB_SEND_TIMEOUT,
    )
    .map_err(|_| "the database is not accepting commands".to_string())?;
    receiver
        .recv_timeout(DB_REPLY_TIMEOUT)
        .map_err(|_| "the database did not answer in time".to_string())?
        .map_err(|refusal: BindingRefusal| format!("{}: {}", refusal.code, refusal.message))
}

/// The stored versions as the resolver reads them.
fn stored_rows(bindings: &VerseBindings) -> Vec<StoredRow> {
    bindings
        .versions
        .iter()
        .map(|version: &StoredBinding| StoredRow {
            record: version.record.clone(),
            written_authorized: version.written_authorized,
        })
        .collect()
}

impl BindingWorker {
    fn run(mut self, signals: Receiver<BindingSignal>, stop: StopSignal) {
        loop {
            if stop.is_requested() {
                break;
            }
            match signals.recv_timeout(WORKER_TURN) {
                Ok(signal) => self.absorb(signal),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(signal) = signals.try_recv() {
                self.absorb(signal);
            }
            self.turn();
        }
        tracing::info!("extension binding worker stopped");
    }

    /// Periodic work: store scan, acceptance hydration, due retries, dirty verses, idle sweep, publication.
    fn turn(&mut self) {
        self.catalog.refresh_if_due();
        if self.acceptance_retry.is_some_and(|at| at <= Instant::now()) {
            self.hydrate_acceptances();
        }
        let now = Instant::now();
        let due: Vec<String> = self
            .retry
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(verse, _)| verse.clone())
            .collect();
        for verse_id in due {
            self.retry.remove(&verse_id);
            self.activator.mark_dirty(&verse_id, Trigger::BindingRow);
        }
        self.resolve_dirty();
        if self.last_sweep.elapsed() >= IDLE_SWEEP {
            self.last_sweep = Instant::now();
            let plan = self.activator.sweep_idle();
            self.apply(plan);
        }
        self.live.prune();
        self.publish();
    }

    fn absorb(&mut self, signal: BindingSignal) {
        match signal {
            BindingSignal::Dirty(verse_id, trigger) => {
                self.activator.mark_dirty(&verse_id, trigger)
            }
            BindingSignal::AllDirty(trigger) => self.activator.mark_all_dirty(trigger),
            BindingSignal::Navigate(active) => {
                let plan = self.activator.navigate(active);
                self.apply(plan);
            }
            BindingSignal::Forget(installation_id) => {
                self.activator.forget_installation(&installation_id);
                self.changed = true;
            }
            BindingSignal::Payload(job) => {
                self.backlog.fetch_sub(1, Ordering::AcqRel);
                self.decode(&job);
            }
            BindingSignal::Snippets { petal_id, requests } => {
                self.answer_snippets(&petal_id, requests)
            }
            BindingSignal::Call {
                petal_id,
                point,
                input,
                reply,
            } => {
                let answer = self.call_point(&petal_id, &point, &input);
                let _ = reply.send(answer);
            }
            BindingSignal::Scene { call, reply } => self.scene_call(call, reply),
            BindingSignal::Manager(work) => self.manager_work(work),
        }
    }

    fn manager_work(&mut self, work: ManagerWork) {
        let mut manager = lock(&self.manager);
        match work {
            ManagerWork::Consent(record) => {
                manager.governance_mut().record_consent(record);
                drop(manager);
                self.activator.mark_all_dirty(Trigger::Lifecycle);
            }
            ManagerWork::Override {
                verse_id,
                content_hash,
            } => {
                manager
                    .governance_mut()
                    .add_kill_list_override(&verse_id, &content_hash);
                drop(manager);
                self.activator.mark_dirty(&verse_id, Trigger::KillList);
            }
            ManagerWork::Reevaluate(verse_id) => {
                let stopped = manager.reevaluate_verse_policy(&verse_id);
                drop(manager);
                if !stopped.is_empty() {
                    tracing::info!(%verse_id, stopped = ?stopped, "verse policy change stopped extension instances");
                }
                self.activator.mark_dirty(&verse_id, Trigger::Policy);
            }
        }
    }

    /// "Allow in <verse>" records live in the audit trail (Bind/Unbind with a verse); replay them oldest first.
    fn hydrate_acceptances(&mut self) {
        // Until this lands no binding instance starts (row 12 refuses), so retry on the snapshot spacing.
        self.acceptance_retry = Some(Instant::now() + SNAPSHOT_RETRY);
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let command = DbCommand::ListExtensionAudit {
            installation: None,
            verse_id: None,
            limit: ACCEPTANCE_SCAN_LIMIT,
            reply,
        };
        if self.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return;
        }
        let rows: Vec<Value> = match receiver.recv_timeout(DB_REPLY_TIMEOUT) {
            Ok(Ok(rows)) => rows,
            Ok(Err(error)) => {
                tracing::warn!(%error, "extension acceptances did not load; retrying");
                return;
            }
            Err(_) => return,
        };
        let mut accepted: BTreeSet<(String, String)> = BTreeSet::new();
        for entry in rows
            .into_iter()
            .rev()
            .filter_map(|row| serde_json::from_value::<PolicyEntry>(row).ok())
        {
            // Acceptance is this identity's word only (R12): a rotated key's "Allow in <verse>" never carries over.
            if entry.outcome != PolicyOutcome::Ok || entry.actor_did != self.local_did {
                continue;
            }
            let (Some(installation), Some(verse_id)) = (entry.installation, entry.verse_id) else {
                continue;
            };
            match entry.action {
                PolicyAction::Bind => {
                    accepted.insert((installation, verse_id));
                }
                PolicyAction::Unbind => {
                    accepted.remove(&(installation, verse_id));
                }
                _ => {}
            }
        }
        {
            let mut manager = lock(&self.manager);
            for (installation_id, verse_id) in &accepted {
                manager
                    .governance_mut()
                    .accept_in_verse(installation_id, verse_id);
            }
        }
        self.acceptance_retry = None;
        self.activator.mark_all_dirty(Trigger::Lifecycle);
    }

    fn local_facts(&self, verse_id: &str, petals: &[PetalAddress]) -> LocalFacts {
        let is_owner = self.owners.is_owner(verse_id);
        let manager = lock(&self.manager);
        let local = manager.governance();
        LocalFacts {
            local_did: self.local_did.clone(),
            host_enabled: self.admission && !local.is_disabled_all(),
            is_verse_owner_host: is_owner,
            builds: self
                .catalog
                .builds
                .values()
                .map(|build| build_facts(build, verse_id, petals, local, &self.governance))
                .collect(),
            accepted: local.acceptances(),
            registry: Vec::new(),
            package_policy: PackagePolicy::default(),
        }
    }

    fn resolve_dirty(&mut self) {
        for (verse_id, triggers) in self.activator.take_dirty() {
            let petals = self.petals.of_verse(&verse_id);
            let bindings = match load_binding_snapshot(&self.db, &verse_id) {
                Ok(bindings) => bindings,
                Err(reason) => {
                    tracing::warn!(%verse_id, %reason, "binding snapshot did not load; retrying");
                    self.retry.insert(verse_id, Instant::now() + SNAPSHOT_RETRY);
                    continue;
                }
            };
            let rows = stored_rows(&bindings);
            // Unknown roles judge every author below Manager: the built-in serves (fail closed).
            let roles = bindings.roles.clone().unwrap_or_default();
            let facts = self.local_facts(&verse_id, &petals);
            let running = lock(&self.manager).running_instances();
            let snapshot = BindingSnapshot {
                rows: &rows,
                roles: &roles,
                facts: &facts,
                stored_terrain_petals: &bindings.stored_terrain_petals,
            };
            let plan = self
                .activator
                .reconcile(&verse_id, &snapshot, &petals, &running);
            tracing::debug!(%verse_id, ?triggers, starts = plan.starts.len(), stops = plan.stops.len(), "bindings re-resolved");
            self.snapshots.insert(verse_id, bindings);
            self.apply(plan);
            self.changed = true;
        }
    }

    fn apply(&mut self, plan: ActivationPlan) {
        if plan.is_empty() {
            return;
        }
        let Self {
            activator,
            manager,
            catalog,
            stager,
            live,
            ..
        } = self;
        let mut launcher = BridgeLauncher {
            catalog: &*catalog,
            stager: &*stager,
            live: &*live,
        };
        let report = activator.apply(plan, &**manager, &mut launcher);
        for (plugin_id, reason, detail) in &report.refused {
            tracing::info!(%plugin_id, ?reason, %detail, "binding instance refused");
        }
        for (plugin_id, detail) in &report.faults {
            tracing::warn!(%plugin_id, %detail, "binding configure faulted");
        }
        self.changed = true;
    }

    fn decode(&mut self, job: &DecodeJob) {
        let Self {
            activator,
            manager,
            catalog,
            stager,
            live,
            reader,
            ..
        } = self;
        let mut launcher = BridgeLauncher {
            catalog: &*catalog,
            stager: &*stager,
            live: &*live,
        };
        let mut host = DecodePort {
            reader: &*reader,
            live: &*live,
        };
        match decode_payload(activator, &**manager, &mut launcher, &mut host, job) {
            DecodeOutcome::Decoded { staged } => {
                tracing::debug!(payload_id = %job.payload_id, staged, "payload decoded")
            }
            DecodeOutcome::Opaque(reason) => {
                tracing::debug!(payload_id = %job.payload_id, ?reason, "payload stays opaque")
            }
            DecodeOutcome::Failed(error) => {
                tracing::warn!(payload_id = %job.payload_id, ?error, "payload decode failed")
            }
        }
        self.changed = true;
    }

    /// Call the instance serving `point` at `petal_id`, lazily started; any failure leaves the built-in serving.
    fn call_point(&mut self, petal_id: &str, point: &BindingPoint, input: &Value) -> HookAnswer {
        let target = {
            let Self {
                activator,
                manager,
                catalog,
                stager,
                live,
                ..
            } = self;
            let mut launcher = BridgeLauncher {
                catalog: &*catalog,
                stager: &*stager,
                live: &*live,
            };
            activator.ensure_call_target(petal_id, point, &**manager, &mut launcher)
        };
        self.changed = true;
        let Ok(target) = target else {
            return HookAnswer::Builtin;
        };
        let request = HookRequest {
            hook: point.hook.clone(),
            key: point.key.clone(),
            binding_scope: target.binding_scope.clone(),
            config: target.config.to_string(),
            input: input.to_string(),
            body: Vec::new(),
        };
        match self.live.call(&target.plugin_id, &request, None) {
            Ok(reply) => HookAnswer::Output {
                package_id: target.key.package_id,
                output: reply.output,
            },
            Err(failure) => HookAnswer::Failed(format!("{failure:?}")),
        }
    }

    /// One `scene.*` call on the instance the binding names (lazily started); a moved binding or no instance is a
    /// fault, so the gate lets the built-in proceed.
    fn scene_call(&mut self, call: SceneHookCall, reply: SceneHookReplySender) {
        if reply.is_closed() {
            return;
        }
        let point = BindingPoint::new(call.hook.id(), "");
        let target = {
            let Self {
                activator,
                manager,
                catalog,
                stager,
                live,
                ..
            } = self;
            let mut launcher = BridgeLauncher {
                catalog: &*catalog,
                stager: &*stager,
                live: &*live,
            };
            activator.ensure_call_target(&call.petal_id, &point, &**manager, &mut launcher)
        };
        self.changed = true;
        let answer = match target {
            Ok(target) if target.key.package_id == call.package_id => {
                let request = HookRequest {
                    hook: call.hook.id().to_string(),
                    key: String::new(),
                    binding_scope: call.binding_scope,
                    config: call.config.to_string(),
                    input: call.input.to_string(),
                    body: Vec::new(),
                };
                match self.live.call(&target.plugin_id, &request, None) {
                    Ok(reply) => serde_json::from_str(&reply.output)
                        .map(SceneHookReply::Output)
                        .unwrap_or_else(|error| {
                            SceneHookReply::Fault(format!("scene hook output: {error}"))
                        }),
                    Err(HookFailure::Refused(reason)) => SceneHookReply::Refused(reason),
                    Err(HookFailure::Declined(reason)) => SceneHookReply::Declined(reason),
                    Err(HookFailure::Fault(reason)) => SceneHookReply::Fault(reason),
                }
            }
            Ok(_) => SceneHookReply::Fault("the binding now names another package".into()),
            Err(_) => SceneHookReply::Fault(format!("no instance serves {} here", call.hook.id())),
        };
        let _ = reply.send(answer);
    }

    /// `egress.snippet`: `{sql, query_url, export_urls}` -> `{label, snippet}`; a failure stores an empty answer.
    fn answer_snippets(&mut self, petal_id: &str, requests: Vec<SnippetRequest>) {
        for request in requests {
            let point = BindingPoint::new(SNIPPET_HOOK, request.target.key.clone());
            let input = json!({
                "sql": request.args.sql,
                "query_url": request.args.query_url,
                "export_urls": { "parquet": request.args.export_parquet, "csv": request.args.export_csv },
            });
            let (label, text) = match self.call_point(petal_id, &point, &input) {
                HookAnswer::Output { package_id, output }
                    if package_id == request.target.package_id =>
                {
                    let parsed: Value = serde_json::from_str(&output).unwrap_or(Value::Null);
                    let field = |name: &str| {
                        parsed
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (field("label"), field("snippet"))
                }
                HookAnswer::Failed(reason) => {
                    tracing::debug!(key = %request.target.key, %reason, "egress.snippet provider failed");
                    (String::new(), String::new())
                }
                // The point moved to another package or back to the built-in: the stale answer is dropped.
                HookAnswer::Output { .. } | HookAnswer::Builtin => (String::new(), String::new()),
            };
            if self
                .answers
                .send(SnippetAnswer {
                    target: request.target,
                    label,
                    text,
                })
                .is_err()
            {
                return;
            }
        }
    }

    /// The winning row per point at each petal with this host's resolution (BindingHost::effective, bound slots,
    /// `petal.map`). A row binding the compiled-in terrain package is never staged, so it publishes as resolved.
    fn published_petals(&self) -> HashMap<String, PublishedPetal> {
        let mut petals_out = HashMap::new();
        for (verse_id, bindings) in &self.snapshots {
            let rows = stored_rows(bindings);
            let roles = bindings.roles.clone().unwrap_or_default();
            for petal in self.petals.of_verse(verse_id) {
                let points = self
                    .activator
                    .resolutions_at(&petal.petal_id)
                    .into_iter()
                    .map(|(point, resolved)| {
                        // OQ-B6: the petal's own terrain column decided, so no outer row wins here (R-BND-13).
                        let winner: Option<BindingRecord> = if resolved.detail.implicit_terrain {
                            None
                        } else {
                            resolution_scopes(&petal.verse_id, &petal.fractal_id, &petal.petal_id)
                                .iter()
                                .find_map(|scope| {
                                    effective_binding(&rows, &roles, scope, &point).cloned()
                                })
                        };
                        let manifest = match &resolved.resolution {
                            BindingResolution::Resolved { package_id, .. } => self
                                .catalog
                                .builds
                                .get(package_id)
                                .and_then(|build| build.facts.manifest.clone()),
                            _ => None,
                        };
                        let mut published = PublishedPoint {
                            point,
                            resolved,
                            winner,
                            manifest,
                        };
                        if let Some(scope) = published
                            .native_terrain()
                            .map(|record| record.scope.clone())
                        {
                            published.resolved.resolution = BindingResolution::Resolved {
                                package_id: TERRAIN_EXTENSION_ID.to_string(),
                                scope,
                            };
                        }
                        published
                    })
                    .collect();
                let scope = fe_database::build_scope(
                    &petal.verse_id,
                    Some(petal.fractal_id.as_str()),
                    Some(petal.petal_id.as_str()),
                );
                petals_out.insert(petal.petal_id.clone(), PublishedPetal { scope, points });
            }
        }
        petals_out
    }

    fn publish(&mut self) {
        let running = lock(&self.manager).running_instances();
        // The ops drain starts, disables and uninstalls without a signal: a changed running set is a lifecycle
        // trigger, so local Disable/Enable/Uninstall reach the resolution cache (and the bound slots) in one turn.
        let fingerprint: Vec<(String, bool)> = running
            .iter()
            .map(|instance| (instance.plugin_id.clone(), instance.degraded))
            .collect();
        if fingerprint != self.running {
            self.running = fingerprint;
            self.activator.mark_all_dirty(Trigger::Lifecycle);
        }
        let mut consents = BTreeSet::new();
        let mut disabled = BTreeSet::new();
        {
            let manager = lock(&self.manager);
            let local = manager.governance();
            for build in self.catalog.builds.values() {
                let id = &build.facts.package_id;
                let consent = local.consent(id);
                if consent
                    .is_some_and(|record| record.capability_digest == build.facts.capability_digest)
                {
                    consents.insert(id.clone());
                }
                if local.is_disabled_all() || local.is_locally_disabled(id, None) {
                    disabled.insert(id.clone());
                }
            }
        }
        let rows = self
            .catalog
            .builds
            .values()
            .map(|build| {
                let id = build.facts.package_id.clone();
                let hashes: Vec<&str> = build.facts.hashes().collect();
                let row = IndexRow {
                    package_id: id.clone(),
                    version: build.facts.version.clone(),
                    name: build.name.clone(),
                    revoked: self.governance.is_revoked_any(&hashes),
                    disabled: disabled.contains(&id),
                    consented: consents.contains(&id),
                    instances: running
                        .iter()
                        .filter(|instance| instance.installation_id == id)
                        .cloned()
                        .collect(),
                };
                (id, row)
            })
            .collect();
        self.index.replace(rows);
        if std::mem::take(&mut self.changed) {
            self.view
                .replace(self.published_petals(), self.activator.status_rows());
        }
    }
}

// ---------------------------------------------------------------------------
// Ops drain ports (F-OPS)
// ---------------------------------------------------------------------------

/// What the ops worker needs from the binary. No method takes the manager lock: the drain calls several of them
/// while it holds it (`rows_event`, `consent_request`).
struct BridgeOpsPorts {
    governance: ExtensionGovernance,
    stager: InstanceStager,
    db: Sender<DbCommand>,
    owners: VerseOwners,
    index: InstallationIndex,
    signals: BindingSignals,
    /// This host's last resolution per petal (the Bindings view's states and offers).
    view: BindingView,
}

impl BridgeOpsPorts {
    /// One durable audit entry as the local DID; waits for the DB so the caller can refuse when it is not recorded.
    fn append_audit_blocking(&self, entry: PolicyEntry) -> Result<(), String> {
        let entry = serde_json::to_value(&entry).map_err(|error| error.to_string())?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.db
            .send_timeout(DbCommand::AppendExtensionAudit { entry, reply }, DB_SEND_TIMEOUT)
            .map_err(|_| "the database is not accepting commands".to_string())?;
        receiver
            .recv_timeout(DB_REPLY_TIMEOUT)
            .map_err(|_| "the database did not answer; the change may or may not be recorded".to_string())??;
        Ok(())
    }

    fn local_entry(&self, action: PolicyAction, target: String) -> PolicyEntry {
        PolicyEntry {
            action,
            installation: None,
            package_id: None,
            content_hash: None,
            actor_did: self.governance.local_did().to_string(),
            target,
            outcome: PolicyOutcome::Ok,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: rfc3339_utc(now_ms()),
        }
    }
}

impl OpsPorts for BridgeOpsPorts {
    fn now(&self) -> String {
        rfc3339_utc(now_ms())
    }

    fn trust_tier(&self, trusted_by: Option<&str>) -> TrustTier {
        // No vouching root reads as `Publisher`, which `first-party-only` refuses; `DevUnsigned` would be admitted
        // there and only `tier_for(None)` under the debug dev flag may produce it.
        self.governance
            .trust()
            .tier_for(trusted_by)
            .unwrap_or(TrustTier::Publisher)
    }

    fn is_verse_owner(&self, verse_id: &str) -> bool {
        self.owners.is_owner(verse_id)
    }

    fn persist_consent(&self, record: &ConsentRecord) -> Result<(), String> {
        // One durable `consent` audit entry; governance then hands the record to the manager as well.
        self.governance.record_consent(record.clone())?;
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
        Ok(())
    }

    fn persist_consent_revoke(
        &self,
        installation_id: &str,
        package_id: &str,
    ) -> Result<(), String> {
        let entry = PolicyEntry {
            action: PolicyAction::ConsentRevoke,
            installation: Some(installation_id.to_string()),
            package_id: Some(package_id.to_string()),
            content_hash: None,
            actor_did: self.governance.local_did().to_string(),
            target: installation_id.to_string(),
            outcome: PolicyOutcome::Ok,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: rfc3339_utc(now_ms()),
        };
        let entry = serde_json::to_value(&entry).map_err(|error| error.to_string())?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.db
            .send_timeout(
                DbCommand::AppendExtensionAudit { entry, reply },
                DB_SEND_TIMEOUT,
            )
            .map_err(|_| "the database is not accepting commands".to_string())?;
        receiver.recv_timeout(DB_REPLY_TIMEOUT).map_err(|_| {
            "the database did not answer; the revoke may or may not be recorded".to_string()
        })??;
        // An uninstall also lands here; the activator drops what it held for the installation.
        self.signals
            .send(BindingSignal::Forget(installation_id.to_string()));
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
        Ok(())
    }

    fn record_kill_list_override(
        &self,
        verse_id: &str,
        package_id: &str,
        content_hash: &str,
        confirm: &str,
    ) -> Result<(), String> {
        let params = json!({ "verse_id": verse_id, "package_id": package_id, "content_hash": content_hash, "confirm": confirm });
        self.governance
            .kill_list_override(self.governance.local_did(), &params)
            .map_err(|refusal| format!("{}: {}", refusal.code, refusal.message))?;
        self.signals.dirty(verse_id, Trigger::KillList);
        Ok(())
    }

    fn persist_acceptance(
        &self,
        installation_id: &str,
        verse_id: &str,
        package_id: &str,
    ) -> Result<(), String> {
        // fe-database `acceptance_entry` shape: `bind` with the verse, read back by `hydrate_acceptances`.
        let mut entry = self.local_entry(PolicyAction::Bind, format!("{installation_id}@{verse_id}"));
        entry.installation = Some(installation_id.to_string());
        entry.package_id = Some(package_id.to_string());
        entry.verse_id = Some(verse_id.to_string());
        self.append_audit_blocking(entry)?;
        self.signals.dirty(verse_id, Trigger::Lifecycle);
        Ok(())
    }

    fn audit_change(&self, action: PolicyAction, installation_id: &str) {
        let mut entry = self.local_entry(action, installation_id.to_string());
        entry.installation = Some(installation_id.to_string());
        entry.package_id = Some(installation_id.to_string());
        self.governance.append_audit(entry);
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
    }

    fn bindings_snapshot(&self, scope: &ActiveScope) -> Result<BindingsSnapshot, String> {
        let petal = self
            .stager
            .petals
            .get(&scope.petal_id)
            .filter(|petal| petal.verse_id == scope.verse_id)
            .ok_or("the open petal is not in the loaded hierarchy yet")?;
        let plane = load_binding_snapshot(&self.db, &petal.verse_id)?;
        let rows = stored_rows(&plane);
        let roles = plane.roles.clone().unwrap_or_default();
        let local_did = self.governance.local_did();
        let local_is_owner = self.owners.is_owner(&petal.verse_id);
        let chain = resolution_scopes(&petal.verse_id, &petal.fractal_id, &petal.petal_id);
        let named = |kind: &str, name: &str| if name.is_empty() { kind.to_string() } else { format!("{kind} {name}") };
        let targets = [
            (
                BindingScopeLevel::Petal,
                named("Petal", &scope.petal_name),
                ScopeTarget {
                    verse_id: petal.verse_id.clone(),
                    fractal_id: Some(petal.fractal_id.clone()),
                    petal_id: Some(petal.petal_id.clone()),
                },
            ),
            (
                BindingScopeLevel::Fractal,
                named("Fractal", &scope.fractal_name),
                ScopeTarget {
                    verse_id: petal.verse_id.clone(),
                    fractal_id: Some(petal.fractal_id.clone()),
                    petal_id: None,
                },
            ),
            (
                BindingScopeLevel::Verse,
                named("Verse", &scope.verse_name),
                ScopeTarget {
                    verse_id: petal.verse_id.clone(),
                    fractal_id: None,
                    petal_id: None,
                },
            ),
        ];
        // Manager+ by subtree authority on the judged role plane (an Owner pin cuts it); without a plane only the
        // verse Owner is offered a scope. The DB plane judges every write again.
        let manages = |scope_string: &str| match &plane.roles {
            Some(roles) => roles
                .authority(local_did, scope_string)
                .is_at_least(RoleLevel::Manager),
            None => local_is_owner,
        };
        let scopes: Vec<ScopeOption> = chain
            .iter()
            .zip(targets.iter())
            .filter(|(scope_string, _)| manages(scope_string))
            .map(|(_, (level, label, target))| ScopeOption {
                level: *level,
                label: label.clone(),
                target: target.clone(),
            })
            .collect();
        let at_scope = |scope_string: &str| {
            chain
                .iter()
                .zip(targets.iter())
                .find(|(candidate, _)| candidate.as_str() == scope_string)
                .map(|(_, (_, label, target))| (label.clone(), target.clone()))
        };
        // Every point the resolver published here, plus every point with a stored row on the chain (a fresh write
        // shows before the worker re-resolves; its state then reads from the next refresh).
        let published = self.view.points_at(&petal.petal_id).unwrap_or_default();
        let mut points: BTreeSet<BindingPoint> = published
            .iter()
            .map(|point| BindingPoint::new(point.hook.clone(), point.key.clone()))
            .collect();
        points.extend(
            rows.iter()
                .filter(|row| chain.contains(&row.record.scope))
                .map(|row| row.record.point()),
        );
        let point_rows = points
            .into_iter()
            .map(|point| {
                let resolution = published
                    .iter()
                    .find(|candidate| candidate.hook == point.hook && candidate.key == point.key)
                    .and_then(|candidate| candidate.resolution.clone());
                let winner = chain
                    .iter()
                    .find_map(|scope_string| effective_binding(&rows, &roles, scope_string, &point).cloned());
                match winner {
                    Some(record) => {
                        let (scope_label, target) = at_scope(&record.scope).unzip();
                        BindingPointRow {
                            hook: point.hook,
                            key: point.key,
                            scope_label: scope_label.unwrap_or_default(),
                            target,
                            mode: record.mode,
                            package_id: record.package_id,
                            version_req: record.version_req,
                            config: record.config,
                            runs_on: record.runs_on,
                            resolution,
                        }
                    }
                    None => BindingPointRow {
                        hook: point.hook,
                        key: point.key,
                        scope_label: String::new(),
                        target: None,
                        mode: BindingMode::Inherit,
                        package_id: None,
                        version_req: None,
                        config: json!({}),
                        runs_on: None,
                        resolution,
                    },
                }
            })
            .collect();
        Ok(BindingsSnapshot {
            verse_id: petal.verse_id.clone(),
            petal_label: named("Petal", &scope.petal_name),
            can_manage: manages(&chain[0]),
            local_is_owner,
            scopes,
            points: point_rows,
            providers: Vec::new(),
        })
    }

    fn write_binding(&self, template: BindingRecord) -> Result<(), String> {
        let verse_id = fe_database::parse_scope(&template.scope)
            .map(|parts| parts.verse_id)
            .map_err(|error| format!("invalid-input: {error}"))?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        // `caller_did: None` is the GUI: the DB plane judges the local DID (the row's author) at the scope.
        let command = DbCommand::PutBinding {
            caller_did: None,
            binding: template,
            reply,
        };
        self.db
            .send_timeout(command, DB_SEND_TIMEOUT)
            .map_err(|_| "the database is not accepting commands".to_string())?;
        receiver
            .recv_timeout(DB_REPLY_TIMEOUT)
            .map_err(|_| "the database did not answer; the binding may or may not be written".to_string())?
            .map_err(|refusal: BindingRefusal| format!("{}: {}", refusal.code, refusal.message))?;
        self.signals.dirty(&verse_id, Trigger::BindingRow);
        Ok(())
    }

    fn stage_launch(
        &self,
        build: &BuildToRun<'_>,
        scope: &ActiveScope,
    ) -> Result<StagedInstance, String> {
        let petal = self
            .stager
            .petals
            .get(&scope.petal_id)
            .filter(|petal| petal.verse_id == scope.verse_id)
            .ok_or("the open petal is not in the loaded hierarchy yet")?;
        let staged = self.stager.stage(&StageRequest {
            plugin_id: build.installation_id,
            installation_id: build.installation_id,
            package: build.package,
            facts: build.facts,
            petal: &petal,
            first_activation: build.first_activation,
        })?;
        self.signals.dirty(&scope.verse_id, Trigger::Lifecycle);
        Ok(staged)
    }

    fn counters(&self, installation_id: &str) -> Vec<(String, String)> {
        let Some(row) = self.index.row(installation_id) else {
            return Vec::new();
        };
        let totals = tick_totals(&self.stager.ticks, &row.instances);
        let bus = self
            .stager
            .bus
            .counters_for_installation(installation_id)
            .unwrap_or_default();
        let net = self
            .stager
            .net
            .counters_for_installation(installation_id)
            .unwrap_or_default();
        [
            ("Ticks run", totals.ticks_run),
            ("Ticks skipped", totals.ticks_skipped),
            ("Commits", totals.commits),
            ("Refusals", totals.refusals),
            ("Unknown outcomes", totals.unknown_outcomes),
            ("Ingress frames", net.ingress_frames),
            ("Egress requests", net.egress_requests),
            ("Bus events published", bus.published),
            ("Commands answered", bus.commands_ok),
        ]
        .into_iter()
        .map(|(label, value)| (label.to_string(), value.to_string()))
        .collect()
    }

    fn audit(&self, installation_id: &str) -> Vec<AuditLine> {
        self.governance
            .audit_rows(installation_id, DETAILS_AUDIT_LINES)
            .unwrap_or_default()
            .iter()
            .map(|row| {
                let text = |name: &str| {
                    row.get(name)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let outcome = text("outcome");
                let action = text("action");
                AuditLine {
                    at: text("at"),
                    actor: text("actor_did"),
                    action: if outcome == "ok" {
                        action
                    } else {
                        format!("{action} ({outcome})")
                    },
                }
            })
            .collect()
    }
}

/// Summed tick counters of an installation's instances.
#[derive(Default)]
struct TickTotals {
    ticks_run: u64,
    ticks_skipped: u64,
    flushes: u64,
    commits: u64,
    refusals: u64,
    unknown_outcomes: u64,
}

fn tick_totals(ticks: &TickHost, instances: &[RunningInstance]) -> TickTotals {
    instances
        .iter()
        .filter_map(|instance| ticks.counters(&instance.plugin_id))
        .fold(TickTotals::default(), |mut totals, counters| {
            totals.ticks_run += counters.ticks_run;
            totals.ticks_skipped += counters.ticks_skipped;
            totals.flushes += counters.flushes;
            totals.commits += counters.commits;
            totals.refusals += counters.refusals;
            totals.unknown_outcomes += counters.unknown_outcomes;
            totals
        })
}

// ---------------------------------------------------------------------------
// fe-api seams: ExtensionHost and BindingHost
// ---------------------------------------------------------------------------

/// Everything the API hosts read; cheap to clone.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionHostParts {
    local_did: String,
    bus: Bus,
    net: Net,
    runtime: Option<tokio::runtime::Handle>,
    ticks: TickHost,
    governance: ExtensionGovernance,
    manager: Option<Arc<Mutex<PluginManager>>>,
    index: InstallationIndex,
    view: BindingView,
    live: LiveInstances,
    roles: Arc<dyn RoleSource>,
    audit: Arc<dyn AuditSink>,
    signals: Option<BindingSignals>,
}

impl ExtensionHostParts {
    /// Run `task` on the caller's runtime, else the network runtime, else a short-lived thread.
    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(handle) = tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| self.runtime.clone())
        {
            handle.spawn(task);
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("fe-ext-forward".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(task),
                    Err(error) => tracing::warn!(%error, "extension forward runtime did not start"),
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "extension forward thread did not start");
        }
    }

    fn signal(&self, signal: BindingSignal) {
        if let Some(signals) = &self.signals {
            signals.send(signal);
        }
    }
}

/// [`ExtensionHost`] over the bus, the network broker, governance and the plugin manager.
pub(crate) struct BusExtensionHost {
    parts: ExtensionHostParts,
}

impl BusExtensionHost {
    fn bus_caller(&self, caller: &ApiCaller) -> Caller {
        let kind = match caller.kind {
            ApiCallerKind::Rest => CallerKind::Rest,
            ApiCallerKind::Mcp => CallerKind::Mcp,
        };
        // The bus recomputes `local`; the token's max_role caps the node DID's Owner role (G1 security fix).
        Caller {
            did: caller.did.clone(),
            kind,
            package_id: None,
            local: caller.did == self.parts.bus.local_did()
                && caller.role_ceiling.is_at_least(RoleLevel::Owner),
            role_ceiling: Some(caller.role_ceiling),
        }
    }

    /// Role floor first, capped by the token's ceiling (the network broker sees no ceiling); refusal is the uniform 404.
    fn ingress_gate(
        &self,
        caller_did: &str,
        ceiling: RoleLevel,
        petal_id: &str,
        package_id: &str,
        endpoint: &str,
        websocket: bool,
    ) -> Result<(), IngressRefusal> {
        let key = InstanceKey::new(petal_id.to_string(), package_id.to_string());
        let floor = self
            .parts
            .live
            .manifest_at(&key)
            .and_then(|manifest| endpoint_floor(&manifest, endpoint, websocket))
            .ok_or(IngressRefusal::NotFound)?;
        if capped(self.parts.roles.role_at(caller_did, petal_id), ceiling).is_at_least(floor) {
            return Ok(());
        }
        self.parts.audit.record(AuditEntry {
            action: AuditAction::Denial,
            actor_did: caller_did.to_string(),
            caller_kind: None,
            installation_id: None,
            target: format!("{package_id}/ingress/{endpoint}"),
            args_digest: None,
            outcome: "not-found".into(),
            at_ms: now_ms(),
        });
        Err(IngressRefusal::NotFound)
    }

    fn status_doc(&self, installation_id: &str, row: &IndexRow) -> Value {
        let ticks = &self.parts.ticks;
        let totals = tick_totals(ticks, &row.instances);
        let bus = self.parts.bus.counters_for_installation(installation_id);
        let net = self.parts.net.counters_for_installation(installation_id);
        let any_degraded = row.instances.iter().any(|instance| instance.degraded);
        let any_running = row.instances.iter().any(|instance| !instance.degraded);
        let state = if any_running {
            "active"
        } else if any_degraded {
            "degraded"
        } else if row.revoked {
            "revoked"
        } else if row.disabled {
            "disabled"
        } else {
            "installed"
        };
        let instances: Vec<Value> = row
            .instances
            .iter()
            .map(|instance| {
                json!({
                    "instance_id": instance.plugin_id,
                    "verse_id": instance.verse_id,
                    "petal_id": instance.petal_id,
                    "content_hash": instance.content_hash,
                    "origin": if instance.origin == InstanceOrigin::Binding { "binding" } else { "local-bind" },
                    "degraded": instance.degraded,
                })
            })
            .collect();
        let commands_refused: u64 = bus
            .as_ref()
            .map_or(0, |counters| counters.commands_refused.values().sum());
        // Host-wide binding counts (`bindings.resolved`, `bindings.unresolved.<state>`).
        let bindings: serde_json::Map<String, Value> = self
            .parts
            .view
            .status_rows()
            .into_iter()
            .map(|(name, count)| (name, json!(count)))
            .collect();
        json!({
            "installation_id": installation_id,
            "package_id": row.package_id,
            "version": row.version,
            "name": row.name,
            "state": state,
            "consented": row.consented,
            "instances": instances,
            "counters": {
                "ticks_run": totals.ticks_run,
                "ticks_skipped": totals.ticks_skipped,
                "flushes": totals.flushes,
                "commits": totals.commits,
                "refusals": totals.refusals,
                "unknown_outcomes": totals.unknown_outcomes,
                "ingress_frames": net.as_ref().map_or(0, |counters| counters.ingress_frames),
                "commands_ok": bus.as_ref().map_or(0, |counters| counters.commands_ok),
                "commands_refused": commands_refused,
            },
            "bus": bus,
            "net": net,
            "bindings": bindings,
        })
    }

    fn admin_audit(
        &self,
        action: PolicyAction,
        target: &str,
        installation: Option<&str>,
        outcome: PolicyOutcome,
        reason: Option<String>,
    ) {
        self.parts.governance.append_audit(PolicyEntry {
            action,
            installation: installation.map(str::to_string),
            package_id: None,
            content_hash: None,
            actor_did: self.parts.local_did.clone(),
            target: target.to_string(),
            outcome,
            reason,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: String::new(),
        });
    }
}

fn host_refusal(refusal: BusRefusal) -> HostRefusal {
    match refusal {
        BusRefusal::NotFound => HostRefusal::NotFound,
        BusRefusal::NotAvailable(detail) => HostRefusal::NotAvailable(detail),
        BusRefusal::CapabilityDenied(detail) => HostRefusal::CapabilityDenied(detail),
        BusRefusal::RateLimited(detail) => HostRefusal::RateLimited(detail),
        BusRefusal::InvalidInput(detail) => HostRefusal::InvalidInput(detail),
        BusRefusal::LimitReached(detail) => HostRefusal::LimitReached(detail),
    }
}

fn ingress_refusal(refusal: NetRefusal) -> IngressRefusal {
    match refusal {
        NetRefusal::NotFound => IngressRefusal::NotFound,
        NetRefusal::TooManyRequests => IngressRefusal::TooManyRequests,
        NetRefusal::PayloadTooLarge => IngressRefusal::PayloadTooLarge,
        NetRefusal::MethodNotAllowed => IngressRefusal::MethodNotAllowed,
        NetRefusal::UnsupportedMediaType => IngressRefusal::UnsupportedMediaType,
    }
}

fn command_failure(outcome: CommandOutcome) -> Result<String, CommandFailure> {
    match outcome {
        CommandOutcome::Ok(text) => Ok(text),
        CommandOutcome::Guest(CommandError::InvalidInput(detail)) => {
            Err(CommandFailure::InvalidInput(detail))
        }
        CommandOutcome::Guest(CommandError::Refused(detail)) => {
            Err(CommandFailure::Refused(detail))
        }
        CommandOutcome::Guest(CommandError::Failed(detail)) => Err(CommandFailure::Failed(detail)),
        CommandOutcome::Host(HostError::UnknownOutcome(detail)) => {
            Err(CommandFailure::UnknownOutcome(detail))
        }
        CommandOutcome::Host(HostError::NotAvailable(detail)) => {
            Err(CommandFailure::NotAvailable(detail))
        }
        CommandOutcome::Host(other) => Err(CommandFailure::Failed(other.to_string())),
    }
}

fn admin_refusal(code: &str, message: impl Into<String>) -> AdminRefusal {
    AdminRefusal {
        code: code.to_string(),
        message: message.into(),
    }
}

impl ExtensionHost for BusExtensionHost {
    fn local_did(&self) -> String {
        self.parts.bus.local_did().to_string()
    }

    fn admit_remote(&self, did: &str) -> bool {
        let admitted = self.parts.bus.admit_remote(did);
        if admitted {
            // The bus role view learns this DID's grants at every known petal.
            self.parts.governance.watch_did(did);
        }
        admitted
    }

    fn list(&self) -> Vec<Value> {
        self.parts
            .index
            .rows()
            .iter()
            .map(|(id, row)| self.status_doc(id, row))
            .collect()
    }

    fn status(&self, installation: &str) -> Option<Value> {
        let row = self.parts.index.row(installation)?;
        Some(self.status_doc(installation, &row))
    }

    fn audit(&self, installation: &str, limit: usize) -> Option<Vec<Value>> {
        self.parts
            .governance
            .audit_rows(installation, limit.min(MAX_AUDIT_ROWS))
    }

    fn administer(&self, actor_did: &str, op: AdminOp) -> Result<Value, AdminRefusal> {
        if self.parts.local_did.is_empty() || actor_did != self.parts.local_did {
            return Err(admin_refusal(
                "not-local-identity",
                "only the host's local identity administers extensions",
            ));
        }
        if op.verb == AdminVerb::KillListOverride {
            let recorded = self
                .parts
                .governance
                .kill_list_override(actor_did, &op.params)?;
            self.parts
                .signal(BindingSignal::AllDirty(Trigger::KillList));
            return Ok(recorded);
        }
        let manager = self.parts.manager.as_ref().ok_or_else(|| {
            admin_refusal("host-off", "the plugin manager did not start on this host")
        })?;
        match op.verb {
            AdminVerb::DisableAll => {
                let stopped = lock(manager).disable_all();
                self.admin_audit(PolicyAction::Disable, "*", None, PolicyOutcome::Ok, None);
                self.parts.signal(BindingSignal::AllDirty(Trigger::Lifecycle));
                Ok(json!({ "disabled_all": true, "stopped": stopped }))
            }
            AdminVerb::EnableAll => {
                lock(manager).enable_all();
                self.admin_audit(PolicyAction::Enable, "*", None, PolicyOutcome::Ok, None);
                self.parts.signal(BindingSignal::AllDirty(Trigger::Lifecycle));
                Ok(json!({ "disabled_all": false }))
            }
            AdminVerb::Disable => {
                let installation = op
                    .installation
                    .as_deref()
                    .ok_or_else(|| admin_refusal("invalid-input", "disable needs an installation"))?;
                let stopped = lock(manager).disable_instance(installation);
                self.admin_audit(PolicyAction::Disable, installation, Some(installation), PolicyOutcome::Ok, None);
                self.parts.signal(BindingSignal::AllDirty(Trigger::Lifecycle));
                Ok(json!({ "installation_id": installation, "stopped": stopped }))
            }
            verb => Err(admin_refusal(
                "not-available",
                format!("{} runs through the Extensions panel on this host (it needs the consent dialog)", verb.as_str()),
            )),
        }
    }

    fn record_denied_admin(&self, actor_did: &str, target: &str) {
        self.parts.audit.record(AuditEntry {
            action: AuditAction::Denial,
            actor_did: actor_did.to_string(),
            caller_kind: None,
            installation_id: None,
            target: target.to_string(),
            args_digest: None,
            outcome: "non-local-admin".into(),
            at_ms: now_ms(),
        });
    }

    fn call_command(
        &self,
        caller: &ApiCaller,
        petal_id: &str,
        package_id: &str,
        command: &str,
        args: &Value,
    ) -> Result<CommandReply, HostRefusal> {
        let ticket = self
            .parts
            .bus
            .call_command(
                &self.bus_caller(caller),
                petal_id,
                package_id,
                command,
                args,
            )
            .map_err(host_refusal)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let outcome = ticket.reply;
        self.parts.spawn(async move {
            let result = match outcome.await {
                Ok(outcome) => command_failure(outcome),
                Err(_) => Err(CommandFailure::UnknownOutcome(
                    "the command reply was dropped".into(),
                )),
            };
            let _ = tx.send(result);
        });
        Ok(rx)
    }

    fn ingress(
        &self,
        call: IngressCall,
    ) -> Result<tokio::sync::oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal> {
        self.ingress_gate(
            &call.caller_did,
            call.role_ceiling,
            &call.petal_id,
            &call.package_id,
            &call.endpoint,
            false,
        )?;
        let receiver = self
            .parts
            .net
            .accept_http(HttpIngressRequest {
                petal_id: call.petal_id,
                package_id: call.package_id,
                endpoint: call.endpoint,
                caller_did: call.caller_did,
                method: call.method,
                sub_path: call.sub_path,
                query: call.query,
                headers: call.headers,
                content_type: call.content_type,
                body: call.body,
            })
            .map_err(ingress_refusal)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.parts.spawn(async move {
            // A dropped guest answer (instance stopped) drops `tx`: fe-api answers it as unavailable, never 202.
            if let Ok(answer) = receiver.await {
                let _ = tx.send(answer.map(|reply: IngressReply| IngressAnswer {
                    status: reply.status,
                    content_type: reply.content_type,
                    body: reply.body,
                }));
            }
        });
        Ok(rx)
    }

    fn open_websocket(&self, call: WsAcceptCall) -> Result<WsHostSession, IngressRefusal> {
        self.ingress_gate(
            &call.caller_did,
            call.role_ceiling,
            &call.petal_id,
            &call.package_id,
            &call.endpoint,
            true,
        )?;
        let session = self
            .parts
            .net
            .open_websocket(WsAcceptRequest {
                petal_id: call.petal_id,
                package_id: call.package_id,
                endpoint: call.endpoint,
                caller_did: call.caller_did,
            })
            .map_err(ingress_refusal)?;
        let connection = session.connection;
        let mut outbound = session.outbound;
        let (tx, rx) = tokio::sync::mpsc::channel(fe_plugin::net::OUTBOUND_QUEUE);
        self.parts.spawn(async move {
            while let Some(out) = outbound.recv().await {
                let mapped = match out {
                    WsOutbound::Frame(WsFrame::Text(text)) => WsOut::Frame(WsData::Text(text)),
                    WsOutbound::Frame(WsFrame::Binary(bytes)) => {
                        WsOut::Frame(WsData::Binary(bytes))
                    }
                    WsOutbound::Close { code, reason } => WsOut::Close { code, reason },
                };
                if tx.send(mapped).await.is_err() {
                    break;
                }
            }
        });
        Ok(WsHostSession {
            connection,
            outbound: rx,
        })
    }

    fn websocket_frame(&self, connection: u64, frame: WsData) {
        let frame = match frame {
            WsData::Text(text) => WsFrame::Text(text),
            WsData::Binary(bytes) => WsFrame::Binary(bytes),
        };
        self.parts.net.websocket_frame(connection, frame);
    }

    fn websocket_closed(&self, connection: u64) {
        self.parts.net.websocket_closed(connection);
    }

    fn open_client(&self, caller: &ApiCaller) -> (u64, Arc<tokio::sync::Notify>) {
        let bus_caller = self.bus_caller(caller);
        let handle = self.parts.bus.open_client_capped(
            &caller.did,
            bus_caller.kind,
            bus_caller.role_ceiling,
            ClientGrants::Api,
        );
        (handle.id, handle.notify)
    }

    fn client_subscribe(
        &self,
        client: u64,
        petal_id: &str,
        source: &str,
        params: &Value,
    ) -> Result<u64, HostRefusal> {
        self.parts
            .bus
            .client_subscribe(client, petal_id, source, params)
            .map_err(host_refusal)
    }

    fn client_unsubscribe(&self, client: u64, subscription: u64) {
        self.parts.bus.client_unsubscribe(client, subscription);
    }

    fn take_client_events(&self, client: u64, max: usize) -> Vec<String> {
        self.parts
            .bus
            .take_client_events(client, max)
            .iter()
            .map(|event| event.envelope_text())
            .collect()
    }

    fn close_client(&self, client: u64) {
        self.parts.bus.close_client(client);
    }

    fn verse_policy(&self, verse_id: &str) -> AdminReply {
        self.parts.governance.verse_policy(verse_id)
    }

    fn set_verse_policy(&self, actor_did: &str, verse_id: &str, policy: Value) -> AdminReply {
        self.parts
            .governance
            .set_verse_policy(actor_did, verse_id, policy)
    }
}

/// The binding plane as Bevy systems, fe-api and the terrain bridge reach it; cheap to clone.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionBindings {
    local_did: String,
    db: Sender<DbCommand>,
    bus: Bus,
    roles: Arc<dyn RoleSource>,
    governance: ExtensionGovernance,
    view: BindingView,
    petals: PetalDirectory,
    signals: BindingSignals,
}

impl ExtensionBindings {
    /// Ask the worker to call the instance serving `point` at `petal_id` (lazily started); poll the receiver.
    pub(crate) fn call_hook(
        &self,
        petal_id: &str,
        point: BindingPoint,
        input: Value,
    ) -> Receiver<HookAnswer> {
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.signals.send(BindingSignal::Call {
            petal_id: petal_id.to_string(),
            point,
            input,
            reply,
        });
        receiver
    }

    /// `petal.map` at `petal_id` as `PetalMapResolver::decide` reads it; never blocks, never touches the activator.
    pub(crate) fn petal_map_need(&self, petal_id: &str) -> Need {
        self.view
            .need_at(petal_id, &BindingPoint::new("petal.map", ""))
    }

    /// This host's last resolution of every bound point at `petal_id` (local view; never replicated).
    pub(crate) fn effective_at(&self, petal_id: &str) -> Vec<EffectivePoint> {
        self.view.points_at(petal_id).unwrap_or_default()
    }

    /// Re-resolve a verse (a binding or role change the caller observed).
    pub(crate) fn mark_dirty(&self, verse_id: &str) {
        self.signals.dirty(verse_id, Trigger::BindingRow);
    }

    /// An installation was removed: drop what the activator holds for it.
    pub(crate) fn forget_installation(&self, installation_id: &str) {
        self.signals
            .send(BindingSignal::Forget(installation_id.to_string()));
    }
}

/// Wait for one crossbeam DB reply off the caller's thread and hand it to a oneshot.
fn forward_binding_reply<T: Send + 'static>(
    receiver: Receiver<Result<T, BindingRefusal>>,
    then: impl FnOnce(&Result<T, BindingRefusal>) + Send + 'static,
) -> tokio::sync::oneshot::Receiver<Result<T, BindingRefusal>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let work = move || {
        let outcome = receiver.recv_timeout(DB_REPLY_TIMEOUT).unwrap_or_else(|_| {
            Err(BindingRefusal {
                code: "failed".into(),
                message: "the database did not answer in time".into(),
            })
        });
        then(&outcome);
        let _ = tx.send(outcome);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(work);
        }
        Err(_) => {
            if let Err(error) = std::thread::Builder::new()
                .name("fe-ext-bindings-reply".into())
                .spawn(work)
            {
                tracing::warn!(%error, "binding reply thread did not start");
            }
        }
    }
    rx
}

fn refused_now<T>(message: &str) -> tokio::sync::oneshot::Receiver<Result<T, BindingRefusal>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(Err(BindingRefusal {
        code: "failed".into(),
        message: message.into(),
    }));
    rx
}

impl BindingHost for ExtensionBindings {
    fn local_did(&self) -> String {
        self.local_did.clone()
    }

    fn admit_remote(&self, did: &str) -> bool {
        let admitted = self.bus.admit_remote(did);
        if admitted {
            self.governance.watch_did(did);
        }
        admitted
    }

    fn put_binding(&self, caller_did: &str, template: BindingRecord) -> BindingWriteReply {
        let verse_id = fe_database::parse_scope(&template.scope)
            .ok()
            .map(|parts| parts.verse_id);
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let command = DbCommand::PutBinding {
            caller_did: Some(caller_did.to_string()),
            binding: template,
            reply,
        };
        if self.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return refused_now("the database is not accepting commands");
        }
        let signals = self.signals.clone();
        forward_binding_reply(receiver, move |outcome| {
            if let (Ok(_), Some(verse_id)) = (outcome, verse_id) {
                signals.dirty(&verse_id, Trigger::BindingRow);
            }
        })
    }

    fn list_bindings(&self, caller_did: &str, scope: &str) -> BindingListReply {
        let (reply, receiver) =
            crossbeam::channel::bounded::<Result<Vec<StoredBinding>, BindingRefusal>>(1);
        let command = DbCommand::ListBindings {
            scope: scope.to_string(),
            caller_did: Some(caller_did.to_string()),
            reply,
        };
        if self.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return refused_now("the database is not accepting commands");
        }
        forward_binding_reply(receiver, |_| {})
    }

    fn effective(&self, petal_scope: &str, caller_did: &str) -> Option<Vec<EffectivePoint>> {
        let parts = fe_database::parse_scope(petal_scope).ok()?;
        let petal_id = parts.petal_id?;
        if caller_did != self.local_did
            && !self
                .roles
                .role_at(caller_did, &petal_id)
                .is_at_least(RoleLevel::Viewer)
        {
            return None;
        }
        match self.view.points_at(&petal_id) {
            Some(points) => Some(points),
            None if self.petals.contains(&petal_id) => {
                self.signals.dirty(&parts.verse_id, Trigger::Navigation);
                Some(Vec::new())
            }
            None => None,
        }
    }
}

/// The extension host fe-api mounts (`ApiConfig::extension_host`, wired in `main.rs`).
pub(crate) fn api_extension_host(app: &App) -> Option<Arc<dyn ExtensionHost>> {
    let parts = app.world().get_resource::<ExtensionHostParts>()?;
    Some(Arc::new(BusExtensionHost {
        parts: parts.clone(),
    }))
}

/// The binding plane fe-api mounts (`ApiConfig::binding_host`, wired in `main.rs`).
pub(crate) fn api_binding_host(app: &App) -> Option<Arc<dyn BindingHost>> {
    let bindings = app.world().get_resource::<ExtensionBindings>()?;
    Some(Arc::new(bindings.clone()))
}

/// The `scene.*` gate's backend (BND-10a): the published cache answers `bound`, the binding worker runs calls,
/// augments stage straight into the serving instance's window.
struct SceneHooks {
    local_did: String,
    view: BindingView,
    live: LiveInstances,
    signals: BindingSignals,
}

impl SceneHookBackend for SceneHooks {
    fn local_did(&self) -> &str {
        &self.local_did
    }

    fn bound(&self, hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
        self.view.scene_binding(petal_id, hook.id())
    }

    fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender) {
        self.signals.send(BindingSignal::Scene { call, reply });
    }

    fn stage_augment(
        &self,
        petal_id: &str,
        package_id: &str,
        node_id: &str,
        ops: Vec<AugmentOp>,
    ) -> Result<(), String> {
        let key = InstanceKey::new(petal_id.to_string(), package_id.to_string());
        self.live.stage_augment(&key, node_id, ops)
    }
}

/// Present only when the plugin manager started (otherwise no gate: every write proceeds as built in).
#[derive(Resource, Clone)]
struct ExtensionSceneHooks(Arc<SceneHooks>);

/// The backend `main.rs` installs `fe_runtime::scene_hook_gate::SceneHookGate` over before the API serves.
pub(crate) fn scene_hook_backend(app: &App) -> Option<Arc<dyn SceneHookBackend>> {
    let hooks = app.world().get_resource::<ExtensionSceneHooks>()?;
    Some(hooks.0.clone())
}

// ---------------------------------------------------------------------------
// Bevy systems: hierarchy mirror, navigation, replicated rows, bound slots
// ---------------------------------------------------------------------------

/// Mirror the loaded hierarchy's petal addresses once a second; a verse seen or changed is re-resolved.
fn mirror_petals(
    verses: Option<Res<VerseManager>>,
    bindings: Res<ExtensionBindings>,
    mut last: Local<Option<Instant>>,
) {
    if last.is_some_and(|at| at.elapsed() < PETAL_MIRROR) {
        return;
    }
    *last = Some(Instant::now());
    let Some(verses) = verses else { return };
    let mut next = HashMap::new();
    for verse in &verses.verses {
        for fractal in &verse.fractals {
            for petal in &fractal.petals {
                next.insert(
                    petal.id.clone(),
                    PetalAddress {
                        verse_id: verse.id.clone(),
                        fractal_id: fractal.id.clone(),
                        petal_id: petal.id.clone(),
                    },
                );
            }
        }
    }
    for verse_id in bindings.petals.replace(next) {
        bindings.signals.dirty(&verse_id, Trigger::Navigation);
    }
}

/// The active petal changed: lazy points there start, the petal left starts its idle clock.
fn follow_navigation(
    nav: Option<Res<NavigationManager>>,
    bindings: Res<ExtensionBindings>,
    mut last: Local<Option<String>>,
) {
    let Some(nav) = nav else { return };
    if last.as_deref() == nav.active_petal_id.as_deref() {
        return;
    }
    *last = nav.active_petal_id.clone();
    let active = match (
        &nav.active_verse_id,
        &nav.active_fractal_id,
        &nav.active_petal_id,
    ) {
        (Some(verse_id), Some(fractal_id), Some(petal_id)) => Some(PetalAddress {
            verse_id: verse_id.clone(),
            fractal_id: fractal_id.clone(),
            petal_id: petal_id.clone(),
        }),
        _ => None,
    };
    bindings.signals.send(BindingSignal::Navigate(active));
}

/// Replicated binding, role and policy rows re-resolve their verse (bindings.md §5.6 triggers).
fn follow_binding_rows(mut results: MessageReader<DbResult>, bindings: Res<ExtensionBindings>) {
    for result in results.read() {
        match result {
            DbResult::ReplicatedRowApplied {
                verse_id,
                table,
                outcome: ReplicatedRowOutcome::Applied,
                ..
            } => {
                let trigger = if table == BINDING_TABLE {
                    Trigger::BindingRow
                } else if table == ROLE_TABLE {
                    Trigger::Role
                } else if table == POLICY_TABLE {
                    Trigger::Policy
                } else {
                    continue;
                };
                bindings.signals.dirty(verse_id, trigger);
            }
            DbResult::RoleAssigned { .. } | DbResult::RoleRevoked { .. } => {
                bindings
                    .signals
                    .send(BindingSignal::AllDirty(Trigger::Role));
            }
            _ => {}
        }
    }
}

/// Answers from the worker for the bound snippet cache, and what the cache last showed.
#[derive(Resource)]
struct BoundSlotFeed {
    answers: Receiver<SnippetAnswer>,
    generation: u64,
    petal_id: Option<String>,
}

/// Feed `BoundSlotsState` (B10c): resolution for the active petal, snippet calls out, answers in.
fn feed_bound_slots(
    slots: Option<ResMut<BoundSlotsState>>,
    nav: Option<Res<NavigationManager>>,
    bindings: Res<ExtensionBindings>,
    mut feed: ResMut<BoundSlotFeed>,
    time: Res<Time>,
) {
    let (Some(mut slots), Some(nav)) = (slots, nav) else {
        return;
    };
    let generation = bindings.view.generation();
    if generation != feed.generation || feed.petal_id.as_deref() != nav.active_petal_id.as_deref() {
        feed.generation = generation;
        feed.petal_id = nav.active_petal_id.clone();
        match nav.active_petal_id.as_deref() {
            Some(petal_id) => {
                let (panels, targets) = bindings.view.slots_at(petal_id);
                slots.set_resolution(petal_id, panels, targets);
            }
            None => slots.clear(),
        }
    }
    let requests = slots.take_requests();
    if let (false, Some(petal_id)) = (requests.is_empty(), nav.active_petal_id.as_ref()) {
        bindings.signals.send(BindingSignal::Snippets {
            petal_id: petal_id.clone(),
            requests,
        });
    }
    let now = time.elapsed_secs_f64();
    while let Ok(answer) = feed.answers.try_recv() {
        slots.store_snippet(&answer.target, &answer.label, &answer.text, now);
    }
}

/// Without a plugin manager every panel op answers `Failed`, so the Extensions panel never waits forever.
fn refuse_extension_ops(
    panel: Option<ResMut<ExtensionsPanelState>>,
    consent: Option<ResMut<ConsentState>>,
) {
    let (Some(mut panel), Some(mut consent)) = (panel, consent) else {
        return;
    };
    if panel.take_ops().is_empty() {
        return;
    }
    let message = "the extension host did not start on this machine; see the log".to_string();
    panel.apply_event(ExtensionEvent::Failed { message }, &mut consent);
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// `AllowDev` only in a debug build with the governance dev flag (read back from the trust index).
#[cfg(debug_assertions)]
fn unsigned_policy(allow_dev: bool) -> UnsignedPolicy {
    if allow_dev {
        UnsignedPolicy::AllowDev
    } else {
        UnsignedPolicy::Refuse
    }
}

/// Release builds never admit unsigned packages, whatever the flag says.
#[cfg(not(debug_assertions))]
fn unsigned_policy(_allow_dev: bool) -> UnsignedPolicy {
    UnsignedPolicy::Refuse
}

/// The shared source trust: every loaded root (revocation-capable included, R-REG), no shared index key.
fn source_trust(governance: &ExtensionGovernance, allow_dev: bool) -> SourceTrust {
    SourceTrust {
        is_trust_root: governance.trust_root_predicate(),
        index_signer: Arc::new(|_: &str| None),
        allow_unsigned_dev: allow_dev,
    }
}

/// One process-wide plugin manager on the worker's `TickHost`: the node DID, governance's gates and its very trust
/// roots (so a kill list verifies identically in both), fail closed on every gate.
fn build_manager(
    dir: &Path,
    commands: Sender<PluginCommand>,
    governance: &ExtensionGovernance,
    ticks: TickHost,
    contexts: PluginContexts,
    admission: bool,
) -> Result<PluginManager, String> {
    let engine = WasmEngine::new().map_err(|error| error.to_string())?;
    // A separate registry from the Bevy `PluginRegistry`: governance state lives here (F-TICK note).
    let registry = Arc::new(Mutex::new(PluginRegistry::new()));
    let allow_dev = governance.trust().tier_for(None).is_some();
    let manager = PluginManager::new(Arc::new(engine), dir.join(AOT_DIR), registry, commands)
        .map_err(|error| error.to_string())?
        .with_local_did(governance.local_did())
        .with_gates(governance.gates())
        .with_ticks(ticks)
        .with_contexts(contexts)
        .with_trust(
            governance.trust().roots().clone(),
            unsigned_policy(allow_dev),
        )
        .with_admission(admission);
    Ok(manager)
}

/// Build the bus (governance audit, real sources, local DID), governance, the network broker, the plugin host and
/// manager, the revocation pull, the ops drain and the binding worker (src/AGENTS.md §plugin-bridge).
pub(crate) fn install_with(app: &mut App, config: ExtensionBridgeConfig) {
    let db = app
        .world()
        .resource::<fe_runtime::app::DbCommandSender>()
        .0
        .clone();
    let local_did = config.local_did.clone();
    let verified = VerifiedBuilds::default();
    app.insert_resource(
        PluginContexts::default().with_factory(Arc::new(DbSceneFactory {
            sender: db.clone(),
            verified: verified.clone(),
        })),
    );

    let slot = ExtensionApiSlot::default();
    if let Some(api) = config.api_cmd_tx {
        slot.bind(api);
    }
    let roles = Arc::new(ExtensionRoles {
        local_did: local_did.clone(),
        grants: RwLock::new(HashMap::new()),
    });
    let policy = ExtensionExternalPolicy::default();
    app.insert_resource(policy.clone());
    app.insert_resource(ExtensionRoleView(roles.clone()));

    // Governance reads the role view and external policy just inserted; its sink is the bus's durable audit.
    let qa_root = fe_identity::file_fallback::qa_storage_root().ok().flatten();
    let governance_config = GovernanceConfig::from_env(local_did.clone(), qa_root.as_deref());
    let data_dir: PathBuf = governance_config.config_dir.clone();
    let governance = governance::install(app, governance_config);
    let audit = governance.audit_sink();
    let role_source: Arc<dyn RoleSource> = roles;
    let allow_dev = governance.trust().tier_for(None).is_some();

    let in_flight = Arc::new(AtomicUsize::new(0));
    let sources = BusSources {
        query: Some(Arc::new(GuardedQuerySource {
            api: slot.clone(),
            in_flight: in_flight.clone(),
            rate: Mutex::new(HashMap::new()),
        })),
        scene: Some(Arc::new(DbSceneSnapshots {
            api: slot.clone(),
            in_flight,
        })),
        roles: role_source.clone(),
        policy: Arc::new(policy),
        audit: audit.clone(),
    };
    // The bus shares the host clock; insert it so PluginHostPlugin reuses the same TickHost.
    let ticks = app
        .world()
        .get_resource::<TickHost>()
        .cloned()
        .unwrap_or_default();
    let bus = Bus::new(ticks.clock(), sources, local_did.clone());
    app.insert_resource(ticks.clone());
    app.insert_resource(fe_plugin::BusHandle(bus.clone()));
    app.insert_resource(ExtensionBusParts {
        bus: bus.clone(),
        roles: role_source.clone(),
        audit: audit.clone(),
    });
    app.insert_resource(slot.clone());
    let blobs = ExtensionBlobSlot::default();
    app.insert_resource(blobs.clone());

    // The network broker owns a small runtime for dial-outs and egress (net_sources builds inside it).
    let net_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("fe-ext-net")
        .enable_all()
        .build();
    let (net, runtime_handle) = match net_runtime {
        Ok(runtime) => {
            let net = {
                let _entered = runtime.enter();
                net_sources::build_net(ticks.clock(), role_source.clone(), audit.clone(), &data_dir)
            };
            let handle = runtime.handle().clone();
            app.insert_resource(ExtensionNetRuntime(runtime));
            (net, Some(handle))
        }
        Err(error) => {
            tracing::warn!(%error, "extension network runtime did not start: no dial-outs or egress");
            (
                net_sources::build_net(
                    ticks.clock(),
                    role_source.clone(),
                    audit.clone(),
                    &data_dir,
                ),
                None,
            )
        }
    };

    let (reading_tx, reading_rx) = crossbeam::channel::bounded::<ReadingEvent>(1024);
    app.insert_resource(ReadingFeed(reading_tx));
    spawn_reading_feed(bus.clone(), reading_rx);
    if let Some(changes) = config.scene_changes {
        spawn_scene_feed(bus.clone(), changes);
    }

    app.add_plugins(fe_plugin::PluginHostPlugin);
    // PluginHostPlugin re-inserted the TickHost bound to the shared contexts; every stage and stop uses that one.
    let ticks = app.world().resource::<TickHost>().clone();
    let contexts = app.world().resource::<PluginContexts>().clone();
    let commands = app
        .world()
        .resource::<fe_plugin::PluginCommandSender>()
        .0
        .clone();
    let admission = app
        .world()
        .get_resource::<fe_plugin::PackageAdmission>()
        .is_some_and(|admission| admission.0);

    let index = InstallationIndex::default();
    let live = LiveInstances::default();
    let manager = match build_manager(
        &data_dir,
        commands.clone(),
        &governance,
        ticks.clone(),
        contexts,
        admission,
    ) {
        Ok(manager) => Some(Arc::new(Mutex::new(manager))),
        Err(error) => {
            tracing::error!(%error, "plugin manager did not start: extensions stay off on this host");
            None
        }
    };

    let (signal_tx, signal_rx) = crossbeam::channel::unbounded::<BindingSignal>();
    let signals = BindingSignals {
        sender: signal_tx,
        backlog: Arc::new(AtomicUsize::new(0)),
    };
    let view = BindingView::default();
    let petals = PetalDirectory::default();
    let bindings = ExtensionBindings {
        local_did: local_did.clone(),
        db: db.clone(),
        bus: bus.clone(),
        roles: role_source.clone(),
        governance: governance.clone(),
        view: view.clone(),
        petals: petals.clone(),
        signals: signals.clone(),
    };
    app.insert_resource(ExtensionHostParts {
        local_did: local_did.clone(),
        bus: bus.clone(),
        net: net.clone(),
        runtime: runtime_handle,
        ticks: ticks.clone(),
        governance: governance.clone(),
        manager: manager.clone(),
        index: index.clone(),
        view: view.clone(),
        live: live.clone(),
        roles: role_source,
        audit,
        signals: manager.as_ref().map(|_| signals.clone()),
    });
    app.insert_resource(bindings);
    app.add_systems(
        Update,
        (mirror_petals, follow_navigation, follow_binding_rows),
    );

    let Some(manager) = manager else {
        app.add_systems(Update, refuse_extension_ops);
        return;
    };
    governance.attach_manager(Arc::new(ManagerControlHandle {
        manager: manager.clone(),
        signals: signals.clone(),
    }));
    let (pull, pull_handle) = revocation_pull::start(
        governance.clone(),
        Some(data_dir.join(revocation_pull::CACHE_DIR)),
        revocation_pull::interval_from_env(),
    );
    app.insert_resource(revocation_pull::RevocationPullService(Mutex::new(pull)));
    app.insert_resource(pull_handle);

    let components = match ComponentHost::new() {
        Ok(host) => Some(Arc::new(host)),
        Err(error) => {
            tracing::error!(%error, "extension component host did not start: no 2.0 instance can run");
            None
        }
    };
    let stager = InstanceStager {
        ticks: ticks.clone(),
        bus,
        net,
        components,
        commands,
        local_did: local_did.clone(),
        verified,
        petals: petals.clone(),
        live: live.clone(),
    };
    let owners = VerseOwners {
        api: slot.clone(),
        local_did: local_did.clone(),
        cache: Arc::default(),
    };

    // Accepted device payloads reach the decoder through the worker; the DB thread never waits.
    let payload_signals = signals.clone();
    let installed = set_accepted_payload_sink(Box::new(move |event: AcceptedPayloadEvent| {
        payload_signals.payload(DecodeJob {
            payload_id: event.payload_id,
            format_id: event.format_id,
            mime: event.mime,
            node_id: event.node_id,
            petal_id: event.petal_id,
            recorded_at_ms: event.recorded_at_ms,
            asset_ids: event.asset_ids,
        });
    }));
    if !installed {
        tracing::warn!("an accepted-payload sink was already installed; payload decode is off");
    }

    app.insert_resource(ExtensionSceneHooks(Arc::new(SceneHooks {
        local_did: local_did.clone(),
        view: view.clone(),
        live: live.clone(),
        signals: signals.clone(),
    })));

    let (answer_tx, answer_rx) = crossbeam::channel::unbounded::<SnippetAnswer>();
    app.insert_resource(BoundSlotFeed {
        answers: answer_rx,
        generation: 0,
        petal_id: None,
    });
    app.add_systems(Update, feed_bound_slots);

    let worker = BindingWorker {
        manager: manager.clone(),
        governance: governance.clone(),
        activator: BindingActivator::new(ticks.clock()),
        catalog: InstalledCatalog::new(&data_dir, governance.clone(), allow_dev),
        stager: stager.clone(),
        live,
        petals,
        db: db.clone(),
        view: view.clone(),
        index: index.clone(),
        reader: PayloadReader { api: slot, blobs },
        owners: owners.clone(),
        answers: answer_tx,
        backlog: signals.backlog.clone(),
        admission,
        local_did,
        snapshots: HashMap::new(),
        retry: HashMap::new(),
        acceptance_retry: Some(Instant::now()),
        last_sweep: Instant::now(),
        changed: true,
        running: Vec::new(),
    };
    let service = OwnedService::spawn("ext-bindings", move |stop| worker.run(signal_rx, stop));
    app.insert_resource(ExtensionBindingService(Mutex::new(Some(service))));

    // The ops drain builds its sources on its own thread (protocol-v1 sources own a private runtime).
    let ports = BridgeOpsPorts {
        governance: governance.clone(),
        stager,
        db,
        owners,
        index,
        signals,
        view,
    };
    let ops_trust = source_trust(&governance, allow_dev);
    let worker_trust = source_trust(&governance, allow_dev);
    app.add_plugins(ExtensionOpsPlugin::new(move || {
        let configs = load_source_configs(&data_dir.join(SOURCES_FILE)).unwrap_or_else(|reason| {
            tracing::warn!(%reason, "extension sources unreadable: Browse lists none");
            Vec::new()
        });
        let sources = SourceSet::build(&configs, &ops_trust);
        OpsWorker::new(
            manager,
            PluginPackageStore::new(data_dir.join(PACKAGES_DIR)),
            worker_trust,
            sources,
            Box::new(ports),
        )
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::manifest_v2::ExtensionSourceClass;

    fn admission() -> PluginAdmission {
        PluginAdmission::new(
            "org.example.fleet@petal-1".into(),
            "petal-1".into(),
            vec![
                fe_sdk::CAP_STORAGE_READ.into(),
                fe_sdk::CAP_STORAGE_WRITE.into(),
            ],
            Vec::new(),
            vec!["VERSE#v1".into()],
            vec!["VERSE#v1-FRACTAL#f1-PETAL#petal-1".into()],
        )
        .expect("admission")
    }

    fn grants(package_id: &str) -> V2Grants {
        V2Grants {
            package_id: package_id.into(),
            scene_read: true,
            scene_write: vec![SceneWrite::SetProperty],
            metrics: vec!["temperature".into()],
            readings_class: Some(ExtensionSourceClass::Device),
            routes: false,
            tick: None,
        }
    }

    /// G6-C6 at the bridge: a v2 admission is stamped only with the build this host verified under the context id.
    #[test]
    fn sealed_admission_stamps_only_the_verified_build() {
        let verified = VerifiedBuilds::default();
        let plugin_id = "org.example.fleet@petal-1";
        let hash = "a".repeat(64);

        let unstaged = sealed_admission(
            admission(),
            plugin_id,
            &grants("org.example.fleet"),
            &verified,
        );
        assert!(
            unstaged.is_err(),
            "no verified build: refused, never an unstamped admission"
        );

        verified.record(
            plugin_id,
            PackageIdentity::new("org.example.other".into(), hash.clone()).expect("identity"),
        );
        let swapped = sealed_admission(
            admission(),
            plugin_id,
            &grants("org.example.fleet"),
            &verified,
        );
        assert!(
            swapped.is_err(),
            "a verified build of another package never stamps this token"
        );

        verified.record(
            plugin_id,
            PackageIdentity::new("org.example.fleet".into(), hash.clone()).expect("identity"),
        );
        let stamped = sealed_admission(
            admission(),
            plugin_id,
            &grants("org.example.fleet"),
            &verified,
        )
        .expect("stamped");
        assert_eq!(
            stamped.package().map(PackageIdentity::content_hash),
            Some(hash.as_str())
        );
        assert!(stamped.allows_write(SceneWrite::SetProperty));
        assert!(
            !stamped.allows_write(SceneWrite::DeleteNode),
            "writes narrow to the declared kinds"
        );
        let readings = stamped.readings().expect("readings bound");
        assert_eq!(readings.source_class(), SourceClass::Device);
        assert!(readings.allows_metric("temperature") && !readings.allows_metric("pressure"));
    }
}
