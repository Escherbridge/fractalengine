//! The broker behind [`Bus`]: registrations, subscriptions, fan-out, commands and query pacing; see bus/AGENTS.md §broker.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crossbeam::channel::{Receiver, Sender, TryRecvError};
use fe_policy::RoleLevel;
use fe_runtime::messages::SceneChange;
use fe_runtime::plugin_host::{PluginRefusal, PluginReply, PluginResult};
use fe_sdk::manifest_v2::{
    metric_allowed, scope_allows, ConsumeEntry, ExposedCommand, ExposedStream, ExtensionManifest, RoleFloor,
};
use fe_sdk::plugin_host::{PendingOp, TransactionReceipt};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::sync::{oneshot, Notify};

use super::query::QueryState;
use super::rate::TokenBucket;
use super::schema::validate_args;
use super::sources::{
    AdmissionEvent, AdmissionKind, AdmissionReason, AuditAction, AuditEntry, BusSources, PortalWrite,
    PortalWriteSink, QueryRequest, ReadingEvent,
};
use super::{
    is_content_hash, is_node_id, is_package_id, json_digest, parse_command_address, portal_key_allowed, role_level,
    BusEvent, BusRefusal, Caller, CallerKey, CallerKind, CommandError, CommandOutcome, DeliveryKind, EndReason,
    HostError, InstanceKey, Source, ADMISSION_QUEUE, CALLER_COMMAND_RATE_HZ, COMMAND_QUEUE_PER_INSTANCE,
    COMMAND_REPLY_TIMEOUT_MS, EXTERNAL_QUEUE, GUEST_BATCH_BYTES, GUEST_BATCH_EVENTS, GUEST_QUERY_EVENT_BYTES,
    GUEST_QUEUE, MAX_ANCHOR_FILTER, MAX_COMMAND_ARGS_BYTES, MAX_COMMAND_RATE_HZ, MAX_COMMAND_RESULT_BYTES,
    MAX_STREAM_EVENT_BYTES, MAX_STREAM_RATE_HZ, MAX_SUBSCRIPTIONS_PER_CLIENT, MAX_SUBSCRIPTIONS_PER_INSTANCE,
    NODE_DELTA_CHANGES, NODE_SNAPSHOT_PART, PORTAL_STREAM_RATE_HZ, PORTAL_WRITE_BURST, PORTAL_WRITE_FLOOR,
    PORTAL_WRITE_IDENTITY_BURST, PORTAL_WRITE_IDENTITY_RATE_HZ, PORTAL_WRITE_RATE_HZ, PORTAL_WRITE_TIMEOUT_MS,
    QUERY_MIN_INTERVAL_FLOOR_MS, READINGS_PER_EVENT, REMOTE_REQUEST_RATE_HZ, REST_ROW_CAP,
};
use crate::tick::Clock;

/// Buckets untouched this long are forgotten.
const BUCKET_IDLE_MS: u64 = 60_000;
/// How often idle buckets are swept.
const PRUNE_EVERY_MS: u64 = 10_000;

/// An admitted instance as the broker sees it; the manifest is the consented one.
#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub installation_id: String,
    pub content_hash: String,
    pub verse_id: String,
    pub petal_id: String,
    /// The host's local DID: every bus call of the instance is judged as it.
    pub identity_did: String,
    pub manifest: Arc<ExtensionManifest>,
}

impl InstanceSpec {
    /// `(petal_id, manifest.id)`.
    pub fn key(&self) -> InstanceKey {
        InstanceKey::new(self.petal_id.clone(), self.manifest.id.clone())
    }
}

/// The `bus.*` status counters (R11, bus.md §7).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BusCounters {
    pub published: u64,
    pub delivered: u64,
    pub dropped_rate: u64,
    pub dropped_queue: u64,
    pub subscriptions: u64,
    pub commands_ok: u64,
    pub commands_refused: BTreeMap<String, u64>,
    pub query_runs: u64,
    pub query_unchanged: u64,
    pub query_deferred: u64,
}

/// What a non-guest client may subscribe to.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientGrants {
    /// The Extensions UI and slot widgets (local DID): extension streams and `host/admission`.
    Ui,
    /// A REST/WS/MCP connection: extension streams only (host streams keep their own routes).
    Api,
    /// A mounted portal bundle; open it with [`Bus::open_portal_client`].
    Portal(Box<PortalGrants>),
}

/// A mounted bundle's bus grants: its manifest's declarations intersected with consent, minted by the host at
/// mount from the admission record (portal ipc-v1.md §6.5); nothing in here ever comes from page data.
#[derive(Debug, Clone, PartialEq)]
pub struct PortalGrants {
    pub package_id: String,
    /// The admitted bundle's `content_hash`: the provenance every write is stamped with (R24).
    pub content_hash: String,
    /// The web-bundle installation (A-14); audit rows carry it.
    pub installation_id: Option<String>,
    pub verse_id: String,
    /// The repository node's petal: the one scope of every subscription, publish, call and write.
    pub petal_id: String,
    /// Declared `subscriptions` and `hooks` as consume entries (`host/query` entries carry the named SQL).
    pub consume: Vec<ConsumeEntry>,
    /// Keys a `host/node-changes` snapshot shows.
    pub property_scope: Option<Vec<String>>,
    /// Declared `streams` the bundle may `emit` on.
    pub streams: Vec<PortalStream>,
    /// Declared `commands.invoke` targets as `<package_id>/<command>`.
    pub invoke: Vec<String>,
    /// Declared `commands.set_property.keys` patterns (exact or `prefix.*`).
    pub set_property_keys: Vec<String>,
}

/// One declared bundle stream (`streams[]` of the bundle manifest).
#[derive(Debug, Clone, PartialEq)]
pub struct PortalStream {
    pub id: String,
    pub max_rate_hz: f64,
    pub max_event_bytes: u64,
    /// Restrict to these consumer packages.
    pub consumers: Option<Vec<String>>,
}

/// An admitted portal write; the outcome arrives on `reply` within `PORTAL_WRITE_TIMEOUT_MS` plus a broker turn.
#[derive(Debug)]
pub struct WriteTicket {
    pub write: u64,
    pub reply: oneshot::Receiver<WriteOutcome>,
}

/// What a portal write finally answered; audited once as `property-write`.
#[derive(Debug, Clone)]
pub enum WriteOutcome {
    /// The one op committed with its provenance.
    Committed(TransactionReceipt),
    /// The role at the petal, capped by the client's ceiling, is below Editor: nothing reached the DB.
    RoleDenied(String),
    /// The DB refused or could not confirm it (`revoked` = role floor or ancestry at use); never retried.
    Refused(PluginRefusal),
}

impl WriteOutcome {
    /// The audit outcome text.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Committed(_) => "ok",
            Self::RoleDenied(_) => "role-denied",
            Self::Refused(refusal) => refusal.kind(),
        }
    }
}

/// An open client; await `notify` for new events, then `Bus::take_client_events`.
#[derive(Debug, Clone)]
pub struct ClientHandle {
    pub id: u64,
    pub notify: Arc<Notify>,
}

/// An admitted command call; the broker answers within `COMMAND_REPLY_TIMEOUT_MS`.
#[derive(Debug)]
pub struct CommandTicket {
    pub call: u64,
    pub reply: oneshot::Receiver<CommandOutcome>,
}

/// One unit of guest work, in priority order: call results, commands, then events.
#[derive(Debug, Clone, PartialEq)]
pub enum GuestWork {
    CallResult { call: u64, outcome: Result<String, HostError> },
    Command { call: u64, command: String, args: String, caller: Caller },
    Events(Vec<BusEvent>),
}

/// The guest's `bus-hooks` exports as the worker calls them (E1 implements this over the component).
pub trait BusHooks {
    /// At most 64 events / 256 KiB, `seq` order per subscription.
    fn on_events(&mut self, events: Vec<BusEvent>);
    /// A declared command whose args passed `args_schema`; return JSON text.
    fn on_command(&mut self, command: &str, args: &str, caller: &Caller) -> Result<String, CommandError>;
    /// The answer to an earlier `bus.call`.
    fn on_call_result(&mut self, call: u64, outcome: Result<String, HostError>);
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Subscriber {
    Instance(InstanceKey),
    Client(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overflow {
    DropOldest,
    ReplaceLatest,
    Resync,
}

/// Where a scene snapshot source answers.
type SnapshotReply = crossbeam::channel::Sender<Result<Vec<Value>, HostError>>;
/// A query run to start outside the lock: subscription, request, prepared query, previous start.
type QueryLaunch = (u64, QueryRequest, super::sources::PreparedQuery, Option<u64>);

enum SnapshotState {
    Idle,
    Needed,
    Pending(Receiver<Result<Vec<Value>, HostError>>),
}

enum Detail {
    Query(Box<QueryState>),
    NodeChanges { property_scope: Option<Vec<String>>, snapshot: SnapshotState, ready: bool, pending: Vec<Value> },
    Readings { metrics: Option<Vec<String>>, anchors: Option<HashSet<String>> },
    Admission { visible: Option<HashSet<String>> },
    /// `from_portal`: the producer at `(petal, package)` is a portal client, never an instance of the same address.
    Stream { producer: InstanceKey, stream_id: String, role_floor: RoleLevel, external_required: bool, verse_id: String, from_portal: bool },
}

struct Subscription {
    id: u64,
    subscriber: Subscriber,
    address: Arc<str>,
    petal_id: String,
    identity_did: String,
    caller_kind: CallerKind,
    queue: VecDeque<BusEvent>,
    capacity: usize,
    overflow: Overflow,
    next_seq: u64,
    dropped_pending: u32,
    closed: bool,
    detail: Detail,
}

impl Subscription {
    /// Queue one event under the overflow rule; returns how many events it displaced.
    fn push(&mut self, kind: DeliveryKind, data: Arc<str>, at_ms: u64) -> u64 {
        if self.closed {
            return 0;
        }
        let mut displaced = 0u64;
        if self.queue.len() >= self.capacity {
            match self.overflow {
                Overflow::DropOldest => {
                    self.queue.pop_front();
                    displaced = 1;
                }
                Overflow::ReplaceLatest => {
                    displaced = self.queue.len() as u64;
                    self.queue.clear();
                }
                // The incoming delta is covered by the fresh snapshot, so it is dropped with the rest.
                Overflow::Resync => return self.resync(at_ms) + 1,
            }
        }
        self.dropped_pending = self.dropped_pending.saturating_add(displaced as u32);
        self.queue_event(kind, data, at_ms);
        displaced
    }

    /// Append without a capacity check (snapshot parts, `ended`).
    fn queue_event(&mut self, kind: DeliveryKind, data: Arc<str>, at_ms: u64) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.queue.push_back(BusEvent {
            subscription: self.id,
            source: self.address.clone(),
            seq,
            kind,
            dropped_before: 0,
            at_ms,
            data,
        });
    }

    /// Drop queued deltas, announce `resync-required` and ask for a fresh snapshot; `dropped_before` = the seq gap.
    fn resync(&mut self, at_ms: u64) -> u64 {
        let displaced = self.queue.len() as u64;
        self.queue.clear();
        self.dropped_pending = self.dropped_pending.saturating_add(displaced as u32);
        if let Detail::NodeChanges { snapshot, ready, pending, .. } = &mut self.detail {
            *snapshot = SnapshotState::Needed;
            *ready = false;
            pending.clear();
        }
        self.queue_event(DeliveryKind::ResyncRequired, Arc::from("{}"), at_ms);
        displaced
    }

    /// Queue `ended` and stop accepting events; removed once drained.
    fn end(&mut self, reason: EndReason, at_ms: u64) {
        if self.closed {
            return;
        }
        let data = format!("{{\"reason\":\"{}\"}}", reason.as_str());
        self.queue_event(DeliveryKind::Ended, Arc::from(data), at_ms);
        self.closed = true;
    }

    fn pop(&mut self) -> Option<BusEvent> {
        let mut event = self.queue.pop_front()?;
        event.dropped_before = std::mem::take(&mut self.dropped_pending);
        Some(event)
    }

    fn finished(&self) -> bool {
        self.closed && self.queue.is_empty()
    }

    fn owned_by_instance(&self, key: &InstanceKey) -> bool {
        matches!(&self.subscriber, Subscriber::Instance(owner) if owner == key)
    }

    fn owned_by_client(&self, client: u64) -> bool {
        self.subscriber == Subscriber::Client(client)
    }

    /// Whose counters this subscription's deliveries and drops land on.
    fn counter_owner(&self) -> Option<InstanceKey> {
        match (&self.subscriber, &self.detail) {
            (Subscriber::Instance(key), _) => Some(key.clone()),
            (Subscriber::Client(_), Detail::Stream { producer, from_portal: false, .. }) => Some(producer.clone()),
            (Subscriber::Client(_), _) => None,
        }
    }
}

struct InstanceState {
    spec: InstanceSpec,
    counters: BusCounters,
    stream_buckets: HashMap<String, TokenBucket>,
    commands: VecDeque<u64>,
    call_results: VecDeque<(u64, Result<String, HostError>)>,
    rr_cursor: u64,
}

struct ClientState {
    caller: Caller,
    grants: ClientGrants,
    notify: Arc<Notify>,
    paused: bool,
    /// A portal whose bundle left `admitted`: no new subscriptions, publishes, calls or writes.
    stopped: Option<EndReason>,
}

/// A portal write sent to the write path, answered and audited once by `settle_writes`.
struct PendingWrite {
    reply: Receiver<PluginResult>,
    answer: oneshot::Sender<WriteOutcome>,
    /// The `property-write` entry; its outcome and time are filled in when the write settles.
    audit: AuditEntry,
    deadline_ms: u64,
}

/// An admitted write: answered without the DB (role below the floor), or to send outside the lock.
enum WriteAdmission {
    Settled(WriteTicket),
    Send { ticket: WriteTicket, write: Box<PortalWrite>, reply: Sender<PluginResult>, sink: Arc<dyn PortalWriteSink> },
}

/// A stream producer at `(petal, package)` as a subscription needs it.
struct ProducerStream {
    external: bool,
    role_floor: RoleLevel,
    consumers: Option<Vec<String>>,
    verse_id: String,
    from_portal: bool,
}

/// An active instance's exposed stream wins over a portal's declared one at the same address.
enum ProducerLookup {
    Missing,
    Undeclared,
    Found(ProducerStream),
}

enum CallReply {
    External(oneshot::Sender<CommandOutcome>),
    Guest(InstanceKey),
}

struct PendingCall {
    target: InstanceKey,
    installation_id: String,
    command: String,
    args: String,
    args_digest: String,
    caller: Caller,
    deadline_ms: u64,
    reply: CallReply,
}

struct BrokerState {
    next_id: u64,
    instances: HashMap<InstanceKey, InstanceState>,
    retired: HashMap<String, BusCounters>,
    clients: HashMap<u64, ClientState>,
    subs: BTreeMap<u64, Subscription>,
    calls: HashMap<u64, PendingCall>,
    remote_gate: HashMap<String, TokenBucket>,
    caller_buckets: HashMap<CallerKey, TokenBucket>,
    command_buckets: HashMap<(String, CallerKey), TokenBucket>,
    /// Portal publish ceilings per `(petal, package)` and stream, so a reopened client cannot reset them.
    portal_stream_buckets: HashMap<(InstanceKey, String), TokenBucket>,
    /// Portal write budget per bundle package (2/s, burst 5).
    write_buckets: HashMap<String, TokenBucket>,
    /// Portal write budget per installing identity across every bundle.
    identity_write_buckets: HashMap<String, TokenBucket>,
    writes: HashMap<u64, PendingWrite>,
    feeds: HashSet<String>,
    last_prune_ms: u64,
}

impl BrokerState {
    fn new() -> Self {
        Self {
            next_id: 1,
            instances: HashMap::new(),
            retired: HashMap::new(),
            clients: HashMap::new(),
            subs: BTreeMap::new(),
            calls: HashMap::new(),
            remote_gate: HashMap::new(),
            caller_buckets: HashMap::new(),
            command_buckets: HashMap::new(),
            portal_stream_buckets: HashMap::new(),
            write_buckets: HashMap::new(),
            identity_write_buckets: HashMap::new(),
            writes: HashMap::new(),
            // bus.md §2.3 change-feed table: node and iot_reading are always fed.
            feeds: ["node", "iot_reading"].into_iter().map(String::from).collect(),
            last_prune_ms: 0,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn live_subscriptions(&self, subscriber: &Subscriber) -> usize {
        self.subs.values().filter(|sub| &sub.subscriber == subscriber && !sub.closed).count()
    }
}

/// Audits collected under the lock and written after it is released.
#[derive(Default)]
struct Effects {
    audits: Vec<AuditEntry>,
}

/// A subscription request resolved against grants, before any injected service is called.
struct Plan {
    subscriber: Subscriber,
    address: String,
    petal_id: String,
    verse_id: String,
    identity_did: String,
    caller_kind: CallerKind,
    guest: bool,
    installation_id: Option<String>,
    kind: PlanKind,
}

enum PlanKind {
    Query { sql: String, vars: Map<String, Value>, min_interval_ms: u64 },
    NodeChanges { property_scope: Option<Vec<String>> },
    Readings { metrics: Option<Vec<String>>, anchors: Option<HashSet<String>> },
    Admission { visible: Option<HashSet<String>> },
    Stream { producer: InstanceKey, stream_id: String, role_floor: RoleLevel, external_required: bool, verse_id: String, from_portal: bool },
}

/// The integration bus on one host: cheap to clone, shared by the plugin worker, fe-api and the bridge.
#[derive(Clone)]
pub struct Bus {
    inner: Arc<Mutex<BrokerState>>,
    clock: Arc<dyn Clock>,
    sources: BusSources,
    local_did: Arc<str>,
    /// The portal write path, installed once by the binary (absent: portal writes are `not-available`).
    portal_writes: Arc<OnceLock<Arc<dyn PortalWriteSink>>>,
}

impl Bus {
    /// `local_did` is the host identity every instance writes as; callers with any other DID are remote.
    pub fn new(clock: Arc<dyn Clock>, sources: BusSources, local_did: impl Into<String>) -> Self {
        let local_did: String = local_did.into();
        Self {
            inner: Arc::new(Mutex::new(BrokerState::new())),
            clock,
            sources,
            local_did: Arc::from(local_did),
            portal_writes: Arc::new(OnceLock::new()),
        }
    }

    /// The host's local DID.
    pub fn local_did(&self) -> &str {
        &self.local_did
    }

    /// Install the portal write path (the binary's `PluginCommit` sender) on every clone; `false` if one is set.
    pub fn install_portal_writes(&self, sink: Arc<dyn PortalWriteSink>) -> bool {
        self.portal_writes.set(sink).is_ok()
    }

    fn lock(&self) -> MutexGuard<'_, BrokerState> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    fn finish(&self, effects: Effects) {
        for entry in effects.audits {
            self.sources.audit.record(entry);
        }
    }

    /// `local` is recomputed from the DID and the ceiling; callers never assert it.
    fn normalized(&self, caller: &Caller) -> Caller {
        Caller { local: self.is_local(&caller.did, caller.role_ceiling), ..caller.clone() }
    }

    /// The local DID with no ceiling below Owner (a delegated token of this node is not local).
    fn is_local(&self, did: &str, ceiling: Option<RoleLevel>) -> bool {
        did == &*self.local_did && ceiling.is_none_or(|ceiling| ceiling.is_at_least(RoleLevel::Owner))
    }

    /// The caller's role at a petal, capped by its token ceiling.
    fn role_of(&self, caller: &Caller, petal_id: &str) -> RoleLevel {
        let role = self.sources.roles.role_at(&caller.did, petal_id);
        match caller.role_ceiling {
            Some(ceiling) if !ceiling.is_at_least(role) => ceiling,
            _ => role,
        }
    }

    // ---- registration ------------------------------------------------------------------------

    /// Admit an instance's declared bus surface; one `grant` audit entry when it declares a bus section.
    pub fn register_instance(&self, spec: InstanceSpec) -> Result<InstanceKey, HostError> {
        let key = spec.key();
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            if state.instances.contains_key(&key) {
                return Err(HostError::InvalidInput("an instance of this package is already bound at this petal".into()));
            }
            if let Some(bus) = &spec.manifest.bus {
                let surface = serde_json::to_value(bus).unwrap_or(Value::Null);
                effects.audits.push(audit_entry(
                    AuditAction::Grant,
                    &spec.identity_did,
                    None,
                    Some(spec.installation_id.clone()),
                    key.package_id.clone(),
                    Some(json_digest(&surface)),
                    "ok",
                    now,
                ));
            }
            state.retired.remove(&spec.installation_id);
            state.instances.insert(
                key.clone(),
                InstanceState {
                    spec,
                    counters: BusCounters::default(),
                    stream_buckets: HashMap::new(),
                    commands: VecDeque::new(),
                    call_results: VecDeque::new(),
                    rr_cursor: 0,
                },
            );
        }
        self.finish(effects);
        Ok(key)
    }

    /// Stop an instance: drop its subscriptions, end its streams' subscribers, answer its queued calls.
    pub fn unregister_instance(&self, key: &InstanceKey, reason: EndReason) {
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let Some(instance) = state.instances.remove(key) else { return };
            state.retired.insert(instance.spec.installation_id.clone(), instance.counters.clone());
            let BrokerState { subs, calls, clients, instances, .. } = &mut *state;
            let owned: Vec<u64> = subs.values().filter(|sub| sub.owned_by_instance(key)).map(|sub| sub.id).collect();
            for id in owned {
                if let Some(sub) = subs.remove(&id) {
                    effects.audits.push(audit_entry(
                        AuditAction::Unsubscribe,
                        &sub.identity_did,
                        Some(sub.caller_kind),
                        Some(instance.spec.installation_id.clone()),
                        sub.address.to_string(),
                        None,
                        reason.as_str(),
                        now,
                    ));
                }
            }
            for sub in subs.values_mut() {
                let produced_here = matches!(&sub.detail, Detail::Stream { producer, from_portal: false, .. } if producer == key);
                if produced_here && !sub.closed {
                    sub.end(EndReason::ProducerInactive, now);
                    notify_client(clients, &sub.subscriber);
                }
            }
            let answering: Vec<u64> = calls.iter().filter(|(_, call)| &call.target == key).map(|(id, _)| *id).collect();
            for id in answering {
                if let Some(call) = calls.remove(&id) {
                    let outcome = CommandOutcome::Host(HostError::NotAvailable("the instance stopped".into()));
                    resolve_call(instances, id, call, outcome, now, &mut effects);
                }
            }
            calls.retain(|_, call| !matches!(&call.reply, CallReply::Guest(caller) if caller == key));
        }
        self.finish(effects);
    }

    // ---- guest imports (WIT `bus`) -----------------------------------------------------------

    /// `bus.subscribe`: only sources declared under `bus.consume`, at the binding petal.
    pub fn subscribe(&self, key: &InstanceKey, address: &str, params: &str) -> Result<u64, HostError> {
        let now = self.now();
        let planned = parse_params(params).map_err(HostError::InvalidInput).and_then(|params| {
            let state = self.lock();
            let plan = self.plan_guest(&state, key, address, &params)?;
            Ok((plan, state.feeds.clone()))
        });
        let result = planned
            .and_then(|(plan, feeds)| self.materialize(&plan, &feeds).map(|detail| (plan, detail)))
            .and_then(|(plan, detail)| self.insert(plan, detail, now).map_err(BusRefusal::into_host_error));
        if let Err(error) = &result {
            if error.is_denial() {
                let installation = self.installation_of(key);
                let audit = audit_entry(AuditAction::Denial, &self.local_did, Some(CallerKind::Extension), installation, address, None, error.code(), now);
                self.finish(Effects { audits: vec![audit] });
            }
        }
        result
    }

    /// `bus.unsubscribe`: silent for unknown ids.
    pub fn unsubscribe(&self, key: &InstanceKey, subscription: u64) {
        let now = self.now();
        let removed = {
            let mut state = self.lock();
            let owned = state.subs.get(&subscription).is_some_and(|sub| sub.owned_by_instance(key));
            if owned {
                state.subs.remove(&subscription)
            } else {
                None
            }
        };
        if let Some(sub) = removed {
            let installation = self.installation_of(key);
            let audit = audit_entry(AuditAction::Unsubscribe, &sub.identity_did, Some(sub.caller_kind), installation, sub.address.to_string(), None, EndReason::Unsubscribed.as_str(), now);
            self.finish(Effects { audits: vec![audit] });
        }
    }

    /// `bus.refresh`: one coalesced re-run of an owned `host/query` subscription.
    pub fn refresh(&self, key: &InstanceKey, subscription: u64) -> Result<(), HostError> {
        let mut state = self.lock();
        match state.subs.get_mut(&subscription) {
            Some(sub) if sub.owned_by_instance(key) => match &mut sub.detail {
                Detail::Query(query) => {
                    query.mark_dirty();
                    Ok(())
                }
                _ => Err(HostError::InvalidInput("refresh applies to host/query subscriptions only".into())),
            },
            _ => Err(HostError::InvalidInput("unknown subscription".into())),
        }
    }

    /// `bus.publish`: a declared stream, a JSON event within its size, under its rate ceiling.
    pub fn publish(&self, key: &InstanceKey, stream_id: &str, event: &str) -> Result<(), HostError> {
        let now = self.now();
        let result = self.publish_locked(key, stream_id, event, now);
        if let Err(error) = &result {
            if error.is_denial() {
                let installation = self.installation_of(key);
                let target = format!("{}/{stream_id}", key.package_id);
                let audit = audit_entry(AuditAction::Denial, &self.local_did, Some(CallerKind::Extension), installation, target, None, error.code(), now);
                self.finish(Effects { audits: vec![audit] });
            }
        }
        result
    }

    fn publish_locked(&self, key: &InstanceKey, stream_id: &str, event: &str, now: u64) -> Result<(), HostError> {
        let mut state = self.lock();
        let BrokerState { instances, subs, clients, .. } = &mut *state;
        let instance = instances.get_mut(key).ok_or_else(|| HostError::Revoked("instance is not active".into()))?;
        let stream = exposed_stream(&instance.spec.manifest, stream_id)
            .ok_or_else(|| HostError::CapabilityDenied(format!("stream {stream_id} is not declared under bus.expose.streams")))?;
        let cap = usize::try_from(stream.max_event_bytes).unwrap_or(usize::MAX).min(MAX_STREAM_EVENT_BYTES);
        if event.len() > cap {
            return Err(HostError::InvalidInput(format!("event is {} bytes; max_event_bytes is {cap}", event.len())));
        }
        serde_json::from_str::<serde::de::IgnoredAny>(event)
            .map_err(|error| HostError::InvalidInput(format!("event is not JSON: {error}")))?;
        let rate = stream.max_rate_hz.min(MAX_STREAM_RATE_HZ);
        let allowed = instance
            .stream_buckets
            .entry(stream_id.to_string())
            .or_insert_with(|| TokenBucket::new(rate, now))
            .try_take(now);
        if !allowed {
            instance.counters.dropped_rate += 1;
            return Err(HostError::RateLimited(format!("stream {stream_id} is over max_rate_hz")));
        }
        instance.counters.published += 1;
        let producer_drops = fan_out_stream(subs, instances, clients, key, false, stream_id, Arc::from(event), now);
        if let Some(producer) = instances.get_mut(key) {
            producer.counters.dropped_queue += producer_drops;
        }
        Ok(())
    }

    /// `bus.call`: a foreign command listed under `bus.call`, at this instance's petal; answered via `on-call-result`.
    pub fn call(&self, key: &InstanceKey, target: &str, args: &str) -> Result<u64, HostError> {
        let now = self.now();
        let (package_id, command_id) = parse_command_address(target)
            .ok_or_else(|| HostError::InvalidInput(format!("{target} is not <package_id>/<command>")))?;
        let args: Value = serde_json::from_str(args).map_err(|error| HostError::InvalidInput(format!("args are not JSON: {error}")))?;
        let mut effects = Effects::default();
        let result = {
            let mut state = self.lock();
            let identity = state
                .instances
                .get(key)
                .map(|instance| instance.spec.identity_did.clone())
                .ok_or_else(|| HostError::Revoked("instance is not active".into()))?;
            let caller = Caller { did: identity, kind: CallerKind::Extension, package_id: Some(key.package_id.clone()), local: true, role_ceiling: None };
            match self.admit_call(&mut state, &caller, &key.petal_id, package_id, command_id, &args, now) {
                Ok((target_key, installation_id)) => {
                    let id = state.next_id();
                    let call = pending_call(target_key, installation_id, command_id, &args, caller, now, CallReply::Guest(key.clone()));
                    enqueue_call(&mut state, id, call);
                    Ok(id)
                }
                Err(refusal) => {
                    note_refusal(&mut state, &caller, &key.petal_id, package_id, command_id, &refusal, now, &mut effects);
                    Err(refusal.into_host_error())
                }
            }
        };
        self.finish(effects);
        result
    }

    // ---- external callers (REST, MCP, UI, portal IPC) -----------------------------------------

    /// bus.md §4 check 0: a remote DID's request rate on extension routes, before any lookup (never audited).
    pub fn admit_remote(&self, did: &str) -> bool {
        if did == &*self.local_did {
            return true;
        }
        let now = self.now();
        let mut state = self.lock();
        state
            .remote_gate
            .entry(did.to_string())
            .or_insert_with(|| TokenBucket::new(REMOTE_REQUEST_RATE_HZ, now))
            .try_take(now)
    }

    /// Call `<package_id>/<command>` at `petal_id` (bus.md §4 checks 1-8); remote callers see only the uniform not-found.
    pub fn call_command(&self, caller: &Caller, petal_id: &str, package_id: &str, command_id: &str, args: &Value) -> Result<CommandTicket, BusRefusal> {
        let caller = self.normalized(caller);
        match caller.kind {
            CallerKind::Extension => return Err(BusRefusal::InvalidInput("extension callers use the bus.call import".into())),
            // A portal's identity and ceiling come from its host-minted binding, never from a Caller literal.
            CallerKind::Portal => return Err(BusRefusal::InvalidInput("portal callers use Bus::client_call_command".into())),
            CallerKind::Ui | CallerKind::Rest | CallerKind::Mcp => {}
        }
        let now = self.now();
        let mut effects = Effects::default();
        let result = {
            let mut state = self.lock();
            match self.admit_call(&mut state, &caller, petal_id, package_id, command_id, args, now) {
                Ok((target, installation_id)) => {
                    let (reply, receiver) = oneshot::channel();
                    let id = state.next_id();
                    let call = pending_call(target, installation_id, command_id, args, caller.clone(), now, CallReply::External(reply));
                    enqueue_call(&mut state, id, call);
                    Ok(CommandTicket { call: id, reply: receiver })
                }
                Err(refusal) => {
                    note_refusal(&mut state, &caller, petal_id, package_id, command_id, &refusal, now, &mut effects);
                    Err(refusal)
                }
            }
        };
        self.finish(effects);
        result
    }

    /// Checks 1-8 of bus.md §4 in order; returns the target and its installation.
    #[allow(clippy::too_many_arguments)]
    fn admit_call(
        &self,
        state: &mut BrokerState,
        caller: &Caller,
        petal_id: &str,
        package_id: &str,
        command_id: &str,
        args: &Value,
        now: u64,
    ) -> Result<(InstanceKey, String), BusRefusal> {
        let remote = !caller.local;
        let hide = |local: BusRefusal| if remote { BusRefusal::NotFound } else { local };
        let address = format!("{package_id}/{command_id}");
        let target = InstanceKey::new(petal_id, package_id);
        // 1. an active instance at the petal
        let instance = state
            .instances
            .get(&target)
            .ok_or_else(|| hide(BusRefusal::NotAvailable(format!("no active {package_id} at this petal"))))?;
        // 2. the command is exposed
        let command: ExposedCommand = exposed_command(&instance.spec.manifest, command_id)
            .cloned()
            .ok_or_else(|| hide(BusRefusal::CapabilityDenied(format!("{address} is not exposed"))))?;
        let installation_id = instance.spec.installation_id.clone();
        // 3. remote reach is opt-in per command and per verse
        if remote && !(command.external == Some(true) && self.sources.policy.external_access_allowed(&instance.spec.verse_id)) {
            return Err(BusRefusal::NotFound);
        }
        // 4. the caller's role at the petal, read now
        if !self.role_of(caller, petal_id).is_at_least(role_level(command.role_floor)) {
            return Err(hide(BusRefusal::CapabilityDenied(format!("{address} needs role {:?}", command.role_floor))));
        }
        // 5. an extension caller lists the target and is not calling itself
        if caller.kind == CallerKind::Extension {
            let own = caller.package_id.clone().unwrap_or_default();
            let listed = state
                .instances
                .get(&InstanceKey::new(petal_id, own.clone()))
                .and_then(|calling| calling.spec.manifest.bus.as_ref())
                .and_then(|bus| bus.call.as_ref())
                .is_some_and(|targets| targets.contains(&address));
            if own == package_id || !listed {
                return Err(BusRefusal::CapabilityDenied(format!("{address} is not listed under bus.call")));
            }
        }
        // 6. per-caller and per-command rates, keyed per caller so one chatty caller never starves the UI
        let key = caller.key();
        let caller_ok = state
            .caller_buckets
            .entry(key.clone())
            .or_insert_with(|| TokenBucket::new(CALLER_COMMAND_RATE_HZ, now))
            .try_take(now);
        if !caller_ok {
            return Err(BusRefusal::RateLimited("caller command rate".into()));
        }
        let rate = command.rate_hz.min(MAX_COMMAND_RATE_HZ);
        let command_ok = state
            .command_buckets
            .entry((address.clone(), key))
            .or_insert_with(|| TokenBucket::new(rate, now))
            .try_take(now);
        if !command_ok {
            return Err(BusRefusal::RateLimited(format!("{address} is over rate_hz")));
        }
        // 7. args within size and schema
        if args.to_string().len() > MAX_COMMAND_ARGS_BYTES {
            return Err(BusRefusal::InvalidInput("args are over 16 KiB".into()));
        }
        validate_args(command.args_schema.as_ref(), args).map_err(BusRefusal::InvalidInput)?;
        // 8. the instance's command queue has room
        let queued = state.instances.get(&target).map_or(0, |instance| instance.commands.len());
        if queued >= COMMAND_QUEUE_PER_INSTANCE {
            return Err(BusRefusal::RateLimited("command queue full".into()));
        }
        Ok((target, installation_id))
    }

    // ---- clients (UI widgets, WS connections, portal IPC) -----------------------------------

    /// Open a client; `local` follows the DID.
    pub fn open_client(&self, did: &str, kind: CallerKind, grants: ClientGrants) -> ClientHandle {
        self.open_client_capped(did, kind, None, grants)
    }

    /// Open a client for a delegated API token: its `max_role` caps the DID's role.
    pub fn open_client_capped(&self, did: &str, kind: CallerKind, role_ceiling: Option<RoleLevel>, grants: ClientGrants) -> ClientHandle {
        let caller = match &grants {
            // A host-minted portal binding is local by DID; its ceiling (Viewer when absent: fail closed) caps the
            // role only (bus/AGENTS.md §portal-clients).
            ClientGrants::Portal(portal) => Caller {
                did: did.to_string(),
                kind: CallerKind::Portal,
                package_id: Some(portal.package_id.clone()),
                local: did == &*self.local_did,
                role_ceiling: Some(role_ceiling.unwrap_or(RoleLevel::Viewer)),
            },
            _ => Caller { did: did.to_string(), kind, package_id: None, local: self.is_local(did, role_ceiling), role_ceiling },
        };
        let notify = Arc::new(Notify::new());
        let mut state = self.lock();
        let id = state.next_id();
        state.clients.insert(id, ClientState { caller, grants, notify: notify.clone(), paused: false, stopped: None });
        ClientHandle { id, notify }
    }

    /// Open a mounted bundle's client: `installing_did` admitted it; `role_ceiling` is that identity's role at the
    /// bundle's scope, never above the viewer's token (steer 2026-11-08 23:40).
    pub fn open_portal_client(&self, installing_did: &str, role_ceiling: RoleLevel, grants: PortalGrants) -> ClientHandle {
        self.open_client_capped(installing_did, CallerKind::Portal, Some(role_ceiling), ClientGrants::Portal(Box::new(grants)))
    }

    /// A portal's `invoke`: a declared `commands.invoke` target at the mount petal, judged as the client's binding
    /// (installing DID, kind `portal`, its ceiling), then bus.md §4 checks 1-8.
    pub fn client_call_command(&self, client: u64, package_id: &str, command_id: &str, args: &Value) -> Result<CommandTicket, BusRefusal> {
        let now = self.now();
        let address = format!("{package_id}/{command_id}");
        let mut effects = Effects::default();
        let result = {
            let mut state = self.lock();
            match portal_client(&state, client) {
                Err(refusal) => Err(refusal),
                Ok((caller, grants)) => {
                    let petal_id = grants.petal_id.clone();
                    let admitted = if grants.invoke.contains(&address) {
                        self.admit_call(&mut state, &caller, &petal_id, package_id, command_id, args, now)
                    } else if caller.local {
                        Err(BusRefusal::CapabilityDenied(format!("{address} is not declared under commands.invoke")))
                    } else {
                        Err(BusRefusal::NotFound)
                    };
                    match admitted {
                        Ok((target, installation_id)) => {
                            let (reply, receiver) = oneshot::channel();
                            let id = state.next_id();
                            let call = pending_call(target, installation_id, command_id, args, caller, now, CallReply::External(reply));
                            enqueue_call(&mut state, id, call);
                            Ok(CommandTicket { call: id, reply: receiver })
                        }
                        Err(refusal) => {
                            note_refusal(&mut state, &caller, &petal_id, package_id, command_id, &refusal, now, &mut effects);
                            Err(refusal)
                        }
                    }
                }
            }
        };
        self.finish(effects);
        result
    }

    /// A portal's `emit`: publish on `<bundle package_id>/<stream_id>` at its mount petal; a declared stream, within
    /// its size and its rate (≤ 10/s, keyed per `(petal, package, stream)` so a reopened client cannot reset it).
    pub fn client_publish(&self, client: u64, stream_id: &str, event: &Value) -> Result<(), BusRefusal> {
        let now = self.now();
        let mut effects = Effects::default();
        let result = self.client_publish_locked(client, stream_id, &event.to_string(), now, &mut effects);
        self.finish(effects);
        result
    }

    fn client_publish_locked(&self, client: u64, stream_id: &str, event: &str, now: u64, effects: &mut Effects) -> Result<(), BusRefusal> {
        let mut state = self.lock();
        let (caller, grants) = portal_client(&state, client)?;
        let target = format!("{}/{stream_id}", grants.package_id);
        let Some(stream) = grants.streams.iter().find(|stream| stream.id == stream_id).filter(|_| caller.local) else {
            let refusal = if caller.local { BusRefusal::CapabilityDenied(format!("stream {stream_id} is not declared")) } else { BusRefusal::NotFound };
            effects.audits.push(audit_entry(AuditAction::Denial, &caller.did, Some(CallerKind::Portal), grants.installation_id.clone(), target, None, refusal.code(), now));
            return Err(refusal);
        };
        let cap = usize::try_from(stream.max_event_bytes).unwrap_or(usize::MAX).min(MAX_STREAM_EVENT_BYTES);
        if event.len() > cap {
            return Err(BusRefusal::InvalidInput(format!("event is {} bytes; max_event_bytes is {cap}", event.len())));
        }
        let producer = InstanceKey::new(grants.petal_id.clone(), grants.package_id.clone());
        let rate = stream.max_rate_hz.min(PORTAL_STREAM_RATE_HZ);
        let allowed = state
            .portal_stream_buckets
            .entry((producer.clone(), stream_id.to_string()))
            .or_insert_with(|| TokenBucket::new(rate, now))
            .try_take(now);
        if !allowed {
            return Err(BusRefusal::RateLimited(format!("stream {stream_id} is over max_rate_hz")));
        }
        let BrokerState { subs, instances, clients, .. } = &mut *state;
        fan_out_stream(subs, instances, clients, &producer, true, stream_id, Arc::from(event), now);
        Ok(())
    }

    /// A portal's `act set_property` (ipc-v1.md §6.1, A-13): ONE op on the extension write path outside any tick,
    /// provenance stamped from the binding, the installing identity's role at write time capped by the ceiling, and
    /// the caller's shared budgets; audited once with its outcome (bus/AGENTS.md §portal-writes).
    pub fn client_set_property(&self, client: u64, node_id: &str, key: &str, value: &Value) -> Result<WriteTicket, BusRefusal> {
        let now = self.now();
        let mut effects = Effects::default();
        let admitted = self.admit_write(client, node_id, key, value, now, &mut effects);
        self.finish(effects);
        match admitted? {
            WriteAdmission::Settled(ticket) => Ok(ticket),
            WriteAdmission::Send { ticket, write, reply, sink } => {
                sink.commit(&write, reply);
                // A synchronous write path answers in the same call.
                self.settle_writes();
                Ok(ticket)
            }
        }
    }

    /// Checks in ipc-v1.md §6.1 order under the lock: binding, declared key, shape, budgets, role; never the DB.
    fn admit_write(&self, client: u64, node_id: &str, key: &str, value: &Value, now: u64, effects: &mut Effects) -> Result<WriteAdmission, BusRefusal> {
        let mut state = self.lock();
        let (caller, grants) = portal_client(&state, client)?;
        let target = format!("{}/set_property/{node_id}/{key}", grants.package_id);
        let deny = |refusal: BusRefusal, effects: &mut Effects| {
            effects.audits.push(audit_entry(AuditAction::Denial, &caller.did, Some(CallerKind::Portal), grants.installation_id.clone(), target.clone(), None, refusal.code(), now));
            refusal
        };
        // Writes are the host identity's (the DB writes as the local DID): a binding for any other DID never writes.
        if !caller.local {
            return Err(deny(BusRefusal::NotFound, effects));
        }
        // 1. a declared, non-reserved key; nothing reaches the DB otherwise
        if !portal_key_allowed(&grants.set_property_keys, key) {
            return Err(deny(BusRefusal::CapabilityDenied(format!("{key} is not a declared set_property key")), effects));
        }
        // Provenance is stamped from the binding; a binding without a valid one never writes (R24).
        if !is_package_id(&grants.package_id) || !is_content_hash(&grants.content_hash) {
            return Err(BusRefusal::NotAvailable("the mount binding carries no valid provenance".into()));
        }
        let Some(sink) = self.portal_writes.get().cloned() else {
            return Err(BusRefusal::NotAvailable("portal writes are not served by this host".into()));
        };
        // 2-3. shape and size (petal membership is the DB's to prove at use)
        if !is_node_id(node_id) {
            return Err(BusRefusal::InvalidInput("node_id is not a node id".into()));
        }
        if value.to_string().len() > MAX_COMMAND_ARGS_BYTES {
            return Err(BusRefusal::InvalidInput("value is over 16 KiB".into()));
        }
        // Shared budgets: the caller key's command budget (shared with invoke), the package's write budget and the
        // installing identity's write budget across every bundle.
        let caller_ok = state
            .caller_buckets
            .entry(caller.key())
            .or_insert_with(|| TokenBucket::new(CALLER_COMMAND_RATE_HZ, now))
            .try_take(now);
        let package_ok = caller_ok
            && state
                .write_buckets
                .entry(grants.package_id.clone())
                .or_insert_with(|| TokenBucket::with_burst(PORTAL_WRITE_RATE_HZ, PORTAL_WRITE_BURST, now))
                .try_take(now);
        let identity_ok = package_ok
            && state
                .identity_write_buckets
                .entry(caller.did.clone())
                .or_insert_with(|| TokenBucket::with_burst(PORTAL_WRITE_IDENTITY_RATE_HZ, PORTAL_WRITE_IDENTITY_BURST, now))
                .try_take(now);
        if !identity_ok {
            return Err(BusRefusal::RateLimited("set_property budget".into()));
        }
        // 4. the role at write time, capped by the binding's ceiling; the DB re-checks the role and ancestry at use.
        let id = state.next_id();
        let (answer, receiver) = oneshot::channel();
        let ticket = WriteTicket { write: id, reply: receiver };
        let audit = audit_entry(AuditAction::PropertyWrite, &caller.did, Some(CallerKind::Portal), grants.installation_id.clone(), target.clone(), Some(json_digest(value)), "pending", now);
        let role = self.role_of(&caller, &grants.petal_id);
        if !role.is_at_least(PORTAL_WRITE_FLOOR) {
            let outcome = WriteOutcome::RoleDenied(format!("role {role} at the petal is below editor"));
            effects.audits.push(AuditEntry { outcome: outcome.code().to_string(), ..audit });
            let _ = answer.send(outcome);
            return Ok(WriteAdmission::Settled(ticket));
        }
        let (reply, pending) = crossbeam::channel::bounded(1);
        state.writes.insert(id, PendingWrite { reply: pending, answer, audit, deadline_ms: now + PORTAL_WRITE_TIMEOUT_MS });
        let write = PortalWrite {
            write: id,
            identity_did: caller.did.clone(),
            package_id: grants.package_id.clone(),
            content_hash: grants.content_hash.clone(),
            installation_id: grants.installation_id.clone(),
            verse_id: grants.verse_id.clone(),
            petal_id: grants.petal_id.clone(),
            op: PendingOp::SetProperty { node_id: node_id.to_string(), key: key.to_string(), value: value.clone() },
        };
        Ok(WriteAdmission::Send { ticket, write: Box::new(write), reply, sink })
    }

    /// Answer and audit portal writes the write path settled, dropped or left past their deadline.
    fn settle_writes(&self) {
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let settled: Vec<(u64, WriteOutcome)> = state
                .writes
                .iter()
                .filter_map(|(id, write)| {
                    let outcome = match write.reply.try_recv() {
                        Ok(result) => write_outcome(result),
                        Err(TryRecvError::Disconnected) => {
                            WriteOutcome::Refused(PluginRefusal::UnknownOutcome("the write path dropped the reply".into()))
                        }
                        Err(TryRecvError::Empty) if write.deadline_ms <= now => {
                            WriteOutcome::Refused(PluginRefusal::UnknownOutcome("no reply within 5 s".into()))
                        }
                        Err(TryRecvError::Empty) => return None,
                    };
                    Some((*id, outcome))
                })
                .collect();
            for (id, outcome) in settled {
                if let Some(write) = state.writes.remove(&id) {
                    effects.audits.push(AuditEntry { outcome: outcome.code().to_string(), at_ms: now, ..write.audit });
                    let _ = write.answer.send(outcome);
                }
            }
        }
        self.finish(effects);
    }

    /// Subscribe a client at `petal_id`; refusals for remote callers are the uniform not-found.
    pub fn client_subscribe(&self, client: u64, petal_id: &str, address: &str, params: &Value) -> Result<u64, BusRefusal> {
        let now = self.now();
        let (caller, planned) = {
            let state = self.lock();
            let Some(client_state) = state.clients.get(&client) else { return Err(BusRefusal::NotFound) };
            let caller = client_state.caller.clone();
            let planned = self.plan_client(&state, client, petal_id, address, params).map(|plan| (plan, state.feeds.clone()));
            (caller, planned)
        };
        let remote = !caller.local;
        let result = planned.and_then(|(plan, feeds)| {
            let detail = self.materialize(&plan, &feeds).map_err(|error| refusal_for(error, remote))?;
            self.insert(plan, detail, now)
        });
        if let Err(refusal) = &result {
            if matches!(refusal, BusRefusal::NotFound | BusRefusal::CapabilityDenied(_)) {
                let audit = audit_entry(AuditAction::Denial, &caller.did, Some(caller.kind), None, address, None, refusal.code(), now);
                self.finish(Effects { audits: vec![audit] });
            }
        }
        result
    }

    /// End one of a client's subscriptions with `ended {"reason":"unsubscribed"}`.
    pub fn client_unsubscribe(&self, client: u64, subscription: u64) {
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let BrokerState { subs, clients, .. } = &mut *state;
            if let Some(sub) = subs.get_mut(&subscription).filter(|sub| sub.owned_by_client(client) && !sub.closed) {
                sub.end(EndReason::Unsubscribed, now);
                effects.audits.push(audit_entry(AuditAction::Unsubscribe, &sub.identity_did, Some(sub.caller_kind), None, sub.address.to_string(), None, EndReason::Unsubscribed.as_str(), now));
                notify_client(clients, &sub.subscriber);
            }
        }
        self.finish(effects);
    }

    /// A hidden portal pauses its query runs; they stay dirty and run once on resume.
    pub fn set_client_paused(&self, client: u64, paused: bool) {
        if let Some(state) = self.lock().clients.get_mut(&client) {
            state.paused = paused;
        }
    }

    /// Up to `max` queued events across the client's subscriptions, in id then `seq` order.
    pub fn take_client_events(&self, client: u64, max: usize) -> Vec<BusEvent> {
        let mut state = self.lock();
        let BrokerState { subs, instances, .. } = &mut *state;
        let mut events = Vec::new();
        let mut finished = Vec::new();
        for sub in subs.values_mut().filter(|sub| sub.owned_by_client(client)) {
            let owner = sub.counter_owner();
            let mut taken = 0u64;
            while events.len() < max {
                let Some(event) = sub.pop() else { break };
                events.push(event);
                taken += 1;
            }
            bump(instances, owner.as_ref(), |counters| counters.delivered += taken);
            if sub.finished() {
                finished.push(sub.id);
            }
        }
        for id in finished {
            subs.remove(&id);
        }
        events
    }

    /// Close a client and drop all its subscriptions; a portal's in-flight writes still settle (OD-E1), and
    /// subscribers of streams no live portal client still declares get `ended {producer-inactive}`.
    pub fn close_client(&self, client: u64) {
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let closed = state.clients.remove(&client);
            let owned: Vec<u64> = state.subs.values().filter(|sub| sub.owned_by_client(client)).map(|sub| sub.id).collect();
            for id in owned {
                if let Some(sub) = state.subs.remove(&id) {
                    if !sub.closed {
                        effects.audits.push(audit_entry(AuditAction::Unsubscribe, &sub.identity_did, Some(sub.caller_kind), None, sub.address.to_string(), None, EndReason::Unsubscribed.as_str(), now));
                    }
                }
            }
            if closed.is_some_and(|closed| matches!(closed.grants, ClientGrants::Portal(_))) {
                let BrokerState { subs, clients, .. } = &mut *state;
                end_orphaned_portal_streams(subs, clients, now);
            }
        }
        self.finish(effects);
    }

    // ---- host feeds (injected by the binary) ---------------------------------------------------

    /// Committed scene changes, local and replicated; `transform_failed` never reaches the bus.
    pub fn feed_node_changes(&self, changes: &[SceneChange]) {
        if changes.is_empty() {
            return;
        }
        let now = self.now();
        let mut state = self.lock();
        let BrokerState { subs, instances, clients, .. } = &mut *state;
        let encoded: Vec<(&str, Option<&str>, Value)> = changes
            .iter()
            .filter(|change| !matches!(change, SceneChange::TransformFailed { .. }))
            .filter_map(|change| {
                let key = match change {
                    SceneChange::PropertyChanged { key, .. } => Some(key.as_str()),
                    _ => None,
                };
                serde_json::to_value(change).ok().map(|json| (change.petal_id(), key, json))
            })
            .collect();
        for sub in subs.values_mut() {
            if sub.closed {
                continue;
            }
            let petal = sub.petal_id.as_str();
            let mut overflowed = false;
            match &mut sub.detail {
                Detail::Query(query) => {
                    if query.reads("node") && encoded.iter().any(|(change_petal, _, _)| *change_petal == petal) {
                        query.mark_dirty();
                    }
                }
                Detail::NodeChanges { property_scope, pending, .. } => {
                    for (change_petal, key, json) in &encoded {
                        if *change_petal != petal {
                            continue;
                        }
                        if let Some(key) = key {
                            if !property_scope.as_deref().is_some_and(|scope| scope_allows(scope, key)) {
                                continue;
                            }
                        }
                        pending.push(json.clone());
                    }
                    overflowed = pending.len() > sub.capacity * NODE_DELTA_CHANGES;
                }
                _ => {}
            }
            if overflowed {
                let displaced = sub.resync(now);
                bump(instances, sub.counter_owner().as_ref(), |counters| counters.dropped_queue += displaced);
                notify_client(clients, &sub.subscriber);
            }
        }
    }

    /// The scene feed lagged: every node-changes subscription (at `petal_id`, or everywhere) resyncs.
    pub fn feed_node_changes_lagged(&self, petal_id: Option<&str>) {
        let now = self.now();
        let mut state = self.lock();
        let BrokerState { subs, instances, clients, .. } = &mut *state;
        for sub in subs.values_mut() {
            let here = petal_id.is_none_or(|petal| petal == sub.petal_id);
            if here && !sub.closed && matches!(sub.detail, Detail::NodeChanges { .. }) {
                let displaced = sub.resync(now);
                bump(instances, sub.counter_owner().as_ref(), |counters| counters.dropped_queue += displaced);
                notify_client(clients, &sub.subscriber);
            }
        }
    }

    /// Readings accepted by the readings door, local and replicated.
    pub fn feed_readings(&self, readings: &[ReadingEvent]) {
        if readings.is_empty() {
            return;
        }
        let now = self.now();
        let rows: Vec<(&ReadingEvent, Value)> =
            readings.iter().map(|reading| (reading, serde_json::to_value(&reading.row).unwrap_or(Value::Null))).collect();
        let mut state = self.lock();
        let BrokerState { subs, instances, clients, .. } = &mut *state;
        for sub in subs.values_mut() {
            if sub.closed {
                continue;
            }
            let petal = sub.petal_id.as_str();
            let matching: Vec<&Value> = match &mut sub.detail {
                Detail::Readings { metrics, anchors } => rows
                    .iter()
                    .filter(|(reading, _)| {
                        reading.petal_id == petal
                            && metrics.as_deref().is_none_or(|metrics| metric_allowed(metrics, &reading.row.metric))
                            && anchors.as_ref().is_none_or(|anchors| anchors.contains(&reading.row.node_id))
                    })
                    .map(|(_, json)| json)
                    .collect(),
                Detail::Query(query) => {
                    if query.reads("iot_reading") && readings.iter().any(|reading| reading.petal_id == petal) {
                        query.mark_dirty();
                    }
                    continue;
                }
                _ => continue,
            };
            if matching.is_empty() {
                continue;
            }
            let mut displaced = 0;
            for chunk in matching.chunks(READINGS_PER_EVENT) {
                let data = json!({ "readings": chunk }).to_string();
                displaced += sub.push(DeliveryKind::Delta, Arc::from(data), now);
            }
            bump(instances, sub.counter_owner().as_ref(), |counters| counters.dropped_queue += displaced);
            notify_client(clients, &sub.subscriber);
        }
    }

    /// An admission change on this host (spec R23); visibility per bus.md §2.4. A web bundle leaving `admitted`
    /// stops its portal clients in the same call (bus/AGENTS.md §portal-clients).
    pub fn feed_admission(&self, event: &AdmissionEvent) {
        let now = self.now();
        let data: Arc<str> = Arc::from(event.to_json().to_string());
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let BrokerState { subs, instances, clients, .. } = &mut *state;
            for sub in subs.values_mut() {
                let visible = match &sub.detail {
                    Detail::Admission { visible } => visible.as_ref().is_none_or(|packages| packages.contains(&event.package_id)),
                    _ => false,
                };
                if visible && !sub.closed {
                    let displaced = sub.push(DeliveryKind::Delta, data.clone(), now);
                    bump(instances, sub.counter_owner().as_ref(), |counters| counters.dropped_queue += displaced);
                    notify_client(clients, &sub.subscriber);
                }
            }
            if event.kind == AdmissionKind::WebBundle {
                if let Some(reason) = portal_end_reason(event.reason) {
                    stop_portal_clients(subs, clients, event, reason, now, &mut effects);
                }
            }
        }
        self.finish(effects);
    }

    /// A write to `table` in `petal_id` from a feed the binary enabled (e.g. `device_payload`).
    pub fn feed_table_write(&self, table: &str, petal_id: &str) {
        let table = table.to_ascii_lowercase();
        let mut state = self.lock();
        for sub in state.subs.values_mut() {
            if sub.petal_id != petal_id {
                continue;
            }
            if let Detail::Query(query) = &mut sub.detail {
                if query.reads(&table) {
                    query.mark_dirty();
                }
            }
        }
    }

    /// Declare that `table` now has a change feed, lifting the subscribe-time refusal.
    pub fn enable_change_feed(&self, table: &str) {
        self.lock().feeds.insert(table.to_ascii_lowercase());
    }

    /// A role, binding or verse-policy change: queries re-run, stream subscriptions below their floor end.
    pub fn revalidate(&self, petal_id: Option<&str>) {
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let BrokerState { subs, clients, .. } = &mut *state;
            for sub in subs.values_mut() {
                if sub.closed || petal_id.is_some_and(|petal| petal != sub.petal_id) {
                    continue;
                }
                let identity = sub.identity_did.as_str();
                let petal = sub.petal_id.as_str();
                let end = match &mut sub.detail {
                    Detail::Query(query) => {
                        query.mark_dirty();
                        None
                    }
                    Detail::Stream { role_floor, external_required, verse_id, .. } => {
                        if !self.sources.roles.role_at(identity, petal).is_at_least(*role_floor) {
                            Some(EndReason::RoleLost)
                        } else if *external_required && !self.sources.policy.external_access_allowed(verse_id) {
                            Some(EndReason::PolicyDenied)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(reason) = end {
                    sub.end(reason, now);
                    effects.audits.push(audit_entry(AuditAction::Unsubscribe, &sub.identity_did, Some(sub.caller_kind), None, sub.address.to_string(), None, reason.as_str(), now));
                    notify_client(clients, &sub.subscriber);
                }
            }
        }
        self.finish(effects);
    }

    // ---- the worker ------------------------------------------------------------------------------

    /// One broker turn on the plugin worker: settle runs and snapshots, start due runs, expire calls, settle writes.
    pub fn poll(&self) {
        self.settle();
        self.launch();
        self.settle();
        self.expire_calls();
        self.settle_writes();
    }

    /// Hand at most `max_calls` units of work to one instance's hooks; returns how many ran.
    pub fn dispatch(&self, key: &InstanceKey, hooks: &mut dyn BusHooks, max_calls: usize) -> usize {
        let mut calls = 0;
        while calls < max_calls {
            let Some(work) = self.next_work(key) else { break };
            calls += 1;
            match work {
                GuestWork::CallResult { call, outcome } => hooks.on_call_result(call, outcome),
                GuestWork::Command { call, command, args, caller } => {
                    let result = hooks.on_command(&command, &args, &caller);
                    self.complete_command(call, result);
                }
                GuestWork::Events(events) => hooks.on_events(events),
            }
        }
        calls
    }

    /// The next unit of work for an instance; a `Command` must be answered with [`Bus::complete_command`].
    pub fn next_work(&self, key: &InstanceKey) -> Option<GuestWork> {
        let mut state = self.lock();
        let BrokerState { instances, calls, subs, .. } = &mut *state;
        let instance = instances.get_mut(key)?;
        if let Some((call, outcome)) = instance.call_results.pop_front() {
            return Some(GuestWork::CallResult { call, outcome });
        }
        while let Some(id) = instance.commands.pop_front() {
            if let Some(call) = calls.get(&id) {
                return Some(GuestWork::Command { call: id, command: call.command.clone(), args: call.args.clone(), caller: call.caller.clone() });
            }
        }
        let events = take_guest_batch(subs, key, &mut instance.rr_cursor);
        if events.is_empty() {
            return None;
        }
        instance.counters.delivered += events.len() as u64;
        Some(GuestWork::Events(events))
    }

    /// The guest's answer to a dispatched command; results over 64 KiB or not JSON are refused.
    pub fn complete_command(&self, call: u64, result: Result<String, CommandError>) {
        let outcome = match result {
            Ok(json) if json.len() > MAX_COMMAND_RESULT_BYTES => {
                CommandOutcome::Host(HostError::BudgetExceeded("command result is over 64 KiB".into()))
            }
            Ok(json) if serde_json::from_str::<serde::de::IgnoredAny>(&json).is_err() => {
                CommandOutcome::Guest(CommandError::Failed("command result is not JSON".into()))
            }
            Ok(json) => CommandOutcome::Ok(json),
            Err(error) => CommandOutcome::Guest(error),
        };
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let BrokerState { calls, instances, .. } = &mut *state;
            if let Some(pending) = calls.remove(&call) {
                resolve_call(instances, call, pending, outcome, now, &mut effects);
            }
        }
        self.finish(effects);
    }

    // ---- status --------------------------------------------------------------------------------

    /// Live counters of one instance.
    pub fn counters(&self, key: &InstanceKey) -> Option<BusCounters> {
        let state = self.lock();
        let instance = state.instances.get(key)?;
        let mut counters = instance.counters.clone();
        counters.subscriptions = state.live_subscriptions(&Subscriber::Instance(key.clone())) as u64;
        Some(counters)
    }

    /// Counters by installation, kept after the instance stops until [`Bus::forget`].
    pub fn counters_for_installation(&self, installation_id: &str) -> Option<BusCounters> {
        let key = {
            let state = self.lock();
            if let Some(retired) = state.retired.get(installation_id) {
                return Some(retired.clone());
            }
            state.instances.values().find(|instance| instance.spec.installation_id == installation_id).map(|instance| instance.spec.key())
        };
        key.and_then(|key| self.counters(&key))
    }

    /// Drop a stopped installation's counters (uninstall).
    pub fn forget(&self, installation_id: &str) {
        self.lock().retired.remove(installation_id);
    }

    fn installation_of(&self, key: &InstanceKey) -> Option<String> {
        self.lock().instances.get(key).map(|instance| instance.spec.installation_id.clone())
    }

    // ---- internals -----------------------------------------------------------------------------

    fn plan_guest(&self, state: &BrokerState, key: &InstanceKey, address: &str, params: &Map<String, Value>) -> Result<Plan, HostError> {
        let instance = state.instances.get(key).ok_or_else(|| HostError::Revoked("instance is not active".into()))?;
        if state.live_subscriptions(&Subscriber::Instance(key.clone())) >= MAX_SUBSCRIPTIONS_PER_INSTANCE {
            return Err(HostError::BudgetExceeded(format!("at most {MAX_SUBSCRIPTIONS_PER_INSTANCE} subscriptions per instance")));
        }
        let source = Source::parse(address).ok_or_else(|| HostError::InvalidInput(format!("{address} is not a bus source")))?;
        let manifest = &instance.spec.manifest;
        let bus = manifest.bus.as_ref();
        let consume: &[ConsumeEntry] = bus.and_then(|bus| bus.consume.as_deref()).unwrap_or(&[]);
        let kind = match &source {
            Source::Query => {
                let (sql, entry) = match (params.get("query").and_then(Value::as_str), params.get("sql").and_then(Value::as_str)) {
                    (Some(id), None) => {
                        let entry = consume
                            .iter()
                            .find(|entry| entry.source == "host/query" && entry.id.as_deref() == Some(id) && entry.sql.as_deref() != Some("*"))
                            .ok_or_else(|| HostError::CapabilityDenied(format!("named query {id} is not declared under bus.consume")))?;
                        (entry.sql.clone().unwrap_or_default(), entry)
                    }
                    (None, Some(sql)) => {
                        let entry = consume
                            .iter()
                            .find(|entry| entry.source == "host/query" && entry.sql.as_deref() == Some("*"))
                            .ok_or_else(|| HostError::CapabilityDenied("ad-hoc host/query SQL needs sql: \"*\" under bus.consume".into()))?;
                        (sql.to_string(), entry)
                    }
                    _ => return Err(HostError::InvalidInput("host/query params need exactly one of query or sql".into())),
                };
                let min_interval_ms = entry.min_interval_ms.unwrap_or(QUERY_MIN_INTERVAL_FLOOR_MS).max(QUERY_MIN_INTERVAL_FLOOR_MS);
                PlanKind::Query { sql, vars: query_vars(params)?, min_interval_ms }
            }
            Source::NodeChanges => {
                consumed(consume, address)?;
                let scene = manifest.scene.as_ref().filter(|scene| scene.read);
                let scene = scene.ok_or_else(|| HostError::CapabilityDenied("host/node-changes needs scene.read".into()))?;
                PlanKind::NodeChanges { property_scope: scene.property_scope.clone() }
            }
            Source::Readings => readings_plan(consumed(consume, address)?.metrics.as_deref(), params)?,
            Source::Admission => {
                consumed(consume, address)?;
                let mut visible: HashSet<String> = HashSet::from([key.package_id.clone()]);
                for entry in consume {
                    if let Some(Source::Stream { package_id, .. }) = Source::parse(&entry.source) {
                        visible.insert(package_id);
                    }
                }
                for target in bus.and_then(|bus| bus.call.as_deref()).unwrap_or(&[]) {
                    if let Some((package_id, _)) = parse_command_address(target) {
                        visible.insert(package_id.to_string());
                    }
                }
                PlanKind::Admission { visible: Some(visible) }
            }
            Source::Stream { package_id, stream_id } => {
                consumed(consume, address)?;
                let producer_key = InstanceKey::new(key.petal_id.clone(), package_id.clone());
                let producer = match lookup_producer(state, &producer_key, stream_id) {
                    ProducerLookup::Missing => return Err(HostError::NotAvailable(format!("{address}: producer-inactive"))),
                    ProducerLookup::Undeclared => return Err(HostError::NotAvailable(format!("{address} is not exposed by the producer"))),
                    ProducerLookup::Found(producer) => producer,
                };
                if producer.consumers.as_ref().is_some_and(|consumers| !consumers.contains(&key.package_id)) {
                    return Err(HostError::CapabilityDenied(format!("{address} is restricted to other consumers")));
                }
                if !self.sources.roles.role_at(&instance.spec.identity_did, &key.petal_id).is_at_least(producer.role_floor) {
                    return Err(HostError::CapabilityDenied(format!("{address} is above the installing identity's role")));
                }
                PlanKind::Stream {
                    producer: producer_key,
                    stream_id: stream_id.clone(),
                    role_floor: producer.role_floor,
                    external_required: false,
                    verse_id: producer.verse_id,
                    from_portal: producer.from_portal,
                }
            }
        };
        Ok(Plan {
            subscriber: Subscriber::Instance(key.clone()),
            address: address.to_string(),
            petal_id: key.petal_id.clone(),
            verse_id: instance.spec.verse_id.clone(),
            identity_did: instance.spec.identity_did.clone(),
            caller_kind: CallerKind::Extension,
            guest: true,
            installation_id: Some(instance.spec.installation_id.clone()),
            kind,
        })
    }

    fn plan_client(&self, state: &BrokerState, client: u64, petal_id: &str, address: &str, params: &Value) -> Result<Plan, BusRefusal> {
        let client_state = state.clients.get(&client).ok_or(BusRefusal::NotFound)?;
        let caller = &client_state.caller;
        let remote = !caller.local;
        let hide = |local: BusRefusal| if remote { BusRefusal::NotFound } else { local };
        if let Some(reason) = client_state.stopped {
            return Err(hide(BusRefusal::NotAvailable(format!("the client stopped: {}", reason.as_str()))));
        }
        if state.live_subscriptions(&Subscriber::Client(client)) >= MAX_SUBSCRIPTIONS_PER_CLIENT {
            return Err(BusRefusal::LimitReached(format!("at most {MAX_SUBSCRIPTIONS_PER_CLIENT} subscriptions per connection")));
        }
        let empty = Map::new();
        let params = match params {
            Value::Object(map) => map,
            Value::Null => &empty,
            _ => return Err(hide(BusRefusal::InvalidInput("params must be a JSON object".into()))),
        };
        let source = Source::parse(address).ok_or_else(|| hide(BusRefusal::InvalidInput(format!("{address} is not a bus source"))))?;
        let not_granted = || hide(BusRefusal::CapabilityDenied(format!("{address} is not available to this client")));
        let mut verse_id = String::new();
        let kind = match (&source, &client_state.grants) {
            (Source::Stream { package_id, stream_id }, grants) => {
                // A portal consumes declared hooks at its mount petal only.
                if let ClientGrants::Portal(portal) = grants {
                    if portal.petal_id != petal_id || !portal.consume.iter().any(|entry| entry.source == address) {
                        return Err(not_granted());
                    }
                }
                let producer_key = InstanceKey::new(petal_id, package_id.clone());
                let producer = match lookup_producer(state, &producer_key, stream_id) {
                    ProducerLookup::Missing => return Err(hide(BusRefusal::NotAvailable(format!("{address}: producer-inactive")))),
                    ProducerLookup::Undeclared => return Err(not_granted()),
                    ProducerLookup::Found(producer) => producer,
                };
                if remote && !(producer.external && self.sources.policy.external_access_allowed(&producer.verse_id)) {
                    return Err(BusRefusal::NotFound);
                }
                if let (ClientGrants::Portal(portal), Some(consumers)) = (grants, &producer.consumers) {
                    if !consumers.contains(&portal.package_id) {
                        return Err(not_granted());
                    }
                }
                if !self.role_of(caller, petal_id).is_at_least(producer.role_floor) {
                    return Err(not_granted());
                }
                verse_id = producer.verse_id.clone();
                PlanKind::Stream {
                    producer: producer_key,
                    stream_id: stream_id.clone(),
                    role_floor: producer.role_floor,
                    external_required: remote,
                    verse_id: verse_id.clone(),
                    from_portal: producer.from_portal,
                }
            }
            (Source::Admission, ClientGrants::Ui) => PlanKind::Admission { visible: None },
            (_, ClientGrants::Portal(portal)) => {
                let PortalGrants { petal_id: mount, verse_id: mount_verse, consume, property_scope, .. } = &**portal;
                // Host sources are this host's data: a foreign DID's binding, or a ceiling below viewer, reads none of it.
                if mount != petal_id || remote || !self.role_of(caller, petal_id).is_at_least(RoleLevel::Viewer) {
                    return Err(not_granted());
                }
                verse_id = mount_verse.clone();
                match &source {
                    Source::Admission if consume.iter().any(|entry| entry.source == address) => PlanKind::Admission { visible: None },
                    Source::NodeChanges if consume.iter().any(|entry| entry.source == address) => {
                        PlanKind::NodeChanges { property_scope: property_scope.clone() }
                    }
                    Source::Readings => {
                        let entry = consume.iter().find(|entry| entry.source == address).ok_or_else(not_granted)?;
                        readings_plan(entry.metrics.as_deref(), params).map_err(|error| refusal_for(error, remote))?
                    }
                    Source::Query => {
                        // Portals use declared named queries only, never ad-hoc SQL (bus.md §8).
                        let id = params.get("query").and_then(Value::as_str).ok_or_else(not_granted)?;
                        let entry = consume
                            .iter()
                            .find(|entry| entry.source == "host/query" && entry.id.as_deref() == Some(id) && entry.sql.as_deref() != Some("*"))
                            .ok_or_else(not_granted)?;
                        let min_interval_ms = entry.min_interval_ms.unwrap_or(QUERY_MIN_INTERVAL_FLOOR_MS).max(QUERY_MIN_INTERVAL_FLOOR_MS);
                        let vars = query_vars(params).map_err(|error| refusal_for(error, remote))?;
                        PlanKind::Query { sql: entry.sql.clone().unwrap_or_default(), vars, min_interval_ms }
                    }
                    _ => return Err(not_granted()),
                }
            }
            _ => return Err(not_granted()),
        };
        Ok(Plan {
            subscriber: Subscriber::Client(client),
            address: address.to_string(),
            petal_id: petal_id.to_string(),
            verse_id,
            identity_did: caller.did.clone(),
            caller_kind: caller.kind,
            guest: false,
            // A portal's subscribe audit carries its web-bundle installation (A-14).
            installation_id: match &client_state.grants {
                ClientGrants::Portal(portal) => portal.installation_id.clone(),
                _ => None,
            },
            kind,
        })
    }

    /// Call the injected services a plan needs (outside the lock) and build its state.
    fn materialize(&self, plan: &Plan, feeds: &HashSet<String>) -> Result<Detail, HostError> {
        Ok(match &plan.kind {
            PlanKind::Query { sql, vars, min_interval_ms } => {
                let source = self.sources.query.as_ref().ok_or_else(|| HostError::NotAvailable("host/query is not served by this host".into()))?;
                let request = QueryRequest {
                    identity_did: plan.identity_did.clone(),
                    verse_id: plan.verse_id.clone(),
                    petal_id: plan.petal_id.clone(),
                    sql: sql.clone(),
                    vars: vars.clone(),
                };
                let prepared = source.prepare(&request)?;
                let query = QueryState::new(request, prepared, *min_interval_ms, plan.guest.then_some(GUEST_QUERY_EVENT_BYTES));
                if let Some(table) = query.tables().find(|table| !feeds.contains(*table)) {
                    return Err(HostError::NotAvailable(format!("{table} has no change feed")));
                }
                Detail::Query(Box::new(query))
            }
            PlanKind::NodeChanges { property_scope } => {
                if self.sources.scene.is_none() {
                    return Err(HostError::NotAvailable("host/node-changes is not served by this host".into()));
                }
                Detail::NodeChanges { property_scope: property_scope.clone(), snapshot: SnapshotState::Needed, ready: false, pending: Vec::new() }
            }
            PlanKind::Readings { metrics, anchors } => Detail::Readings { metrics: metrics.clone(), anchors: anchors.clone() },
            PlanKind::Admission { visible } => Detail::Admission { visible: visible.clone() },
            PlanKind::Stream { producer, stream_id, role_floor, external_required, verse_id, from_portal } => Detail::Stream {
                producer: producer.clone(),
                stream_id: stream_id.clone(),
                role_floor: *role_floor,
                external_required: *external_required,
                verse_id: verse_id.clone(),
                from_portal: *from_portal,
            },
        })
    }

    /// Insert under the lock, re-checking that the subscriber still exists and has room.
    fn insert(&self, plan: Plan, detail: Detail, now: u64) -> Result<u64, BusRefusal> {
        let (capacity, overflow) = match (&detail, plan.guest) {
            (Detail::Query(_), _) => (1, Overflow::ReplaceLatest),
            (Detail::Admission { .. }, _) => (ADMISSION_QUEUE, Overflow::DropOldest),
            (Detail::NodeChanges { .. }, true) => (GUEST_QUEUE, Overflow::Resync),
            (Detail::NodeChanges { .. }, false) => (EXTERNAL_QUEUE, Overflow::Resync),
            (_, true) => (GUEST_QUEUE, Overflow::DropOldest),
            (_, false) => (EXTERNAL_QUEUE, Overflow::DropOldest),
        };
        let audit = {
            let mut state = self.lock();
            let present = match &plan.subscriber {
                Subscriber::Instance(key) => state.instances.contains_key(key),
                Subscriber::Client(client) => state.clients.contains_key(client),
            };
            if !present {
                return Err(BusRefusal::NotAvailable("the subscriber stopped".into()));
            }
            let limit = if plan.guest { MAX_SUBSCRIPTIONS_PER_INSTANCE } else { MAX_SUBSCRIPTIONS_PER_CLIENT };
            if state.live_subscriptions(&plan.subscriber) >= limit {
                return Err(BusRefusal::LimitReached(format!("at most {limit} subscriptions")));
            }
            let id = state.next_id();
            state.subs.insert(
                id,
                Subscription {
                    id,
                    subscriber: plan.subscriber,
                    address: Arc::from(plan.address.as_str()),
                    petal_id: plan.petal_id,
                    identity_did: plan.identity_did.clone(),
                    caller_kind: plan.caller_kind,
                    queue: VecDeque::new(),
                    capacity,
                    overflow,
                    next_seq: 1,
                    dropped_pending: 0,
                    closed: false,
                    detail,
                },
            );
            (id, audit_entry(AuditAction::Subscribe, &plan.identity_did, Some(plan.caller_kind), plan.installation_id, plan.address, None, "ok", now))
        };
        self.finish(Effects { audits: vec![audit.1] });
        Ok(audit.0)
    }

    /// Collect finished query runs and snapshots; coalesce pending scene changes into deltas.
    fn settle(&self) {
        let now = self.now();
        let mut state = self.lock();
        let BrokerState { subs, instances, clients, .. } = &mut *state;
        for sub in subs.values_mut() {
            if sub.closed {
                continue;
            }
            let mut snapshot_parts: Vec<String> = Vec::new();
            let mut outgoing: Vec<(DeliveryKind, String)> = Vec::new();
            let mut unchanged = false;
            match &mut sub.detail {
                Detail::Query(query) => match query.collect() {
                    Some(Ok(rows)) => match query.snapshot_if_changed(&rows) {
                        Some(data) => outgoing.push((DeliveryKind::Snapshot, data)),
                        None => unchanged = true,
                    },
                    Some(Err(error)) => tracing::warn!(subscription = sub.id, %error, "host/query run failed"),
                    None => {}
                },
                Detail::NodeChanges { property_scope, snapshot, ready, pending } => {
                    if let SnapshotState::Pending(reply) = snapshot {
                        match reply.try_recv() {
                            Ok(Ok(nodes)) => {
                                snapshot_parts = snapshot_part_texts(nodes, property_scope.as_deref());
                                *snapshot = SnapshotState::Idle;
                                *ready = true;
                            }
                            Ok(Err(error)) => {
                                tracing::warn!(subscription = sub.id, %error, "node snapshot failed; retrying");
                                *snapshot = SnapshotState::Needed;
                            }
                            Err(TryRecvError::Disconnected) => *snapshot = SnapshotState::Needed,
                            Err(TryRecvError::Empty) => {}
                        }
                    }
                    if *ready && !pending.is_empty() {
                        let changes = std::mem::take(pending);
                        for chunk in changes.chunks(NODE_DELTA_CHANGES) {
                            outgoing.push((DeliveryKind::Delta, json!({ "changes": chunk }).to_string()));
                        }
                    }
                }
                _ => {}
            }
            let owner = sub.counter_owner();
            if unchanged {
                bump(instances, owner.as_ref(), |counters| counters.query_unchanged += 1);
            }
            let queued_any = !snapshot_parts.is_empty() || !outgoing.is_empty();
            for part in snapshot_parts {
                sub.queue_event(DeliveryKind::Snapshot, Arc::from(part), now);
            }
            let mut displaced = 0;
            for (kind, data) in outgoing {
                // A resync mid-batch drops the rest: the fresh snapshot covers it.
                if kind == DeliveryKind::Delta && matches!(sub.detail, Detail::NodeChanges { ready: false, .. }) {
                    break;
                }
                displaced += sub.push(kind, Arc::from(data), now);
            }
            bump(instances, owner.as_ref(), |counters| counters.dropped_queue += displaced);
            if queued_any {
                notify_client(clients, &sub.subscriber);
            }
        }
    }

    /// Start due query runs and requested snapshots; the services are called outside the lock.
    fn launch(&self) {
        let now = self.now();
        let mut runs: Vec<QueryLaunch> = Vec::new();
        let mut snapshots: Vec<(String, SnapshotReply)> = Vec::new();
        {
            let mut state = self.lock();
            let BrokerState { subs, clients, .. } = &mut *state;
            for sub in subs.values_mut() {
                if sub.closed {
                    continue;
                }
                let paused = match &sub.subscriber {
                    Subscriber::Client(client) => clients.get(client).is_some_and(|client| client.paused),
                    Subscriber::Instance(_) => false,
                };
                match &mut sub.detail {
                    Detail::Query(query) if self.sources.query.is_some() && query.due(now, paused) => {
                        let previous = query.begin_launch(now);
                        runs.push((sub.id, query.request.clone(), query.prepared.clone(), previous));
                    }
                    Detail::NodeChanges { snapshot, .. } if matches!(snapshot, SnapshotState::Needed) && self.sources.scene.is_some() => {
                        let (reply, receiver) = crossbeam::channel::bounded(1);
                        *snapshot = SnapshotState::Pending(receiver);
                        snapshots.push((sub.petal_id.clone(), reply));
                    }
                    _ => {}
                }
            }
        }
        if let Some(scene) = &self.sources.scene {
            for (petal_id, reply) in snapshots {
                scene.snapshot(&petal_id, reply);
            }
        }
        let Some(source) = self.sources.query.clone() else { return };
        if runs.is_empty() {
            return;
        }
        let mut started = Vec::with_capacity(runs.len());
        for (id, request, prepared, previous) in runs {
            if source.try_acquire_rate(&request.identity_did) {
                let (reply, receiver) = crossbeam::channel::bounded(1);
                source.run(&request, &prepared, reply);
                started.push((id, Some(receiver), previous));
            } else {
                started.push((id, None, previous));
            }
        }
        let mut state = self.lock();
        let BrokerState { subs, instances, .. } = &mut *state;
        for (id, receiver, previous) in started {
            let Some(sub) = subs.get_mut(&id) else { continue };
            let owner = sub.counter_owner();
            let Detail::Query(query) = &mut sub.detail else { continue };
            match receiver {
                Some(receiver) => {
                    query.launched(receiver);
                    bump(instances, owner.as_ref(), |counters| counters.query_runs += 1);
                }
                None => {
                    query.launch_deferred(previous);
                    bump(instances, owner.as_ref(), |counters| counters.query_deferred += 1);
                }
            }
        }
    }

    /// Calls past their 5 s deadline answer `unknown-outcome` and are never retried; idle buckets are swept.
    fn expire_calls(&self) {
        let now = self.now();
        let mut effects = Effects::default();
        {
            let mut state = self.lock();
            let BrokerState {
                calls,
                instances,
                remote_gate,
                caller_buckets,
                command_buckets,
                portal_stream_buckets,
                write_buckets,
                identity_write_buckets,
                last_prune_ms,
                ..
            } = &mut *state;
            let expired: Vec<u64> = calls.iter().filter(|(_, call)| call.deadline_ms <= now).map(|(id, _)| *id).collect();
            for id in expired {
                if let Some(call) = calls.remove(&id) {
                    if let Some(target) = instances.get_mut(&call.target) {
                        target.commands.retain(|queued| *queued != id);
                    }
                    let outcome = CommandOutcome::Host(HostError::UnknownOutcome("no reply within 5 s".into()));
                    resolve_call(instances, id, call, outcome, now, &mut effects);
                }
            }
            if now.saturating_sub(*last_prune_ms) >= PRUNE_EVERY_MS {
                *last_prune_ms = now;
                remote_gate.retain(|_, bucket| !bucket.is_idle(now, BUCKET_IDLE_MS));
                caller_buckets.retain(|_, bucket| !bucket.is_idle(now, BUCKET_IDLE_MS));
                command_buckets.retain(|_, bucket| !bucket.is_idle(now, BUCKET_IDLE_MS));
                portal_stream_buckets.retain(|_, bucket| !bucket.is_idle(now, BUCKET_IDLE_MS));
                write_buckets.retain(|_, bucket| !bucket.is_idle(now, BUCKET_IDLE_MS));
                identity_write_buckets.retain(|_, bucket| !bucket.is_idle(now, BUCKET_IDLE_MS));
            }
        }
        self.finish(effects);
    }
}

// ---- free helpers ---------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn audit_entry(
    action: AuditAction,
    actor: &str,
    caller_kind: Option<CallerKind>,
    installation_id: Option<String>,
    target: impl Into<String>,
    args_digest: Option<String>,
    outcome: &str,
    at_ms: u64,
) -> AuditEntry {
    AuditEntry {
        action,
        actor_did: actor.to_string(),
        caller_kind,
        installation_id,
        target: target.into(),
        args_digest,
        outcome: outcome.to_string(),
        at_ms,
    }
}

fn bump(instances: &mut HashMap<InstanceKey, InstanceState>, owner: Option<&InstanceKey>, change: impl FnOnce(&mut BusCounters)) {
    if let Some(instance) = owner.and_then(|key| instances.get_mut(key)) {
        change(&mut instance.counters);
    }
}

fn notify_client(clients: &HashMap<u64, ClientState>, subscriber: &Subscriber) {
    if let Subscriber::Client(id) = subscriber {
        if let Some(client) = clients.get(id) {
            client.notify.notify_one();
        }
    }
}

fn exposed_stream<'a>(manifest: &'a ExtensionManifest, id: &str) -> Option<&'a ExposedStream> {
    manifest.bus.as_ref()?.expose.as_ref()?.streams.as_ref()?.iter().find(|stream| stream.id == id)
}

fn exposed_command<'a>(manifest: &'a ExtensionManifest, id: &str) -> Option<&'a ExposedCommand> {
    manifest.bus.as_ref()?.expose.as_ref()?.commands.as_ref()?.iter().find(|command| command.id == id)
}

fn consumed<'a>(consume: &'a [ConsumeEntry], address: &str) -> Result<&'a ConsumeEntry, HostError> {
    consume
        .iter()
        .find(|entry| entry.source == address)
        .ok_or_else(|| HostError::CapabilityDenied(format!("{address} is not declared under bus.consume")))
}

fn parse_params(params: &str) -> Result<Map<String, Value>, String> {
    if params.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(params) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("params must be a JSON object".into()),
        Err(error) => Err(format!("params are not JSON: {error}")),
    }
}

fn query_vars(params: &Map<String, Value>) -> Result<Map<String, Value>, HostError> {
    match params.get("vars") {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(vars)) => Ok(vars.clone()),
        Some(_) => Err(HostError::InvalidInput("vars must be a JSON object".into())),
    }
}

fn string_list(value: Option<&Value>, name: &str) -> Result<Option<Vec<String>>, HostError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(|| HostError::InvalidInput(format!("{name} must be strings"))))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(HostError::InvalidInput(format!("{name} must be a list"))),
    }
}

/// `host/readings` filters, kept within the consumed metrics (absent `metrics` on the consume entry = any metric).
fn readings_plan(granted: Option<&[String]>, params: &Map<String, Value>) -> Result<PlanKind, HostError> {
    let requested = string_list(params.get("metrics"), "metrics")?;
    if let (Some(granted), Some(requested)) = (granted, &requested) {
        if let Some(metric) = requested.iter().find(|metric| !metric_allowed(granted, metric)) {
            return Err(HostError::CapabilityDenied(format!("metric {metric} is not consumed")));
        }
    }
    let anchors = string_list(params.get("anchors"), "anchors")?;
    if anchors.as_ref().is_some_and(|anchors| anchors.len() > MAX_ANCHOR_FILTER) {
        return Err(HostError::InvalidInput(format!("at most {MAX_ANCHOR_FILTER} anchors")));
    }
    Ok(PlanKind::Readings {
        metrics: requested.or_else(|| granted.map(<[String]>::to_vec)),
        anchors: anchors.map(|anchors| anchors.into_iter().collect()),
    })
}

/// Map a service refusal to what an external caller may see.
fn refusal_for(error: HostError, remote: bool) -> BusRefusal {
    if remote {
        return BusRefusal::NotFound;
    }
    match error {
        HostError::CapabilityDenied(detail) | HostError::PolicyDenied(detail) => BusRefusal::CapabilityDenied(detail),
        HostError::InvalidInput(detail) => BusRefusal::InvalidInput(detail),
        HostError::RateLimited(detail) | HostError::BudgetExceeded(detail) => BusRefusal::RateLimited(detail),
        other => BusRefusal::NotAvailable(other.to_string()),
    }
}

/// Snapshot parts of at most 500 nodes, each node's `properties` cut to the property scope; at most REST's row cap
/// of nodes in all (the source is not trusted to cap), the last part then carrying `"truncated": true`.
fn snapshot_part_texts(nodes: Vec<Value>, scope: Option<&[String]>) -> Vec<String> {
    let truncated = nodes.len() > REST_ROW_CAP;
    let nodes: Vec<Value> = nodes.into_iter().take(REST_ROW_CAP).map(|node| project_properties(node, scope)).collect();
    if nodes.is_empty() {
        return vec![json!({ "part": 0, "last": true, "nodes": [] }).to_string()];
    }
    let parts = nodes.len().div_ceil(NODE_SNAPSHOT_PART);
    nodes
        .chunks(NODE_SNAPSHOT_PART)
        .enumerate()
        .map(|(index, chunk)| {
            let last = index + 1 == parts;
            let mut part = json!({ "part": index, "last": last, "nodes": chunk });
            if last && truncated {
                part["truncated"] = Value::Bool(true);
            }
            part.to_string()
        })
        .collect()
}

fn project_properties(mut node: Value, scope: Option<&[String]>) -> Value {
    if let Some(object) = node.as_object_mut() {
        let kept: Map<String, Value> = match (object.remove("properties"), scope) {
            (Some(Value::Object(properties)), Some(scope)) => {
                properties.into_iter().filter(|(key, _)| scope_allows(scope, key)).collect()
            }
            _ => Map::new(),
        };
        object.insert("properties".into(), Value::Object(kept));
    }
    node
}

/// Queue one published event on every open subscription to `(producer, stream_id)` of the same producer kind;
/// returns the events displaced on external clients' queues (counted on an instance producer).
#[allow(clippy::too_many_arguments)]
fn fan_out_stream(
    subs: &mut BTreeMap<u64, Subscription>,
    instances: &mut HashMap<InstanceKey, InstanceState>,
    clients: &HashMap<u64, ClientState>,
    producer: &InstanceKey,
    portal: bool,
    stream_id: &str,
    data: Arc<str>,
    now: u64,
) -> u64 {
    let mut client_drops = 0u64;
    for sub in subs.values_mut() {
        let matches = matches!(
            &sub.detail,
            Detail::Stream { producer: from, stream_id: id, from_portal, .. } if from == producer && id == stream_id && *from_portal == portal
        );
        if !matches || sub.closed {
            continue;
        }
        let displaced = sub.push(DeliveryKind::Delta, data.clone(), now);
        match &sub.subscriber {
            Subscriber::Instance(consumer) => {
                if let Some(consumer) = instances.get_mut(consumer) {
                    consumer.counters.dropped_queue += displaced;
                }
            }
            Subscriber::Client(client) => {
                client_drops += displaced;
                if let Some(client) = clients.get(client) {
                    client.notify.notify_one();
                }
            }
        }
    }
    client_drops
}

/// A live portal client's caller and grants (cloned); a foreign DID's binding sees only the uniform not-found.
fn portal_client(state: &BrokerState, client: u64) -> Result<(Caller, PortalGrants), BusRefusal> {
    let client_state = state.clients.get(&client).ok_or(BusRefusal::NotFound)?;
    let caller = &client_state.caller;
    let hide = |local: BusRefusal| if caller.local { local } else { BusRefusal::NotFound };
    let ClientGrants::Portal(grants) = &client_state.grants else {
        return Err(hide(BusRefusal::CapabilityDenied("only a mounted portal client may do this".into())));
    };
    if let Some(reason) = client_state.stopped {
        return Err(hide(BusRefusal::NotAvailable(format!("the portal client stopped: {}", reason.as_str()))));
    }
    Ok((caller.clone(), (**grants).clone()))
}

/// The producer of `<package>/<stream_id>` at a petal: an active instance first, else a live local portal client.
fn lookup_producer(state: &BrokerState, producer: &InstanceKey, stream_id: &str) -> ProducerLookup {
    if let Some(instance) = state.instances.get(producer) {
        return match exposed_stream(&instance.spec.manifest, stream_id) {
            Some(stream) => ProducerLookup::Found(ProducerStream {
                external: stream.external == Some(true),
                role_floor: role_level(stream.role_floor.unwrap_or(RoleFloor::Viewer)),
                consumers: stream.consumers.clone(),
                verse_id: instance.spec.verse_id.clone(),
                from_portal: false,
            }),
            None => ProducerLookup::Undeclared,
        };
    }
    match portal_stream(&state.clients, producer, stream_id) {
        // Portal streams are local-only (no `external`) and readable from Viewer.
        Some((grants, stream)) => ProducerLookup::Found(ProducerStream {
            external: false,
            role_floor: RoleLevel::Viewer,
            consumers: stream.consumers.clone(),
            verse_id: grants.verse_id.clone(),
            from_portal: true,
        }),
        None if !portal_producers(&state.clients, producer).is_empty() => ProducerLookup::Undeclared,
        None => ProducerLookup::Missing,
    }
}

/// Live, local portal clients mounted at `producer`'s petal with its package, lowest client id first.
fn portal_producers<'a>(clients: &'a HashMap<u64, ClientState>, producer: &InstanceKey) -> Vec<(u64, &'a PortalGrants)> {
    let mut found: Vec<(u64, &'a PortalGrants)> = clients
        .iter()
        .filter(|(_, client)| client.stopped.is_none() && client.caller.local)
        .filter_map(|(id, client)| match &client.grants {
            ClientGrants::Portal(grants) if grants.petal_id == producer.petal_id && grants.package_id == producer.package_id => {
                Some((*id, &**grants))
            }
            _ => None,
        })
        .collect();
    found.sort_by_key(|(id, _)| *id);
    found
}

/// The declaring portal of a stream at `producer`, if a live one still declares it.
fn portal_stream<'a>(clients: &'a HashMap<u64, ClientState>, producer: &InstanceKey, stream_id: &str) -> Option<(&'a PortalGrants, &'a PortalStream)> {
    portal_producers(clients, producer)
        .into_iter()
        .find_map(|(_, grants)| grants.streams.iter().find(|stream| stream.id == stream_id).map(|stream| (grants, stream)))
}

/// End portal-produced stream subscriptions whose producer no live portal client declares any more.
fn end_orphaned_portal_streams(subs: &mut BTreeMap<u64, Subscription>, clients: &HashMap<u64, ClientState>, now: u64) {
    for sub in subs.values_mut() {
        let orphaned = match &sub.detail {
            Detail::Stream { producer, stream_id, from_portal: true, .. } => portal_stream(clients, producer, stream_id).is_none(),
            _ => false,
        };
        if orphaned && !sub.closed {
            sub.end(EndReason::ProducerInactive, now);
            notify_client(clients, &sub.subscriber);
        }
    }
}

/// The `ended` reason a web bundle's admission change gives its portal clients; `None` while it stays admitted.
fn portal_end_reason(reason: AdmissionReason) -> Option<EndReason> {
    match reason {
        AdmissionReason::Consented | AdmissionReason::Restored => None,
        AdmissionReason::RevocationList => Some(EndReason::Revoked),
        AdmissionReason::ConsentRevoked => Some(EndReason::ConsentRevoked),
        AdmissionReason::RoleLost => Some(EndReason::RoleLost),
        AdmissionReason::PolicyRemoved | AdmissionReason::Ceiling => Some(EndReason::PolicyDenied),
        AdmissionReason::LocalDisable | AdmissionReason::DisableAll | AdmissionReason::Uninstalled => Some(EndReason::InstanceStopped),
    }
}

/// Stop every portal client of the bundle (by package id, or by content hash under any id) in the event's verse:
/// no new subscriptions, publishes, calls or writes; its subscriptions and its streams' subscribers end now, while
/// writes already sent still settle and their rows stay (OD-E1).
fn stop_portal_clients(
    subs: &mut BTreeMap<u64, Subscription>,
    clients: &mut HashMap<u64, ClientState>,
    event: &AdmissionEvent,
    reason: EndReason,
    now: u64,
    effects: &mut Effects,
) {
    let stopping: Vec<u64> = clients
        .iter()
        .filter(|(_, client)| client.stopped.is_none())
        .filter(|(_, client)| match &client.grants {
            ClientGrants::Portal(grants) => {
                let same_bundle = grants.package_id == event.package_id
                    || (!event.content_hash.is_empty() && grants.content_hash == event.content_hash);
                same_bundle && event.verse_id.as_ref().is_none_or(|verse| *verse == grants.verse_id)
            }
            _ => false,
        })
        .map(|(id, _)| *id)
        .collect();
    if stopping.is_empty() {
        return;
    }
    for id in &stopping {
        if let Some(client) = clients.get_mut(id) {
            client.stopped = Some(reason);
        }
    }
    for sub in subs.values_mut() {
        let owned = matches!(&sub.subscriber, Subscriber::Client(client) if stopping.contains(client));
        if owned && !sub.closed {
            sub.end(reason, now);
            effects.audits.push(audit_entry(AuditAction::Unsubscribe, &sub.identity_did, Some(sub.caller_kind), None, sub.address.to_string(), None, reason.as_str(), now));
            notify_client(clients, &sub.subscriber);
        }
    }
    end_orphaned_portal_streams(subs, clients, now);
}

/// The write path's reply as the portal sees it; only a commit receipt counts as written.
fn write_outcome(result: PluginResult) -> WriteOutcome {
    match result {
        Ok(PluginReply::Committed(receipt)) => WriteOutcome::Committed(receipt),
        Ok(_) => WriteOutcome::Refused(PluginRefusal::UnknownOutcome("the write path answered without a commit receipt".into())),
        Err(refusal) => WriteOutcome::Refused(refusal),
    }
}

fn pending_call(target: InstanceKey, installation_id: String, command_id: &str, args: &Value, caller: Caller, now: u64, reply: CallReply) -> PendingCall {
    PendingCall {
        target,
        installation_id,
        command: command_id.to_string(),
        args: args.to_string(),
        args_digest: json_digest(args),
        caller,
        deadline_ms: now + COMMAND_REPLY_TIMEOUT_MS,
        reply,
    }
}

fn enqueue_call(state: &mut BrokerState, id: u64, call: PendingCall) {
    if let Some(target) = state.instances.get_mut(&call.target) {
        target.commands.push_back(id);
    }
    state.calls.insert(id, call);
}

/// Count, audit (denials only) a refused call.
#[allow(clippy::too_many_arguments)]
fn note_refusal(
    state: &mut BrokerState,
    caller: &Caller,
    petal_id: &str,
    package_id: &str,
    command_id: &str,
    refusal: &BusRefusal,
    now: u64,
    effects: &mut Effects,
) {
    let target = InstanceKey::new(petal_id, package_id);
    let installation = state.instances.get_mut(&target).map(|instance| {
        *instance.counters.commands_refused.entry(refusal.code().to_string()).or_default() += 1;
        instance.spec.installation_id.clone()
    });
    if matches!(refusal, BusRefusal::NotFound | BusRefusal::CapabilityDenied(_)) {
        let installation = if caller.local { installation } else { None };
        effects.audits.push(audit_entry(AuditAction::Denial, &caller.did, Some(caller.kind), installation, format!("{package_id}/{command_id}"), None, refusal.code(), now));
    }
}

/// Answer a call once: count it on the target, audit it, and deliver the outcome.
fn resolve_call(instances: &mut HashMap<InstanceKey, InstanceState>, id: u64, call: PendingCall, outcome: CommandOutcome, now: u64, effects: &mut Effects) {
    let code = match &outcome {
        CommandOutcome::Ok(_) => "ok",
        CommandOutcome::Guest(error) => error.code(),
        CommandOutcome::Host(error) => error.code(),
    };
    if let Some(target) = instances.get_mut(&call.target) {
        if matches!(outcome, CommandOutcome::Ok(_)) {
            target.counters.commands_ok += 1;
        } else {
            *target.counters.commands_refused.entry(code.to_string()).or_default() += 1;
        }
    }
    effects.audits.push(audit_entry(
        AuditAction::CommandCall,
        &call.caller.did,
        Some(call.caller.kind),
        Some(call.installation_id.clone()),
        format!("{}/{}", call.target.package_id, call.command),
        Some(call.args_digest.clone()),
        code,
        now,
    ));
    match call.reply {
        CallReply::External(reply) => {
            let _ = reply.send(outcome);
        }
        CallReply::Guest(caller) => {
            if let Some(caller) = instances.get_mut(&caller) {
                let result = match outcome {
                    CommandOutcome::Ok(json) => Ok(json),
                    CommandOutcome::Guest(error) => Err(error.into_host_error()),
                    CommandOutcome::Host(error) => Err(error),
                };
                caller.call_results.push_back((id, result));
            }
        }
    }
}

/// Up to 64 events / 256 KiB for one instance, round-robin across its subscriptions, `seq` order within each.
fn take_guest_batch(subs: &mut BTreeMap<u64, Subscription>, key: &InstanceKey, cursor: &mut u64) -> Vec<BusEvent> {
    let owned: Vec<u64> = subs.values().filter(|sub| sub.owned_by_instance(key) && !sub.queue.is_empty()).map(|sub| sub.id).collect();
    let mut batch = Vec::new();
    if owned.is_empty() {
        return batch;
    }
    let start = owned.iter().position(|id| *id > *cursor).unwrap_or(0);
    let mut bytes = 0usize;
    'subscriptions: for id in owned[start..].iter().chain(owned[..start].iter()) {
        let Some(sub) = subs.get_mut(id) else { continue };
        while let Some(front) = sub.queue.front() {
            let size = front.data.len();
            if batch.len() >= GUEST_BATCH_EVENTS || (!batch.is_empty() && bytes + size > GUEST_BATCH_BYTES) {
                break 'subscriptions;
            }
            let Some(event) = sub.pop() else { break };
            bytes += size;
            batch.push(event);
            *cursor = *id;
            // A query snapshot over 256 KiB travels alone.
            if bytes >= GUEST_BATCH_BYTES {
                break 'subscriptions;
            }
        }
    }
    subs.retain(|_, sub| !(sub.owned_by_instance(key) && sub.finished()));
    batch
}
