use super::*;

// ---------------------------------------------------------------------------
// Scene DB backend: reads, 1.0 commits and tick flushes over the typed plugin commands
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(super) struct DbSceneBackend {
    pub(super) sender: Sender<DbCommand>,
    pub(super) admission: PluginAdmission,
}

/// A refusal as the sinks' callers read it: `UnknownOutcome` must classify as unknown in `ContextTickSink`.
pub(super) fn refusal_text(refusal: PluginRefusal) -> String {
    match refusal {
        PluginRefusal::UnknownOutcome(detail) => {
            format!("unknown outcome, requires reconciliation: {detail}")
        }
        other => other.to_string(),
    }
}

impl DbSceneBackend {
    /// One read, or one atomic commit of `ops` plus `readings` (pinned `DbCommand::PluginCommit`).
    pub(super) fn request(
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

    pub(super) fn require_petal(&self, petal_id: &str) -> Result<(), String> {
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
pub(super) struct VerifiedBuilds(pub(super) Arc<RwLock<HashMap<String, PackageIdentity>>>);

impl VerifiedBuilds {
    pub(super) fn record(&self, plugin_id: &str, identity: PackageIdentity) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(plugin_id.to_string(), identity);
    }

    pub(super) fn get(&self, plugin_id: &str) -> Option<PackageIdentity> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(plugin_id)
            .cloned()
    }
}

/// A v2 token admits only a build this host verified: stamp `{package_id, content_hash}`, narrow the write kinds and
/// bind the readings grant (G6-C6). No verified build fails closed.
pub(super) fn sealed_admission(
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

pub(super) struct DbSceneFactory {
    pub(super) sender: Sender<DbCommand>,
    pub(super) verified: VerifiedBuilds,
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
    /// The `blob.source` slot the sync thread and the `blob://` reader ask; the bridge installs and pumps the source.
    pub blob_fallback: Option<BlobFallbackSlot>,
    /// This device's X25519 key (R19 row 6 opens a licensed build's content key with it); `None` refuses licensed builds.
    pub device_key: Option<fe_identity::x25519::DeviceKeypair>,
    /// The node's Ed25519 identity key: the licensee (`sub`) a licence token request signs as; `None` refuses requests.
    pub identity_key: Option<ed25519_dalek::SigningKey>,
}

// ---------------------------------------------------------------------------
// Portal link: Disable all and the admitted bundle set (R-X-33, R-X-04)
// ---------------------------------------------------------------------------

/// The portal's governance view, linked once `portal_host::install` built it (after this bridge).
static PORTAL_VIEW: RwLock<Option<Arc<PortalGovernance>>> = RwLock::new(None);
/// The last Disable all state this bridge applied, replayed onto a view linked later.
static HOST_DISABLED: AtomicBool = AtomicBool::new(false);
/// Web-bundle consents seen before the portal linked: `(package_id, content_hash, capability_digest)`.
static PENDING_BUNDLE_CONSENTS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

pub(super) fn portal_view() -> Option<Arc<PortalGovernance>> {
    PORTAL_VIEW.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// `main.rs` / `portal_host::install`: the bridge now feeds `view` its Disable all state and web-bundle consents.
pub(crate) fn link_portal_resolver(view: Arc<PortalGovernance>) {
    if HOST_DISABLED.load(Ordering::Acquire) {
        view.set_host_disabled(true);
    }
    for (package_id, content_hash, digest) in lock(&PENDING_BUNDLE_CONSENTS).drain(..) {
        view.learn_consent(&package_id, &content_hash, &digest);
    }
    *PORTAL_VIEW.write().unwrap_or_else(PoisonError::into_inner) = Some(view);
}

/// Disable all (layer b) for every kill-switch reader: the plugin manager's caller already switched; this tells the portal.
pub(super) fn portal_host_switch(disabled: bool) {
    HOST_DISABLED.store(disabled, Ordering::Release);
    if let Some(view) = portal_view() {
        view.set_host_disabled(disabled);
    }
}

/// A web-bundle consent (hydrated or fresh) admits its exact pair at the portal.
pub(super) fn portal_learn_consent(record: &ConsentRecord) {
    match portal_view() {
        Some(view) => view.learn_consent(&record.package_id, &record.content_hash, &record.capability_digest),
        None => {
            let mut pending = lock(&PENDING_BUNDLE_CONSENTS);
            if pending.len() < MAX_PENDING_BUNDLE_CONSENTS {
                pending.push((
                    record.package_id.clone(),
                    record.content_hash.clone(),
                    record.capability_digest.clone(),
                ));
            }
        }
    }
}

/// `LocalFacts.admitted_bundles`: the portal's admitted pairs; empty until it links (portal.open fails closed).
pub(super) fn portal_admitted_bundles() -> BTreeSet<(String, String)> {
    portal_view().map(|view| view.admitted_set()).unwrap_or_default()
}

/// Late-bound API command channel; the bus sources answer `not-available` until it is set.
#[derive(Resource, Clone, Default)]
pub(crate) struct ExtensionApiSlot(pub(super) Arc<OnceLock<Sender<fe_runtime::messages::ApiCommand>>>);

impl ExtensionApiSlot {
    /// Bind once; later calls are ignored.
    pub(crate) fn bind(&self, sender: Sender<fe_runtime::messages::ApiCommand>) {
        let _ = self.0.set(sender);
    }

    pub(super) fn get(&self) -> Result<&Sender<fe_runtime::messages::ApiCommand>, HostError> {
        self.0
            .get()
            .ok_or_else(|| HostError::NotAvailable("the database bridge is not bound yet".into()))
    }
}

/// Late-bound blob store the decode path reads accepted payload files from.
#[derive(Resource, Clone, Default)]
pub(crate) struct ExtensionBlobSlot(pub(super) Arc<OnceLock<fe_database::BlobStoreHandle>>);

impl ExtensionBlobSlot {
    /// Bind once; later calls are ignored.
    pub(crate) fn bind(&self, store: fe_database::BlobStoreHandle) {
        let _ = self.0.set(store);
    }

    /// The bound store; `None` until the binary binds it.
    pub(crate) fn get(&self) -> Option<&fe_database::BlobStoreHandle> {
        self.0.get()
    }
}

/// Send readings the readings door accepted; a drain thread feeds `host/readings`.
#[derive(Resource, Clone)]
pub(crate) struct ReadingFeed(pub Sender<ReadingEvent>);

/// Role view the bus judges callers by: the local DID is Owner, every other DID needs a published grant.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionRoleView(pub(super) Arc<ExtensionRoles>);

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
pub(crate) struct ExtensionExternalPolicy(pub(super) Arc<RwLock<std::collections::HashSet<String>>>);

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

pub(super) struct ExtensionRoles {
    pub(super) local_did: String,
    pub(super) grants: RwLock<HashMap<(String, String), RoleLevel>>,
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
pub(super) const MAX_BUS_DB_IN_FLIGHT: usize = 8;
/// Deadline for a bus DB round trip (statement timeout plus slack, as REST).
pub(super) const BUS_DB_TIMEOUT: Duration =
    Duration::from_secs(fe_api::query_guard::STATEMENT_TIMEOUT_SECS + 1);
/// Largest `host/query` result in bytes (REST's cap).
pub(super) const QUERY_RESULT_BYTE_CAP: usize = 8 * 1024 * 1024;

/// Releases one in-flight slot on drop.
pub(super) struct InFlight(pub(super) Arc<AtomicUsize>);

impl InFlight {
    pub(super) fn acquire(counter: &Arc<AtomicUsize>) -> Option<Self> {
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
pub(super) fn db_rows_within(
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
pub(super) fn db_rows(
    api: &Sender<fe_runtime::messages::ApiCommand>,
    sql: String,
    vars: HashMap<String, Value>,
) -> Result<Vec<Value>, HostError> {
    db_rows_within(api, sql, vars, BUS_DB_TIMEOUT)
}

/// `host/query` over fe-api's guard core (`prepare_scoped_sql`) and the API DB channel.
pub(super) struct GuardedQuerySource {
    pub(super) api: ExtensionApiSlot,
    pub(super) in_flight: Arc<AtomicUsize>,
    pub(super) rate: Mutex<HashMap<String, (u32, Instant)>>,
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
pub(super) fn cap_rows(rows: Vec<Value>) -> QueryRows {
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
pub(super) struct DbSceneSnapshots {
    pub(super) api: ExtensionApiSlot,
    pub(super) in_flight: Arc<AtomicUsize>,
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
pub(super) fn spawn_scene_feed(
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
pub(super) fn spawn_reading_feed(bus: Bus, readings: Receiver<ReadingEvent>) {
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
    pub(crate) bus: Bus,
    pub(crate) roles: Arc<dyn RoleSource>,
    pub(crate) audit: Arc<dyn AuditSink>,
}

/// The runtime the network broker's dial-outs and egress run on; dropped with the world, never inside a task.
#[derive(Resource)]
pub(super) struct ExtensionNetRuntime(#[allow(dead_code)] pub(super) tokio::runtime::Runtime);

/// The binding worker thread, stopped with the other host services on exit.
#[derive(Resource)]
pub(crate) struct ExtensionBindingService(pub(crate) Mutex<Option<OwnedService>>);

/// Distinct provisional petals (imports being judged) held at once; past it a provision answers unavailable.
pub(super) const MAX_PROVISIONAL_PETALS: usize = 4;

/// A provisional petal's address and the holds (one per provision) keeping it.
pub(super) type ProvisionalHolds = HashMap<String, (PetalAddress, BTreeSet<u64>)>;

/// Petal id -> its `(verse, fractal, petal)` address, mirrored from the loaded hierarchy, overlaid with the
/// provisional petals imports are judged at (`src/AGENTS.md` §scene-hook-import); the mirror always wins.
#[derive(Clone, Default)]
pub(super) struct PetalDirectory(
    Arc<RwLock<HashMap<String, PetalAddress>>>,
    Arc<RwLock<ProvisionalHolds>>,
);

impl PetalDirectory {
    pub(super) fn get(&self, petal_id: &str) -> Option<PetalAddress> {
        let mirrored = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(petal_id)
            .cloned();
        mirrored.or_else(|| {
            self.1
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(petal_id)
                .map(|(address, _)| address.clone())
        })
    }

    pub(super) fn contains(&self, petal_id: &str) -> bool {
        self.get(petal_id).is_some()
    }

    pub(super) fn of_verse(&self, verse_id: &str) -> Vec<PetalAddress> {
        let mirrored = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let mut petals: Vec<PetalAddress> = mirrored
            .values()
            .filter(|petal| petal.verse_id == verse_id)
            .cloned()
            .collect();
        petals.extend(
            self.1
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|(petal_id, (address, _))| {
                    address.verse_id == verse_id && !mirrored.contains_key(*petal_id)
                })
                .map(|(_, (address, _))| address.clone()),
        );
        drop(mirrored);
        petals.sort_by(|left, right| left.petal_id.cmp(&right.petal_id));
        petals
    }

    /// Add one hold on a provisional petal. `Exists` when the id is a known petal at another address.
    pub(super) fn hold_provisional(&self, address: &PetalAddress, hold: u64) -> Result<(), ProvisionFailure> {
        let elsewhere = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&address.petal_id)
            .is_some_and(|mirrored| mirrored != address);
        if elsewhere {
            return Err(ProvisionFailure::Exists);
        }
        let mut provisional = self.1.write().unwrap_or_else(PoisonError::into_inner);
        let held = provisional.len();
        match provisional.get_mut(&address.petal_id) {
            Some((current, holds)) if *current == *address => {
                holds.insert(hold);
                Ok(())
            }
            Some(_) => Err(ProvisionFailure::Exists),
            None if held >= MAX_PROVISIONAL_PETALS => Err(ProvisionFailure::Unavailable(
                "too many petal imports are being judged; retry shortly".into(),
            )),
            None => {
                provisional.insert(
                    address.petal_id.clone(),
                    (address.clone(), BTreeSet::from([hold])),
                );
                Ok(())
            }
        }
    }

    /// Drop one hold; the verse to re-resolve once the petal's last hold is gone (an unknown hold is a no-op).
    pub(super) fn release_provisional(&self, petal_id: &str, hold: u64) -> Option<String> {
        let mut provisional = self.1.write().unwrap_or_else(PoisonError::into_inner);
        let (address, holds) = provisional.get_mut(petal_id)?;
        if !holds.remove(&hold) || !holds.is_empty() {
            return None;
        }
        let verse_id = address.verse_id.clone();
        provisional.remove(petal_id);
        Some(verse_id)
    }

    /// Replace the map; the verses whose petal set changed.
    pub(super) fn replace(&self, next: HashMap<String, PetalAddress>) -> BTreeSet<String> {
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

