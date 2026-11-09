//! Production plugin services over the typed DB channel; see src/AGENTS.md.

use bevy::prelude::{App, Resource};
use crossbeam::channel::Sender;
use fe_api::extensions::{
    AdminOp, AdminRefusal, ApiCaller, ApiCallerKind, CommandFailure, CommandReply, ExtensionHost, HostRefusal,
    IngressAnswer, IngressCall, IngressRefusal,
};
use fe_database::RoleLevel;
use fe_plugin::bus::sources::TracingAudit;
use fe_plugin::bus::{
    AuditAction, AuditEntry, AuditSink, Bus, BusPolicy, BusRefusal, BusSources, Caller, CallerKind, ClientGrants,
    CommandError, CommandOutcome, HostError, PreparedQuery, QueryRequest, QueryRows, QuerySource, ReadingEvent,
    RoleSource, SceneSnapshotSource,
};
use fe_plugin::scene_host::{PluginContexts, SceneBackendFactory};
use fe_plugin::{NodeQuery, NodeReader, NodeSnapshot, TransactionReceipt, TransactionSink};
use fe_runtime::messages::DbCommand;
use fe_runtime::plugin_host::{PluginAdmission, PluginRead, PluginReply};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct DbSceneBackend {
    sender: Sender<DbCommand>,
    admission: PluginAdmission,
}

impl DbSceneBackend {
    fn request(
        &self,
        read: Option<PluginRead>,
        ops: &[fe_plugin::transaction::PendingOp],
    ) -> Result<PluginReply, String> {
        self.admission.require_active()?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
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
                reply,
            },
        };
        self.sender
            .send_timeout(command, Duration::from_secs(1))
            .map_err(|_| "plugin DB command was not enqueued".to_string())?;
        let response = receiver
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| {
                if is_write {
                    "plugin commit outcome is unknown; reconcile before retrying".to_string()
                } else {
                    "plugin DB read reply timed out or disconnected".to_string()
                }
            })??;
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
        match self.request(Some(PluginRead::PetalScope), &[])? {
            PluginReply::Scope(scope) => Ok(scope),
            _ => Err("unrelated plugin scope reply".into()),
        }
    }
    fn get_node(&self, petal_id: &str, node_id: &str) -> Result<Option<NodeSnapshot>, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::Node(node_id.into())), &[])? {
            PluginReply::Node(node) => Ok(node),
            _ => Err("unrelated plugin node reply".into()),
        }
    }
    fn query_nodes(&self, petal_id: &str, query: &NodeQuery) -> Result<Vec<NodeSnapshot>, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::Nodes(query.clone())), &[])? {
            PluginReply::Nodes(nodes) => Ok(nodes),
            _ => Err("unrelated plugin query reply".into()),
        }
    }
}

impl TransactionSink for DbSceneBackend {
    fn commit(
        &mut self,
        petal_id: &str,
        ops: &[fe_plugin::transaction::PendingOp],
    ) -> Result<TransactionReceipt, String> {
        self.require_petal(petal_id)?;
        match self.request(None, ops)? {
            PluginReply::Committed(receipt) => Ok(receipt),
            _ => Err("unrelated plugin commit reply; outcome requires reconciliation".into()),
        }
    }
}

struct DbSceneFactory(Sender<DbCommand>);

impl SceneBackendFactory for DbSceneFactory {
    fn bind(
        &self,
        context: fe_plugin::context::PluginContext,
    ) -> Result<fe_plugin::context::PluginContext, String> {
        let token = context.capabilities();
        let admission = PluginAdmission::new(
            context.plugin_id().into(),
            context.petal_id().into(),
            token.capabilities.clone(),
            token.property_scope.clone(),
            token.verse_scope.clone(),
            token.petal_scope.clone(),
        )?
        .with_lease(context.revocation_lease());
        let mut backend = DbSceneBackend {
            sender: self.0.clone(),
            admission,
        };
        let scope = backend.petal_scope(context.petal_id())?;
        backend.admission = backend.admission.bind_scope(scope)?;
        let backend = Arc::new(Mutex::new(backend));
        Ok(context
            .with_node_reader(backend.clone())
            .with_transaction_sink(backend))
    }
}

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

/// Send readings the readings door accepted; a drain thread feeds `host/readings`.
#[derive(Resource, Clone)]
pub(crate) struct ReadingFeed(pub Sender<ReadingEvent>);

/// Role view the bus judges callers by: the local DID is Owner, every other DID needs a published grant.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionRoleView(Arc<ExtensionRoles>);

impl ExtensionRoleView {
    /// Replace one DID's role at a petal (`RoleLevel::None` removes it).
    pub(crate) fn set_role(&self, did: &str, petal_id: &str, role: RoleLevel) {
        let mut grants = self.0.grants.write().unwrap_or_else(|error| error.into_inner());
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
pub(crate) struct ExtensionExternalPolicy(Arc<RwLock<HashSet<String>>>);

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
        self.0.read().unwrap_or_else(|error| error.into_inner()).contains(verse_id)
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
        let grants = self.grants.read().unwrap_or_else(|error| error.into_inner());
        grants.get(&(did.to_string(), petal_id.to_string())).copied().unwrap_or(RoleLevel::None)
    }
}

/// Bus DB round trips allowed at once across query and snapshot sources.
const MAX_BUS_DB_IN_FLIGHT: usize = 8;
/// Deadline for a bus DB round trip (statement timeout plus slack, as REST).
const BUS_DB_TIMEOUT: Duration = Duration::from_secs(fe_api::query_guard::STATEMENT_TIMEOUT_SECS + 1);
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

/// One correlated RawQuery over the API channel, polled until the deadline.
fn db_rows(
    api: &Sender<fe_runtime::messages::ApiCommand>,
    sql: String,
    vars: HashMap<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>, HostError> {
    use fe_runtime::messages::{ApiCommand, DbResult};
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    api.send(ApiCommand::DbRequest {
        cmd: DbCommand::RawQuery { sql, vars, correlation_id: Some(correlation_id.clone()) },
        reply_tx,
    })
    .map_err(|_| HostError::NotAvailable("the database bridge is closed".into()))?;
    let deadline = Instant::now() + BUS_DB_TIMEOUT;
    loop {
        match reply_rx.try_recv() {
            Ok(DbResult::QueryResult { data, correlation_id: Some(echoed) }) if echoed == correlation_id => {
                return Ok(data)
            }
            Ok(DbResult::QueryFailed { error, correlation_id: echoed }) if echoed == correlation_id => {
                return Err(HostError::Backend(format!("query failed: {error}")))
            }
            Ok(DbResult::Error(error)) => return Err(HostError::Backend(format!("query failed: {error}"))),
            Ok(_) => return Err(HostError::Backend("query failed: uncorrelated reply".into())),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err(HostError::Backend("the database bridge dropped the request".into()))
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
        Ok(PreparedQuery { tables: prepared.tables, guarded_sql: prepared.sql })
    }

    fn run(&self, request: &QueryRequest, prepared: &PreparedQuery, reply: Sender<Result<QueryRows, HostError>>) {
        let api = match self.api.get() {
            Ok(api) => api.clone(),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let Some(slot) = InFlight::acquire(&self.in_flight) else {
            let _ = reply.send(Err(HostError::RateLimited("too many bus queries in flight".into())));
            return;
        };
        let sql = fe_api::query_guard::with_statement_timeout(&prepared.guarded_sql);
        let vars: HashMap<String, serde_json::Value> = request.vars.clone().into_iter().collect();
        let spawned = std::thread::Builder::new().name("fe-bus-query".into()).spawn(move || {
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
fn cap_rows(rows: Vec<serde_json::Value>) -> QueryRows {
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
    QueryRows { rows: kept, truncated }
}

/// `host/node-changes` snapshots: the petal's node rows through the same API DB channel.
struct DbSceneSnapshots {
    api: ExtensionApiSlot,
    in_flight: Arc<AtomicUsize>,
}

impl SceneSnapshotSource for DbSceneSnapshots {
    fn snapshot(&self, petal_id: &str, reply: Sender<Result<Vec<serde_json::Value>, HostError>>) {
        let api = match self.api.get() {
            Ok(api) => api.clone(),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let Some(slot) = InFlight::acquire(&self.in_flight) else {
            let _ = reply.send(Err(HostError::RateLimited("too many bus snapshots in flight".into())));
            return;
        };
        let mut vars = HashMap::new();
        vars.insert("pid".to_string(), serde_json::json!(petal_id));
        let sql = fe_api::query_guard::with_statement_timeout("SELECT * FROM node WHERE petal_id = $pid");
        let spawned = std::thread::Builder::new().name("fe-bus-snapshot".into()).spawn(move || {
            let _slot = slot;
            let _ = reply.send(db_rows(&api, sql, vars));
        });
        if let Err(error) = spawned {
            tracing::warn!(%error, "bus snapshot thread did not start");
        }
    }
}

/// Forward committed scene changes into the bus; a lagged receiver resyncs every subscription.
fn spawn_scene_feed(bus: Bus, mut changes: tokio::sync::broadcast::Receiver<fe_runtime::messages::SceneChange>) {
    use tokio::sync::broadcast::error::RecvError;
    let spawned = std::thread::Builder::new().name("fe-bus-scene-feed".into()).spawn(move || loop {
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
fn spawn_reading_feed(bus: Bus, readings: crossbeam::channel::Receiver<ReadingEvent>) {
    let spawned = std::thread::Builder::new().name("fe-bus-reading-feed".into()).spawn(move || {
        while let Ok(first) = readings.recv() {
            let mut batch = vec![first];
            batch.extend(readings.try_iter().take(fe_plugin::bus::READINGS_PER_EVENT - 1));
            bus.feed_readings(&batch);
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "bus reading feed did not start");
    }
}

/// [`ExtensionHost`] over the bus; lifecycle administration, ingress and status wait on governance and the net mount.
pub(crate) struct BusExtensionHost {
    bus: Bus,
    audit: Arc<dyn AuditSink>,
}

impl BusExtensionHost {
    /// The host fe-api's extension routes call.
    pub(crate) fn new(bus: Bus) -> Self {
        Self { bus, audit: Arc::new(TracingAudit) }
    }

    fn bus_caller(&self, caller: &ApiCaller) -> Caller {
        let kind = match caller.kind {
            ApiCallerKind::Rest => CallerKind::Rest,
            ApiCallerKind::Mcp => CallerKind::Mcp,
        };
        Caller { did: caller.did.clone(), kind, package_id: None, local: caller.did == self.bus.local_did() }
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

fn command_failure(outcome: CommandOutcome) -> Result<String, CommandFailure> {
    match outcome {
        CommandOutcome::Ok(text) => Ok(text),
        CommandOutcome::Guest(CommandError::InvalidInput(detail)) => Err(CommandFailure::InvalidInput(detail)),
        CommandOutcome::Guest(CommandError::Refused(detail)) => Err(CommandFailure::Refused(detail)),
        CommandOutcome::Guest(CommandError::Failed(detail)) => Err(CommandFailure::Failed(detail)),
        CommandOutcome::Host(HostError::UnknownOutcome(detail)) => Err(CommandFailure::UnknownOutcome(detail)),
        CommandOutcome::Host(HostError::NotAvailable(detail)) => Err(CommandFailure::NotAvailable(detail)),
        CommandOutcome::Host(other) => Err(CommandFailure::Failed(other.to_string())),
    }
}

/// Adapt the broker's outcome receiver to the API's reply type, off the caller's thread.
fn adapt_reply(ticket: fe_plugin::bus::CommandTicket) -> CommandReply {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let outcome = ticket.reply;
    let forward = async move {
        let result = match outcome.await {
            Ok(outcome) => command_failure(outcome),
            Err(_) => Err(CommandFailure::UnknownOutcome("the command reply was dropped".into())),
        };
        let _ = tx.send(result);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(forward);
        }
        Err(_) => {
            let spawned = std::thread::Builder::new().name("fe-bus-reply".into()).spawn(move || {
                match tokio::runtime::Builder::new_current_thread().enable_time().build() {
                    Ok(runtime) => runtime.block_on(forward),
                    Err(error) => tracing::warn!(%error, "bus reply runtime did not start"),
                }
            });
            if let Err(error) = spawned {
                tracing::warn!(%error, "bus reply thread did not start");
            }
        }
    }
    rx
}

impl ExtensionHost for BusExtensionHost {
    fn local_did(&self) -> String {
        self.bus.local_did().to_string()
    }

    fn admit_remote(&self, did: &str) -> bool {
        self.bus.admit_remote(did)
    }

    fn list(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }

    fn status(&self, _installation: &str) -> Option<serde_json::Value> {
        None
    }

    fn audit(&self, _installation: &str, _limit: usize) -> Option<Vec<serde_json::Value>> {
        None
    }

    fn administer(&self, _actor_did: &str, op: AdminOp) -> Result<serde_json::Value, AdminRefusal> {
        Err(AdminRefusal {
            code: "host-off".into(),
            message: format!("{} is not wired to a lifecycle store on this host", op.verb.as_str()),
        })
    }

    fn record_denied_admin(&self, actor_did: &str, target: &str) {
        self.audit.record(AuditEntry {
            action: AuditAction::Denial,
            actor_did: actor_did.to_string(),
            caller_kind: None,
            installation_id: None,
            target: target.to_string(),
            args_digest: None,
            outcome: "non-local-admin".into(),
            at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
        });
    }

    fn call_command(
        &self,
        caller: &ApiCaller,
        petal_id: &str,
        package_id: &str,
        command: &str,
        args: &serde_json::Value,
    ) -> Result<CommandReply, HostRefusal> {
        self.bus
            .call_command(&self.bus_caller(caller), petal_id, package_id, command, args)
            .map(adapt_reply)
            .map_err(host_refusal)
    }

    fn ingress(
        &self,
        _call: IngressCall,
    ) -> Result<tokio::sync::oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal> {
        Err(IngressRefusal::NotFound)
    }

    fn open_client(&self, caller: &ApiCaller) -> (u64, Arc<tokio::sync::Notify>) {
        let kind = self.bus_caller(caller).kind;
        let handle = self.bus.open_client(&caller.did, kind, ClientGrants::Api);
        (handle.id, handle.notify)
    }

    fn client_subscribe(
        &self,
        client: u64,
        petal_id: &str,
        source: &str,
        params: &serde_json::Value,
    ) -> Result<u64, HostRefusal> {
        self.bus.client_subscribe(client, petal_id, source, params).map_err(host_refusal)
    }

    fn client_unsubscribe(&self, client: u64, subscription: u64) {
        self.bus.client_unsubscribe(client, subscription);
    }

    fn take_client_events(&self, client: u64, max: usize) -> Vec<String> {
        self.bus.take_client_events(client, max).iter().map(|event| event.envelope_text()).collect()
    }

    fn close_client(&self, client: u64) {
        self.bus.close_client(client);
    }
}

/// The extension host fe-api mounts (`ApiConfig` field, wired in `main.rs`).
pub(crate) fn api_extension_host(app: &App) -> Option<Arc<dyn ExtensionHost>> {
    let bus = app.world().get_resource::<fe_plugin::BusHandle>()?;
    Some(Arc::new(BusExtensionHost::new(bus.0.clone())))
}

pub(crate) fn install(app: &mut App) {
    install_with(app, ExtensionBridgeConfig::default());
}

/// Build the scene factory and the integration bus (real sources, local DID, feeds), then add the plugin host.
pub(crate) fn install_with(app: &mut App, config: ExtensionBridgeConfig) {
    let sender = app
        .world()
        .resource::<fe_runtime::app::DbCommandSender>()
        .0
        .clone();
    app.insert_resource(PluginContexts::default().with_factory(Arc::new(DbSceneFactory(sender))));

    let slot = ExtensionApiSlot::default();
    if let Some(api) = config.api_cmd_tx {
        slot.bind(api);
    }
    let in_flight = Arc::new(AtomicUsize::new(0));
    let roles = Arc::new(ExtensionRoles { local_did: config.local_did.clone(), grants: RwLock::new(HashMap::new()) });
    let policy = ExtensionExternalPolicy::default();
    let sources = BusSources {
        query: Some(Arc::new(GuardedQuerySource {
            api: slot.clone(),
            in_flight: in_flight.clone(),
            rate: Mutex::new(HashMap::new()),
        })),
        scene: Some(Arc::new(DbSceneSnapshots { api: slot.clone(), in_flight })),
        roles: roles.clone(),
        policy: Arc::new(policy.clone()),
        audit: Arc::new(TracingAudit),
    };

    // The bus shares the host clock; insert it so PluginHostPlugin reuses the same TickHost.
    let ticks = app.world().get_resource::<fe_plugin::tick::TickHost>().cloned().unwrap_or_default();
    let bus = Bus::new(ticks.clock(), sources, config.local_did);
    app.insert_resource(ticks);
    app.insert_resource(fe_plugin::BusHandle(bus.clone()));
    app.insert_resource(slot);
    app.insert_resource(policy);
    app.insert_resource(ExtensionRoleView(roles));

    let (reading_tx, reading_rx) = crossbeam::channel::bounded::<ReadingEvent>(1024);
    app.insert_resource(ReadingFeed(reading_tx));
    spawn_reading_feed(bus.clone(), reading_rx);
    if let Some(changes) = config.scene_changes {
        spawn_scene_feed(bus, changes);
    }

    app.add_plugins(fe_plugin::PluginHostPlugin);
}
