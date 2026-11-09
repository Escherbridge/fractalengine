//! Portal IPC broker over the integration bus (P-S2, A-13): host-minted bindings, role ceilings, the one-op write path; see `src/AGENTS.md` §portal-host.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::prelude::{MessageReader, Res, Resource};
use crossbeam::channel::Sender;
use fe_database::RoleLevel;
use fe_format::portal_bundle::PortalSection;
use fe_plugin::bus::{
    AuditAction, AuditEntry, AuditSink, Bus, BusEvent, BusRefusal, CallerKind, CommandError, CommandOutcome,
    DeliveryKind as BusDelivery, HostError, PortalGrants, PortalStream, PortalWrite, PortalWriteSink, RoleSource,
    WriteOutcome,
};
use fe_runtime::messages::DbCommand;
use fe_runtime::plugin_host::{PackageIdentity, PluginAdmission, PluginRefusal, PluginResult};
use fe_sdk::manifest_v2::{ConsumeEntry, SceneWrite};
use fe_sdk::plugin_host::PendingOp;
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::plugin::LocalUserRole;
use fe_ui::verse_manager::VerseManager;
use fe_webview::ipc::BrowserCommand;
use fe_webview::plugin::PortalBrokerRes;
use fe_webview::portal_bundle::admission::{kill_list_status, GovernanceView, KillListStatus, RecordState};
use fe_webview::portal_bundle::limits::REPLY_DEADLINE;
use fe_webview::portal_bundle::{
    BindRequest, BrokerClient, BrokerEvent, BundleTarget, DeliveryKind as PageDelivery, IpcError, PendingReply,
    PortalBroker,
};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::sync::oneshot::error::TryRecvError;

/// The bus address of the scene feed; its events lose host-reserved keys before a page sees them.
const NODE_CHANGES: &str = "host/node-changes";
/// Pinned node scopes kept at most; past it the table starts over (Opens are user actions).
const MAX_PINNED_NODES: usize = 256;
/// Longest page-derived text an audit row keeps.
const MAX_AUDIT_TEXT_BYTES: usize = 128;

// ---------------------------------------------------------------------------
// Scope index: the binding's place in the hierarchy, pinned at a local Open
// ---------------------------------------------------------------------------

/// The verse, fractal and petal an Open's target sits in, pinned at Open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalScope {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
}

impl PortalScope {
    /// The stored full ancestry the DB pins plugin admissions to (`resolve_petal_scope_handler`).
    pub fn ancestry(&self) -> String {
        fe_database::build_scope(&self.verse_id, Some(&self.fractal_id), Some(&self.petal_id))
    }
}

/// The local identity's role at a petal as fe-ui resolved it (`LocalUserRole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalRole {
    /// The active petal, resolved.
    Resolved(RoleLevel),
    /// The active petal, still resolving.
    Pending,
    /// Not the active petal.
    Unknown,
}

#[derive(Debug, Clone)]
struct ActivePetal {
    scope: PortalScope,
    role: Option<RoleLevel>,
}

#[derive(Debug, Default)]
struct ScopeTable {
    pinned: HashMap<String, PortalScope>,
    active: Option<ActivePetal>,
}

/// Binding scopes for the broker, written by [`track_portal_scopes`] and read off the ECS at bind time.
#[derive(Resource, Clone, Default)]
pub struct PortalScopeIndex(Arc<RwLock<ScopeTable>>);

impl PortalScopeIndex {
    fn read(&self) -> RwLockReadGuard<'_, ScopeTable> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, ScopeTable> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Pin `node_id`'s scope at a local Open; `None` (the node is not in the tree) forgets it.
    pub fn record_open(&self, node_id: &str, scope: Option<PortalScope>) {
        let mut table = self.write();
        match scope {
            Some(scope) => {
                if table.pinned.len() >= MAX_PINNED_NODES && !table.pinned.contains_key(node_id) {
                    table.pinned.clear();
                }
                table.pinned.insert(node_id.to_owned(), scope);
            }
            None => {
                table.pinned.remove(node_id);
            }
        }
    }

    /// Mirror the active petal and the local identity's resolved role there; takes the write lock only on a change.
    pub fn set_active(
        &self,
        verse_id: Option<&str>,
        fractal_id: Option<&str>,
        petal_id: Option<&str>,
        role: Option<RoleLevel>,
    ) {
        let ids = verse_id.zip(fractal_id).zip(petal_id);
        let unchanged = match (&self.read().active, ids) {
            (None, None) => true,
            (Some(active), Some(((verse, fractal), petal))) => {
                active.role == role
                    && active.scope.verse_id == verse
                    && active.scope.fractal_id == fractal
                    && active.scope.petal_id == petal
            }
            _ => false,
        };
        if unchanged {
            return;
        }
        self.write().active = ids.map(|((verse, fractal), petal)| ActivePetal {
            scope: PortalScope {
                verse_id: verse.to_owned(),
                fractal_id: fractal.to_owned(),
                petal_id: petal.to_owned(),
            },
            role,
        });
    }

    /// The scope an Open target binds to: a pinned node, or a bound petal that is active or pinned.
    pub fn scope_of(&self, target: &BundleTarget) -> Option<PortalScope> {
        let table = self.read();
        match target {
            BundleTarget::Node(node_id) => table.pinned.get(node_id).cloned(),
            BundleTarget::Bound { petal_id, .. } => table
                .active
                .as_ref()
                .map(|active| &active.scope)
                .filter(|scope| scope.petal_id == *petal_id)
                .or_else(|| table.pinned.values().find(|scope| scope.petal_id == *petal_id))
                .cloned(),
        }
    }

    /// The local identity's role at `petal_id`; fe-ui resolves it for the active petal only.
    pub fn local_role(&self, petal_id: &str) -> LocalRole {
        match &self.read().active {
            Some(active) if active.scope.petal_id == petal_id => {
                active.role.map_or(LocalRole::Pending, LocalRole::Resolved)
            }
            _ => LocalRole::Unknown,
        }
    }
}

/// Update, before `fe_webview::plugin::apply_portal_admission`: pin each local Open's node scope and mirror the active
/// petal's resolved local role (per frame: one read-locked compare, no allocation).
pub fn track_portal_scopes(
    mut commands: MessageReader<BrowserCommand>,
    tree: Res<VerseManager>,
    nav: Res<NavigationManager>,
    local_role: Res<LocalUserRole>,
    index: Res<PortalScopeIndex>,
) {
    for command in commands.read() {
        if let BrowserCommand::OpenBundle { node_id } = command {
            index.record_open(node_id, node_scope(&tree, node_id));
        }
    }
    if nav.has_valid_ancestry() {
        index.set_active(
            nav.active_verse_id.as_deref(),
            nav.active_fractal_id.as_deref(),
            nav.active_petal_id.as_deref(),
            local_role.role,
        );
    } else {
        index.set_active(None, None, None, None);
    }
}

/// The verse, fractal and petal holding `node_id` in the in-memory tree.
fn node_scope(tree: &VerseManager, node_id: &str) -> Option<PortalScope> {
    let petal_id = tree.petal_id_of(node_id)?;
    tree.verses.iter().find_map(|verse| {
        verse.fractals.iter().find_map(|fractal| {
            fractal.petals.iter().any(|petal| petal.id == petal_id).then(|| PortalScope {
                verse_id: verse.id.clone(),
                fractal_id: fractal.id.clone(),
                petal_id: petal_id.to_owned(),
            })
        })
    })
}

// ---------------------------------------------------------------------------
// The broker
// ---------------------------------------------------------------------------

/// Governance's web-bundle installation id for `(package_id, content_hash)` (A-14), when it has one.
pub type InstallationLookup = dyn Fn(&str, &str) -> Option<String> + Send + Sync;

/// One open portal client: the host-minted binding (ipc-v1.md §6.5); nothing in it comes from page data.
#[derive(Debug, Clone)]
struct Binding {
    installing_did: String,
    package_id: String,
    content_hash: String,
    installation_id: Option<String>,
    scope: PortalScope,
    ceiling: RoleLevel,
    /// The client's `host/node-changes` subscriptions.
    node_subs: HashSet<u64>,
}

enum Ticket {
    Call(oneshot::Receiver<CommandOutcome>),
    Write(oneshot::Receiver<WriteOutcome>),
}

/// A bus ticket relayed to the bridge's polled reply channel.
struct Forward {
    client: u64,
    ticket: Ticket,
    reply: mpsc::Sender<Result<Value, IpcError>>,
    deadline: Instant,
}

impl Forward {
    /// The page answer once the bus settled the ticket, dropped it, or the bridge's deadline passed.
    fn settle(&mut self, now: Instant) -> Option<Result<Value, IpcError>> {
        let settled = match &mut self.ticket {
            Ticket::Call(ticket) => match ticket.try_recv() {
                Ok(outcome) => Some(command_reply(outcome)),
                Err(TryRecvError::Closed) => Some(Err(IpcError::UnknownOutcome)),
                Err(TryRecvError::Empty) => None,
            },
            Ticket::Write(ticket) => match ticket.try_recv() {
                Ok(outcome) => Some(write_reply(&outcome)),
                Err(TryRecvError::Closed) => Some(Err(IpcError::UnknownOutcome)),
                Err(TryRecvError::Empty) => None,
            },
        };
        settled.or_else(|| (now >= self.deadline).then_some(Err(IpcError::UnknownOutcome)))
    }
}

#[derive(Default)]
struct BrokerState {
    bindings: HashMap<u64, Binding>,
    forwards: Vec<Forward>,
}

/// The integration bus as the portal bridge's [`PortalBroker`] (bus/AGENTS.md §portal-clients); cheap to clone.
#[derive(Clone)]
pub struct BusPortalBroker {
    bus: Bus,
    roles: Arc<dyn RoleSource>,
    audit: Arc<dyn AuditSink>,
    scopes: PortalScopeIndex,
    governance: Option<Arc<dyn GovernanceView>>,
    installations: Option<Arc<InstallationLookup>>,
    state: Arc<Mutex<BrokerState>>,
}

impl BusPortalBroker {
    /// `roles` and `audit` are the services the bus was built with; `scopes` is the resource [`track_portal_scopes`] fills.
    pub fn new(bus: Bus, roles: Arc<dyn RoleSource>, audit: Arc<dyn AuditSink>, scopes: PortalScopeIndex) -> Self {
        Self {
            bus,
            roles,
            audit,
            scopes,
            governance: None,
            installations: None,
            state: Arc::new(Mutex::new(BrokerState::default())),
        }
    }

    /// Re-check governance at bind: the exact build still admitted, its hash not kill-listed, no host-wide disable.
    pub fn with_governance(mut self, view: Arc<dyn GovernanceView>) -> Self {
        self.governance = Some(view);
        self
    }

    /// Stamp grants and audit rows with governance's web-bundle installation (A-14).
    pub fn with_installations(mut self, lookup: Arc<InstallationLookup>) -> Self {
        self.installations = Some(lookup);
        self
    }

    /// The one-op write path for `Bus::install_portal_writes`; it commits only for a live binding of this broker.
    pub fn write_sink(&self, db: Sender<DbCommand>) -> PortalCommitSink {
        PortalCommitSink {
            db,
            state: Arc::clone(&self.state),
        }
    }

    /// The bridge's resource over this broker.
    pub fn into_resource(self) -> PortalBrokerRes {
        PortalBrokerRes(Arc::new(self))
    }

    fn lock(&self) -> MutexGuard<'_, BrokerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn knows(&self, client: u64) -> bool {
        self.lock().bindings.contains_key(&client)
    }

    fn binding(&self, client: u64) -> Result<Binding, IpcError> {
        self.lock().bindings.get(&client).cloned().ok_or(IpcError::Unavailable)
    }

    /// The installing identity's role at the petal: the bus role view, capped by fe-ui's resolved local role (Viewer
    /// until known) and by the viewer's token.
    fn ceiling(&self, did: &str, petal_id: &str, viewer: RoleLevel) -> RoleLevel {
        let local = match self.scopes.local_role(petal_id) {
            LocalRole::Resolved(role) => role,
            LocalRole::Pending | LocalRole::Unknown => RoleLevel::Viewer,
        };
        self.roles.role_at(did, petal_id).min(local).min(viewer)
    }

    /// Reads need Viewer at use: the bus role view and the resolved local role, under the binding's ceiling.
    fn reads_allowed(&self, binding: &Binding) -> bool {
        let petal_id = &binding.scope.petal_id;
        let mut role = self.roles.role_at(&binding.installing_did, petal_id).min(binding.ceiling);
        if let LocalRole::Resolved(local) = self.scopes.local_role(petal_id) {
            role = role.min(local);
        }
        role.is_at_least(RoleLevel::Viewer)
    }

    /// Relay a ticket to the bridge, answering at once when the bus already settled it.
    fn forward(&self, client: u64, ticket: Ticket) -> PendingReply {
        let now = Instant::now();
        let (reply, pending) = mpsc::channel();
        let mut forward = Forward {
            client,
            ticket,
            reply,
            deadline: now + REPLY_DEADLINE,
        };
        match forward.settle(now) {
            Some(result) => {
                let _ = forward.reply.send(result);
            }
            None => self.lock().forwards.push(forward),
        }
        pending
    }
}

/// Answer every relayed ticket the bus settled.
fn pump(forwards: &mut Vec<Forward>, now: Instant) {
    forwards.retain_mut(|forward| match forward.settle(now) {
        Some(result) => {
            let _ = forward.reply.send(result);
            false
        }
        None => true,
    });
}

impl PortalBroker for BusPortalBroker {
    fn open(&self, request: &BindRequest<'_>) -> Result<BrokerClient, IpcError> {
        // A binding target names its package; other bytes never bind there.
        if let BundleTarget::Bound { package_id, .. } = request.target {
            if package_id.as_str() != request.package_id {
                return Err(IpcError::NotFound);
            }
        }
        let scope = self.scopes.scope_of(request.target).ok_or(IpcError::NotFound)?;
        if self.governance.as_deref().is_some_and(|view| !still_admitted(view, request)) {
            return Err(IpcError::Revoked);
        }
        let installing_did = self.bus.local_did().to_owned();
        let ceiling = self.ceiling(&installing_did, &scope.petal_id, request.viewer_ceiling);
        if !ceiling.is_at_least(RoleLevel::Viewer) {
            return Err(IpcError::RoleDenied);
        }
        let installation_id = self
            .installations
            .as_ref()
            .and_then(|lookup| (**lookup)(request.package_id, request.content_hash));
        let grants = portal_grants(request, &scope, installation_id.clone());
        // The only way a portal client is opened: the ceiling rides on the client, never on a Caller literal.
        let handle = self.bus.open_portal_client(&installing_did, ceiling, grants);
        self.lock().bindings.insert(
            handle.id,
            Binding {
                installing_did: installing_did.clone(),
                package_id: request.package_id.to_owned(),
                content_hash: request.content_hash.to_owned(),
                installation_id,
                scope: scope.clone(),
                ceiling,
                node_subs: HashSet::new(),
            },
        );
        Ok(BrokerClient {
            client: handle.id,
            installing_did,
            petal_id: scope.petal_id,
            granted: granted_capabilities(request.portal),
            role_ceiling: ceiling,
        })
    }

    fn close(&self, client: u64) {
        let known = {
            let mut state = self.lock();
            state.forwards.retain(|forward| forward.client != client);
            state.bindings.remove(&client).is_some()
        };
        // In-flight writes still settle at the bus and their rows stay (OD-E1); only the relays end here.
        if known {
            self.bus.close_client(client);
        }
    }

    fn set_paused(&self, client: u64, paused: bool) {
        if self.knows(client) {
            self.bus.set_client_paused(client, paused);
        }
    }

    fn subscribe(&self, client: u64, address: &str, params: &Value) -> Result<u64, IpcError> {
        let binding = self.binding(client)?;
        if !self.reads_allowed(&binding) {
            return Err(IpcError::NotFound);
        }
        let subscription = self
            .bus
            .client_subscribe(client, &binding.scope.petal_id, address, params)
            .map_err(|refusal| bus_error(&refusal))?;
        if address == NODE_CHANGES {
            if let Some(binding) = self.lock().bindings.get_mut(&client) {
                binding.node_subs.insert(subscription);
            }
        }
        Ok(subscription)
    }

    fn unsubscribe(&self, client: u64, subscription: u64) {
        let known = match self.lock().bindings.get_mut(&client) {
            Some(binding) => {
                binding.node_subs.remove(&subscription);
                true
            }
            None => false,
        };
        if known {
            self.bus.client_unsubscribe(client, subscription);
        }
    }

    fn take_events(&self, client: u64, max: usize) -> Vec<BrokerEvent> {
        let mut state = self.lock();
        pump(&mut state.forwards, Instant::now());
        let Some(binding) = state.bindings.get_mut(&client) else {
            return Vec::new();
        };
        // Safe under this lock: the bus calls no broker code from here (only `client_set_property` reaches the sink).
        self.bus
            .take_client_events(client, max)
            .iter()
            .map(|event| {
                let node_changes = binding.node_subs.contains(&event.subscription);
                if event.kind == BusDelivery::Ended {
                    binding.node_subs.remove(&event.subscription);
                }
                page_event(event, node_changes)
            })
            .collect()
    }

    fn invoke(&self, client: u64, package_id: &str, command_id: &str, args: &Value) -> Result<PendingReply, IpcError> {
        if !self.knows(client) {
            return Err(IpcError::Unavailable);
        }
        let ticket = self
            .bus
            .client_call_command(client, package_id, command_id, args)
            .map_err(|refusal| invoke_error(&refusal))?;
        Ok(self.forward(client, Ticket::Call(ticket.reply)))
    }

    fn emit(&self, client: u64, stream_id: &str, data: &Value) -> Result<(), IpcError> {
        if !self.knows(client) {
            return Err(IpcError::Unavailable);
        }
        self.bus
            .client_publish(client, stream_id, data)
            .map_err(|refusal| bus_error(&refusal))
    }

    fn set_property(&self, client: u64, node_id: &str, key: &str, value: &Value) -> Result<PendingReply, IpcError> {
        if !self.knows(client) {
            return Err(IpcError::Unavailable);
        }
        // Never under this broker's lock: the bus calls the write sink, which takes it, before returning.
        let ticket = self
            .bus
            .client_set_property(client, node_id, key, value)
            .map_err(|refusal| bus_error(&refusal))?;
        Ok(self.forward(client, Ticket::Write(ticket.reply)))
    }

    fn audit_refusal(&self, client: u64, operation: &str, target: &str, code: &str) {
        let Ok(binding) = self.binding(client) else {
            return;
        };
        let operation = match operation {
            "subscribe" | "act" | "invoke" | "emit" | "intent" => operation,
            _ => "other",
        };
        let outcome = audit_text(&code.replace('_', "-"));
        // A write refused below Editor is audited like the bus's own role-denied write (ipc-v1.md §9).
        let action = if operation == "act" && outcome == "role-denied" {
            AuditAction::PropertyWrite
        } else {
            AuditAction::Denial
        };
        self.audit.record(AuditEntry {
            action,
            actor_did: binding.installing_did,
            caller_kind: Some(CallerKind::Portal),
            installation_id: binding.installation_id,
            target: format!("{}/{operation}:{}", binding.package_id, audit_text(target)),
            args_digest: None,
            outcome,
            at_ms: wall_clock_ms(),
        });
    }
}

/// Governance still admits this exact build for its consented publisher, no un-overridden kill-list entry names its
/// hash, and Disable all is off.
fn still_admitted(view: &dyn GovernanceView, request: &BindRequest<'_>) -> bool {
    let admitted = view
        .record(request.package_id, request.content_hash)
        .is_some_and(|record| record.state == RecordState::Admitted && record.publisher_did == request.publisher_did);
    let listed = kill_list_status(view, request.target, &[request.content_hash], request.content_hash)
        == KillListStatus::Listed;
    admitted && !listed && !view.host_disabled()
}

/// The bus grants of a mount: the manifest's declarations (consent covered this exact set) at the pinned petal.
fn portal_grants(request: &BindRequest<'_>, scope: &PortalScope, installation_id: Option<String>) -> PortalGrants {
    let portal = request.portal;
    let host = |source: &str| ConsumeEntry {
        source: source.to_owned(),
        id: None,
        sql: None,
        min_interval_ms: None,
        metrics: None,
    };
    let mut consume = Vec::new();
    if portal.subscriptions.node {
        consume.push(host(NODE_CHANGES));
    }
    if portal.subscriptions.readings {
        consume.push(host("host/readings"));
    }
    consume.extend(portal.subscriptions.queries.iter().map(|query| ConsumeEntry {
        id: Some(query.name.clone()),
        sql: Some(query.sql.clone()),
        ..host("host/query")
    }));
    consume.extend(portal.hooks.iter().map(|hook| host(&hook.source)));
    PortalGrants {
        package_id: request.package_id.to_owned(),
        content_hash: request.content_hash.to_owned(),
        installation_id,
        verse_id: scope.verse_id.clone(),
        petal_id: scope.petal_id.clone(),
        consume,
        // Every property the installing identity reads; `portal` keys are removed on delivery (`scrub_node_event`).
        property_scope: Some(vec!["*".to_owned()]),
        streams: portal
            .streams
            .iter()
            .map(|stream| PortalStream {
                id: stream.id.clone(),
                max_rate_hz: f64::from(stream.max_rate_hz),
                max_event_bytes: u64::from(stream.max_event_bytes),
                consumers: stream.consumers.clone(),
            })
            .collect(),
        invoke: portal
            .commands
            .invoke
            .iter()
            .map(|target| format!("{}/{}", target.package, target.command))
            .collect(),
        set_property_keys: portal
            .commands
            .set_property
            .as_ref()
            .map(|set| set.keys.clone())
            .unwrap_or_default(),
    }
}

/// ipc-v1.md §4.3 strings for every declaration: consent covered this exact set (its capability digest).
fn granted_capabilities(portal: &PortalSection) -> BTreeSet<String> {
    let mut granted = BTreeSet::new();
    if portal.subscriptions.node {
        granted.insert("subscribe:node".to_owned());
    }
    if portal.subscriptions.readings {
        granted.insert("subscribe:readings".to_owned());
    }
    granted.extend(portal.subscriptions.queries.iter().map(|query| format!("subscribe:query/{}", query.name)));
    granted.extend(portal.hooks.iter().map(|hook| format!("subscribe:{}", hook.source)));
    if portal.commands.set_property.is_some() {
        granted.insert("act:set_property".to_owned());
    }
    granted.extend(
        portal
            .commands
            .invoke
            .iter()
            .map(|target| format!("invoke:{}/{}", target.package, target.command)),
    );
    granted.extend(portal.streams.iter().map(|stream| format!("emit:{}", stream.id)));
    granted.extend(portal.ui_intents.iter().map(|intent| format!("intent:{intent}")));
    granted
}

// ---------------------------------------------------------------------------
// Deliveries and refusals as the page sees them
// ---------------------------------------------------------------------------

/// A bus event as the bridge reads it; node-change events lose host-reserved keys first.
fn page_event(event: &BusEvent, node_changes: bool) -> BrokerEvent {
    let mut data = serde_json::from_str::<Value>(&event.data).unwrap_or(Value::Null);
    if node_changes {
        scrub_node_event(event.kind, &mut data);
    }
    BrokerEvent {
        subscription: event.subscription,
        kind: page_delivery(event.kind),
        dropped_before: u64::from(event.dropped_before),
        data,
    }
}

fn page_delivery(kind: BusDelivery) -> PageDelivery {
    match kind {
        BusDelivery::Snapshot => PageDelivery::Snapshot,
        BusDelivery::Delta => PageDelivery::Delta,
        BusDelivery::ResyncRequired => PageDelivery::ResyncRequired,
        BusDelivery::Ended => PageDelivery::Ended,
    }
}

/// Host-reserved `portal` keys hold other bundles' references and share-URL bearer tokens (README §5.1, §9.3).
fn is_hidden_key(key: &str) -> bool {
    key == "portal" || key.starts_with("portal.")
}

/// Drop hidden keys from a node snapshot part's `properties` and hidden `property_changed` entries from a delta.
fn scrub_node_event(kind: BusDelivery, data: &mut Value) {
    match kind {
        BusDelivery::Snapshot => {
            for node in data.get_mut("nodes").and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(properties) = node.get_mut("properties").and_then(Value::as_object_mut) {
                    properties.retain(|key, _| !is_hidden_key(key));
                }
            }
        }
        BusDelivery::Delta => {
            if let Some(changes) = data.get_mut("changes").and_then(Value::as_array_mut) {
                changes.retain(|change| {
                    let property = change.get("op").and_then(Value::as_str) == Some("property_changed");
                    !(property && change.get("key").and_then(Value::as_str).is_some_and(is_hidden_key))
                });
            }
        }
        BusDelivery::ResyncRequired | BusDelivery::Ended => {}
    }
}

/// ipc-v1.md §7.1 for a bus refusal.
fn bus_error(refusal: &BusRefusal) -> IpcError {
    match refusal {
        BusRefusal::NotFound => IpcError::NotFound,
        BusRefusal::NotAvailable(_) => IpcError::Unavailable,
        BusRefusal::CapabilityDenied(_) => IpcError::CapabilityDenied,
        BusRefusal::RateLimited(_) => IpcError::RateLimited,
        BusRefusal::InvalidInput(_) => IpcError::Invalid,
        BusRefusal::LimitReached(_) => IpcError::TooManySubscriptions,
    }
}

/// A declared target the bus denies (not exposed, or below its role floor) is the uniform `not_found` (ipc-v1.md §6.2).
fn invoke_error(refusal: &BusRefusal) -> IpcError {
    match refusal {
        BusRefusal::CapabilityDenied(_) => IpcError::NotFound,
        other => bus_error(other),
    }
}

/// A command's final answer; the bridge wraps a success as `{result}`.
fn command_reply(outcome: CommandOutcome) -> Result<Value, IpcError> {
    match outcome {
        CommandOutcome::Ok(text) => serde_json::from_str(&text).map_err(|_| IpcError::Backend),
        CommandOutcome::Guest(CommandError::InvalidInput(_)) => Err(IpcError::Invalid),
        CommandOutcome::Guest(CommandError::Refused(_)) => Err(IpcError::PolicyDenied),
        CommandOutcome::Guest(CommandError::Failed(_)) => Err(IpcError::Backend),
        CommandOutcome::Host(error) => Err(host_error(&error)),
    }
}

fn host_error(error: &HostError) -> IpcError {
    match error {
        HostError::CapabilityDenied(_) => IpcError::CapabilityDenied,
        HostError::PolicyDenied(_) => IpcError::PolicyDenied,
        HostError::InvalidInput(_) => IpcError::Invalid,
        HostError::BudgetExceeded(_) | HostError::RateLimited(_) => IpcError::RateLimited,
        HostError::Revoked(_) => IpcError::Revoked,
        HostError::NotAvailable(_) => IpcError::Unavailable,
        HostError::Backend(_) => IpcError::Backend,
        HostError::UnknownOutcome(_) => IpcError::UnknownOutcome,
    }
}

/// A portal write's outcome (bus/AGENTS.md, seam for P-BROKER); the bridge answers `{node_id, key}` on success.
fn write_reply(outcome: &WriteOutcome) -> Result<Value, IpcError> {
    match outcome {
        WriteOutcome::Committed(_) => Ok(Value::Null),
        WriteOutcome::RoleDenied(_) | WriteOutcome::Refused(PluginRefusal::Revoked(_)) => Err(IpcError::RoleDenied),
        WriteOutcome::Refused(PluginRefusal::InvalidInput(_)) => Err(IpcError::NotFound),
        WriteOutcome::Refused(PluginRefusal::UnknownOutcome(_)) => Err(IpcError::UnknownOutcome),
        WriteOutcome::Refused(PluginRefusal::CapabilityDenied(_) | PluginRefusal::Backend(_)) => Err(IpcError::Backend),
    }
}

/// Page-derived audit text: control characters replaced, at most 128 bytes on a character boundary.
fn audit_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_AUDIT_TEXT_BYTES));
    for c in text.chars().map(|c| if c.is_control() { '\u{FFFD}' } else { c }) {
        if out.len() + c.len_utf8() > MAX_AUDIT_TEXT_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// The one-op write path (A-13)
// ---------------------------------------------------------------------------

/// Commits one portal `set_property` as ONE `DbCommand::PluginCommit` (no readings, no tick) under an admission minted
/// from the live binding; install it with `Bus::install_portal_writes`.
pub struct PortalCommitSink {
    db: Sender<DbCommand>,
    state: Arc<Mutex<BrokerState>>,
}

impl PortalCommitSink {
    /// The pinned ancestry of the live binding this write came from; a write without one never reaches the DB.
    fn binding_ancestry(&self, write: &PortalWrite) -> Result<String, PluginRefusal> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .bindings
            .values()
            .find(|binding| {
                binding.package_id == write.package_id
                    && binding.content_hash == write.content_hash
                    && binding.installing_did == write.identity_did
                    && binding.scope.petal_id == write.petal_id
                    && binding.scope.verse_id == write.verse_id
            })
            .map(|binding| binding.scope.ancestry())
            .ok_or_else(|| PluginRefusal::Revoked("no live portal binding for this write".into()))
    }
}

impl PortalWriteSink for PortalCommitSink {
    fn commit(&self, write: &PortalWrite, reply: Sender<PluginResult>) {
        let command = self
            .binding_ancestry(write)
            .and_then(|ancestry| portal_commit(write, ancestry, reply.clone()));
        let refusal = match command {
            // `try_send`: the bus calls this on the main thread, which must never block on the DB queue.
            Ok(command) => match self.db.try_send(command) {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(package_id = %write.package_id, "portal write not queued for the database: {error}");
                    PluginRefusal::Backend("the database thread is not accepting writes".into())
                }
            },
            Err(refusal) => refusal,
        };
        let _ = reply.send(Err(refusal));
    }
}

/// The admission for one portal write: `storage.write`, property scope = the matched key, `SetProperty` only, verse and
/// ancestry pinned, provenance `{package_id, content_hash}` from the binding.
fn portal_commit(write: &PortalWrite, ancestry: String, reply: Sender<PluginResult>) -> Result<DbCommand, PluginRefusal> {
    let PendingOp::SetProperty { key, .. } = &write.op else {
        return Err(PluginRefusal::InvalidInput("a portal write is one set_property".into()));
    };
    let mint = |error: String| PluginRefusal::CapabilityDenied(format!("portal write admission: {error}"));
    let package = PackageIdentity::new(write.package_id.clone(), write.content_hash.clone()).map_err(mint)?;
    let admission = PluginAdmission::new(
        write.package_id.clone(),
        write.petal_id.clone(),
        vec![fe_sdk::CAP_STORAGE_WRITE.to_owned()],
        vec![key.clone()],
        vec![fe_database::build_scope(&write.verse_id, None, None)],
        vec![ancestry.clone()],
    )
    .map_err(mint)?
    .with_package(package)
    .with_scene_writes(vec![SceneWrite::SetProperty])
    .map_err(mint)?
    .bind_scope(ancestry)
    .map_err(mint)?;
    Ok(DbCommand::PluginCommit {
        admission,
        ops: vec![write.op.clone()],
        readings: Vec::new(),
        reply,
    })
}

#[cfg(test)]
mod tests {
    use fe_plugin::bus::{BusPolicy, BusSources, SceneSnapshotSource};
    use fe_plugin::tick::ManualClock;
    use fe_runtime::plugin_host::PluginReply;
    use fe_webview::portal_bundle::admission::AdmissionRecord;
    use fe_webview::portal_bundle::BundleIdentity;
    use serde_json::json;

    use super::*;

    const LOCAL: &str = "did:key:z6MkLocalHost";
    const PUBLISHER: &str = "did:key:z6MkPublisher";
    const CONSOLE: &str = "org.example.console";
    const FLEET: &str = "org.example.fleet-sim";
    const VERSE: &str = "01JVERSE";
    const FRACTAL: &str = "01JFRACTAL";
    const PETAL: &str = "01JPETAL";
    const REPO: &str = "node-repo";

    /// The bus's local view: the host DID is Owner everywhere (as `plugin_bridge::ExtensionRoles`).
    struct Roles;

    impl RoleSource for Roles {
        fn role_at(&self, did: &str, _petal_id: &str) -> RoleLevel {
            if did == LOCAL {
                RoleLevel::Owner
            } else {
                RoleLevel::None
            }
        }
    }

    struct NoExternal;

    impl BusPolicy for NoExternal {
        fn external_access_allowed(&self, _verse_id: &str) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<AuditEntry>>);

    impl AuditSink for Audit {
        fn record(&self, entry: AuditEntry) {
            self.0.lock().unwrap().push(entry);
        }
    }

    /// One node whose properties include a host-reserved share feed.
    struct OnePetal;

    impl SceneSnapshotSource for OnePetal {
        fn snapshot(&self, petal_id: &str, reply: Sender<Result<Vec<Value>, HostError>>) {
            let node = json!({
                "node_id": "node-1", "petal_id": petal_id,
                "properties": { "console.note": "hi", "portal.feeds": { "live": "http://127.0.0.1:1/api/v1/shared/t" } }
            });
            let _ = reply.send(Ok(vec![node]));
        }
    }

    /// Governance that no longer admits anything.
    struct NothingAdmitted;

    impl GovernanceView for NothingAdmitted {
        fn host_disabled(&self) -> bool {
            false
        }
        fn kill_listed(&self, _digests: &[&str]) -> bool {
            false
        }
        fn record(&self, _package_id: &str, _content_hash: &str) -> Option<AdmissionRecord> {
            None
        }
        fn can_admit(&self, _target: &BundleTarget, _identity: &BundleIdentity) -> bool {
            false
        }
    }

    fn console_section() -> PortalSection {
        serde_json::from_value(json!({
            "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Console",
            "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [], "wasm": false, "inline_styles": false },
            "navigation": [],
            "subscriptions": { "node": true, "readings": false, "queries": [] },
            "commands": { "set_property": { "keys": ["console.*"] }, "invoke": [{ "package": FLEET, "command": "start" }] },
            "hooks": [],
            "streams": [{ "id": "selection", "max_rate_hz": 4, "max_event_bytes": 2048 }],
            "ui_intents": ["focus_node"]
        }))
        .expect("console portal section")
    }

    fn bind<'a>(target: &'a BundleTarget, portal: &'a PortalSection, hash: &'a str, viewer: RoleLevel) -> BindRequest<'a> {
        BindRequest {
            mount_id: 1,
            target,
            package_id: CONSOLE,
            content_hash: hash,
            publisher_did: PUBLISHER,
            portal,
            viewer_ceiling: viewer,
        }
    }

    fn committed() -> PluginReply {
        PluginReply::Committed(serde_json::from_value(json!({ "op_count": 1, "created_node_ids": [] })).unwrap())
    }

    /// Mount → bind → read → act → refusals → unmount, through the bridge's `PortalBroker` seam over a real bus.
    #[test]
    fn portal_broker_journey() {
        let clock = ManualClock::default();
        clock.set(1_000);
        let audit = Arc::new(Audit::default());
        let roles: Arc<dyn RoleSource> = Arc::new(Roles);
        let scene: Arc<dyn SceneSnapshotSource> = Arc::new(OnePetal);
        let sources = BusSources {
            query: None,
            scene: Some(scene),
            roles: roles.clone(),
            policy: Arc::new(NoExternal),
            audit: audit.clone(),
        };
        let bus = Bus::new(Arc::new(clock.clone()), sources, LOCAL);
        let scopes = PortalScopeIndex::default();
        let broker = BusPortalBroker::new(bus.clone(), roles.clone(), audit.clone(), scopes.clone());
        let (db_tx, db_rx) = crossbeam::channel::unbounded();
        assert!(bus.install_portal_writes(Arc::new(broker.write_sink(db_tx))));
        let section = console_section();
        let hash = "cd".repeat(32);
        let node = BundleTarget::Node(REPO.into());

        // A target no local Open pinned never binds.
        assert_eq!(broker.open(&bind(&node, &section, &hash, RoleLevel::Owner)).unwrap_err(), IpcError::NotFound);

        // The Open pinned the node's scope; fe-ui resolved the local role at the active petal as Editor.
        let scope = PortalScope { verse_id: VERSE.into(), fractal_id: FRACTAL.into(), petal_id: PETAL.into() };
        scopes.record_open(REPO, Some(scope.clone()));
        scopes.set_active(Some(VERSE), Some(FRACTAL), Some(PETAL), Some(RoleLevel::Editor));

        // The ceiling is the installing identity's role at the scope (Editor, though the bus view says Owner).
        let editor = broker.open(&bind(&node, &section, &hash, RoleLevel::Owner)).unwrap();
        assert_eq!((editor.installing_did.as_str(), editor.petal_id.as_str(), editor.role_ceiling), (LOCAL, PETAL, RoleLevel::Editor));
        for capability in ["subscribe:node", "act:set_property", "emit:selection", "intent:focus_node"] {
            assert!(editor.granted.contains(capability), "{capability} granted");
        }
        assert!(editor.granted.contains(&format!("invoke:{FLEET}/start")));
        // ...and never above the viewer's token.
        let viewer = broker.open(&bind(&node, &section, &hash, RoleLevel::Viewer)).unwrap();
        assert_eq!(viewer.role_ceiling, RoleLevel::Viewer);

        // A scope's bound bundle binds at that petal for its own package only (B10f).
        let bound = BundleTarget::Bound { petal_id: PETAL.into(), package_id: CONSOLE.into() };
        let bound_client = broker.open(&bind(&bound, &section, &hash, RoleLevel::Owner)).unwrap();
        assert_eq!(bound_client.petal_id, PETAL);
        let foreign = BundleTarget::Bound { petal_id: PETAL.into(), package_id: "org.example.other".into() };
        assert_eq!(broker.open(&bind(&foreign, &section, &hash, RoleLevel::Owner)).unwrap_err(), IpcError::NotFound);

        // Governance that no longer admits the build refuses the bind (defence in depth after the mount verdict).
        let guarded = BusPortalBroker::new(bus.clone(), roles.clone(), audit.clone(), scopes.clone())
            .with_governance(Arc::new(NothingAdmitted));
        assert_eq!(guarded.open(&bind(&node, &section, &hash, RoleLevel::Owner)).unwrap_err(), IpcError::Revoked);

        // read: the node snapshot reaches the page without host-reserved `portal` keys.
        broker.subscribe(editor.client, NODE_CHANGES, &json!({})).unwrap();
        bus.poll();
        let events = broker.take_events(editor.client, 64);
        assert_eq!(events[0].kind, PageDelivery::Snapshot);
        assert_eq!(events[0].data["nodes"][0]["properties"], json!({ "console.note": "hi" }));

        // act: ONE PluginCommit, provenance and ancestry from the binding, exactly the matched key, no readings.
        let written = broker.set_property(editor.client, "node-1", "console.note", &json!("hello")).unwrap();
        let Ok(DbCommand::PluginCommit { admission, ops, readings, reply }) = db_rx.try_recv() else {
            panic!("one PluginCommit reached the database thread");
        };
        let package = admission.package().map(|package| (package.package_id(), package.content_hash()));
        assert_eq!(package, Some((CONSOLE, hash.as_str())));
        assert_eq!(admission.bound_scope(), Some(scope.ancestry().as_str()));
        assert!(admission.allows_property("console.note") && !admission.allows_property("console.other"));
        assert!(admission.allows_write(SceneWrite::SetProperty) && !admission.allows_write(SceneWrite::DeleteNode));
        assert_eq!(ops, vec![PendingOp::SetProperty { node_id: "node-1".into(), key: "console.note".into(), value: json!("hello") }]);
        assert!(readings.is_empty());
        reply.send(Ok(committed())).unwrap();
        bus.poll();
        broker.take_events(editor.client, 64);
        assert_eq!(written.try_recv(), Ok(Ok(Value::Null)));

        // The DB judges the role at use: its refusal is the page's role_denied.
        let demoted = broker.set_property(editor.client, "node-1", "console.note", &json!("again")).unwrap();
        let Ok(DbCommand::PluginCommit { reply, .. }) = db_rx.try_recv() else {
            panic!("the second write reached the database thread");
        };
        reply.send(Err(PluginRefusal::Revoked("role floor".into()))).unwrap();
        bus.poll();
        broker.take_events(editor.client, 64);
        assert_eq!(demoted.try_recv(), Ok(Err(IpcError::RoleDenied)));

        // A Viewer-capped binding is role-denied before the DB; an undeclared key never leaves the bus.
        let capped = broker.set_property(viewer.client, "node-1", "console.note", &json!(1)).unwrap();
        assert_eq!(capped.try_recv(), Ok(Err(IpcError::RoleDenied)));
        assert_eq!(broker.set_property(editor.client, "node-1", "secret", &json!(1)).unwrap_err(), IpcError::CapabilityDenied);
        assert!(db_rx.try_recv().is_err(), "nothing else reached the database thread");

        // invoke: no running target is `unavailable`; a target the bus denies is the uniform `not_found` (§6.2).
        assert_eq!(broker.invoke(editor.client, FLEET, "start", &json!({})).unwrap_err(), IpcError::Unavailable);
        assert_eq!(broker.invoke(editor.client, FLEET, "ping", &json!({})).unwrap_err(), IpcError::NotFound);

        // The bridge's own refusals are audited as the binding, with page text neutralised.
        broker.audit_refusal(editor.client, "act", "set_property:x\nforged", "capability_denied");
        let last = audit.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!((last.action, last.actor_did.as_str(), last.caller_kind), (AuditAction::Denial, LOCAL, Some(CallerKind::Portal)));
        assert_eq!((last.target.as_str(), last.outcome.as_str()), ("org.example.console/act:set_property:x\u{FFFD}forged", "capability-denied"));

        // Unmount: the client is gone at the bus, and a write naming a closed binding never reaches the DB.
        for client in [editor.client, viewer.client, bound_client.client] {
            broker.close(client);
        }
        assert_eq!(broker.subscribe(editor.client, NODE_CHANGES, &json!({})).unwrap_err(), IpcError::Unavailable);
        assert_eq!(bus.client_subscribe(editor.client, PETAL, NODE_CHANGES, &json!({})).unwrap_err(), BusRefusal::NotFound);
        let (late_tx, late_rx) = crossbeam::channel::bounded(1);
        let late = PortalWrite {
            write: 99,
            identity_did: LOCAL.into(),
            package_id: CONSOLE.into(),
            content_hash: hash.clone(),
            installation_id: None,
            verse_id: VERSE.into(),
            petal_id: PETAL.into(),
            op: PendingOp::SetProperty { node_id: "node-1".into(), key: "console.note".into(), value: json!(2) },
        };
        let (unused_tx, _unused_rx) = crossbeam::channel::unbounded();
        broker.write_sink(unused_tx).commit(&late, late_tx);
        assert!(matches!(late_rx.try_recv(), Ok(Err(PluginRefusal::Revoked(_)))));
    }
}
