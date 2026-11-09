//! `fe.portal.v1` bridge: sender gate, strict decode, capability check, broker routing, paced delivery (ipc-v1.md); see `portal_bundle/AGENTS.md` §ipc.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use fe_format::portal_bundle::PortalSection;
use http::header::{CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use http::{HeaderValue, Method, Request, Response, StatusCode};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::limits::{self, TokenBucket};
use super::mount::{Mount, MountMode, PortalMounts};
use super::origin;

/// Wire name of this API.
pub const API_NAME: &str = "fe.portal.v1";
/// Host-served page API path (protocol.md §3.4).
pub const PAGE_API_PATH: &str = "/__fe/portal-v1.js";
const PAGE_API_TEMPLATE: &str = include_str!("portal-v1.js");
const TOKEN_PLACEHOLDER: &str = "__FE_DOCUMENT_TOKEN__";
const PAGE_API_CONTENT_TYPE: &str = "text/javascript; charset=utf-8";
/// Every envelope starts `{"v":1,"n":"<token>","id":` (ipc-v1.md §2 check 2).
const PREFIX_HEAD: &str = r#"{"v":1,"n":""#;
const PREFIX_TAIL: &str = r#"","id":"#;
const TOKEN_CONTEXT: &str = "fractalengine portal document token v1";
/// Longest page-supplied text echoed into an audit target.
const MAX_AUDIT_TARGET_BYTES: usize = 128;
const MAX_NODE_ID_BYTES: usize = 64;

// ---------------------------------------------------------------------------
// Errors and counters (ipc-v1.md §7)
// ---------------------------------------------------------------------------

/// Typed IPC error; its message text is fixed per code and never echoes page input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpcError {
    Invalid,
    RateLimited,
    TooManySubscriptions,
    CapabilityDenied,
    PolicyDenied,
    RoleDenied,
    NotFound,
    Revoked,
    Unavailable,
    Backend,
    UnknownOutcome,
}

impl IpcError {
    /// Snake_case wire code (§7.1).
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::RateLimited => "rate_limited",
            Self::TooManySubscriptions => "too_many_subscriptions",
            Self::CapabilityDenied => "capability_denied",
            Self::PolicyDenied => "policy_denied",
            Self::RoleDenied => "role_denied",
            Self::NotFound => "not_found",
            Self::Revoked => "revoked",
            Self::Unavailable => "unavailable",
            Self::Backend => "backend",
            Self::UnknownOutcome => "unknown_outcome",
        }
    }

    /// Fixed reply text.
    pub fn message(self) -> &'static str {
        match self {
            Self::Invalid => "The request is malformed",
            Self::RateLimited => "Too many requests",
            Self::TooManySubscriptions => "Too many live subscriptions",
            Self::CapabilityDenied => "This bundle may not do that",
            Self::PolicyDenied => "The verse policy refuses this",
            Self::RoleDenied => "This peer's role does not allow it",
            Self::NotFound => "Not found",
            Self::Revoked => "The bundle was revoked",
            Self::Unavailable => "The host service is not running",
            Self::Backend => "The host failed",
            Self::UnknownOutcome => "The outcome is unknown; do not retry",
        }
    }

    fn count(self, counters: &mut IpcCounters) {
        match self {
            Self::Invalid => counters.invalid += 1,
            Self::RateLimited => counters.rate_limited += 1,
            Self::TooManySubscriptions => counters.too_many_subscriptions += 1,
            Self::CapabilityDenied | Self::PolicyDenied | Self::Revoked => counters.refused_capability += 1,
            Self::RoleDenied | Self::NotFound => counters.refused_role += 1,
            Self::Unavailable | Self::Backend | Self::UnknownOutcome => counters.failed += 1,
        }
    }
}

/// Host-side outcome counters (§7.3); the first six are the spec's outcome classes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct IpcCounters {
    pub accepted: u64,
    pub refused_capability: u64,
    pub refused_role: u64,
    pub rate_limited: u64,
    pub dropped_origin: u64,
    pub invalid: u64,
    pub failed: u64,
    pub too_many_subscriptions: u64,
    pub live_subscriptions: u64,
    pub deliveries: u64,
}

/// What the fixed `ipc-stats` QA operation reports: the last counted mount and the process total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct IpcStats {
    pub mount_id: Option<u64>,
    pub mount: IpcCounters,
    pub total: IpcCounters,
}

impl IpcStats {
    /// The `ipc-stats` shape: process-total counters flat (drops from foreign origins count only there), plus this mount's.
    pub fn to_json(&self) -> Value {
        let mut value = serde_json::to_value(self.total).unwrap_or_default();
        if let Value::Object(map) = &mut value {
            map.insert("mount_id".into(), json!(self.mount_id));
            map.insert("mount".into(), serde_json::to_value(self.mount).unwrap_or_default());
        }
        value
    }
}

// ---------------------------------------------------------------------------
// Broker seam (A-1; bus.md §8 "Portal IPC mapping")
// ---------------------------------------------------------------------------

/// Delivery kinds on the page wire (the bus spells `resync-required` with a hyphen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    Snapshot,
    Delta,
    ResyncRequired,
    Ended,
}

impl DeliveryKind {
    /// Page wire text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Delta => "delta",
            Self::ResyncRequired => "resync_required",
            Self::Ended => "ended",
        }
    }
}

/// One broker event for a portal client (bus.md §5 envelope, `data` parsed).
#[derive(Debug, Clone, PartialEq)]
pub struct BrokerEvent {
    pub subscription: u64,
    pub kind: DeliveryKind,
    pub dropped_before: u64,
    pub data: Value,
}

/// A reply the broker or the write path completes later; the bridge polls it once per frame.
pub type PendingReply = mpsc::Receiver<Result<Value, IpcError>>;

/// The mount facts the bridge hands the broker when it binds an Admitted mount.
#[derive(Debug, Clone, Copy)]
pub struct BindRequest<'a> {
    pub mount_id: u64,
    pub repository_node_id: &'a str,
    pub package_id: &'a str,
    pub content_hash: &'a str,
    pub publisher_did: &'a str,
    pub portal: &'a PortalSection,
}

/// The broker's half of a binding (ipc-v1.md §6.5); host-minted, never built from page data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerClient {
    pub client: u64,
    /// The local identity that admitted the bundle; acts and invokes are judged as it at use.
    pub installing_did: String,
    /// The repository node's petal: the scope of every subscription, act and intent.
    pub petal_id: String,
    /// Capability strings (§4.3) governance permits now, intents included; the bridge intersects them with the manifest.
    pub granted: BTreeSet<String>,
}

/// The integration-bus client the binary injects; every call is non-blocking and made on the main thread.
///
/// Refusals map from the sibling's `BusRefusal`: not-found → `NotFound`, not-available → `Unavailable`,
/// capability-denied → `CapabilityDenied`, rate-limited → `RateLimited`, invalid-input → `Invalid`,
/// limit-reached → `TooManySubscriptions`.
pub trait PortalBroker: Send + Sync + 'static {
    /// Open a caller-kind `portal` client at the repository node's petal.
    fn open(&self, request: &BindRequest<'_>) -> Result<BrokerClient, IpcError>;
    /// Close a client; its subscriptions end.
    fn close(&self, client: u64);
    /// Pause or resume the client's query runs (hidden portal).
    fn set_paused(&self, client: u64, paused: bool);
    /// Subscribe to `host/node-changes`, `host/readings`, `host/query` or `<package_id>/<stream>`.
    fn subscribe(&self, client: u64, address: &str, params: &Value) -> Result<u64, IpcError>;
    /// End one subscription.
    fn unsubscribe(&self, client: u64, subscription: u64);
    /// Up to `max` queued events, in subscription then `seq` order.
    fn take_events(&self, client: u64, max: usize) -> Vec<BrokerEvent>;
    /// Call `<package_id>/<command_id>` as caller kind `portal`; below the role floor is `NotFound`; the broker audits it.
    fn invoke(&self, client: u64, package_id: &str, command_id: &str, args: &Value) -> Result<PendingReply, IpcError>;
    /// Publish on `<bundle package_id>/<stream_id>`.
    fn emit(&self, client: u64, stream_id: &str, data: &Value) -> Result<(), IpcError>;
    /// One-op commit on the extension write path with host-stamped provenance (A-13): `NotFound` outside the
    /// client's petal, `RoleDenied` below Editor at write time; audited there with the key and a value digest.
    fn set_property(&self, client: u64, node_id: &str, key: &str, value: &Value) -> Result<PendingReply, IpcError>;
    /// Audit one request the bridge refused at check 5 (§9).
    fn audit_refusal(&self, client: u64, operation: &str, target: &str, code: &str);
}

/// A local UI intent the page asked for (§6.4); fe-ui applies it only inside `petal_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UiIntent {
    FocusNode,
    SelectNode,
}

impl UiIntent {
    /// Manifest and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FocusNode => "focus_node",
            Self::SelectNode => "select_node",
        }
    }
}

/// A page's focus/select request for fe-ui; never a write and never audited.
#[derive(Debug, Clone, PartialEq, Eq, bevy::prelude::Message)]
pub struct PortalIntent {
    pub intent: UiIntent,
    pub node_id: String,
    pub petal_id: String,
    pub package_id: String,
}

/// What the backend gives the bridge: the shared state and a host→page post (OD-2).
pub trait BridgeWebview {
    /// The backend's bridge; `None` without an embedded browser.
    fn portal_bridge(&self) -> Option<&PortalBridge> {
        None
    }
    /// Post one envelope to the top-level document (Windows: `PostWebMessageAsJson`).
    fn post_page_message(&self, _json: &str) -> anyhow::Result<()> {
        anyhow::bail!("this backend has no portal bridge")
    }
}

// ---------------------------------------------------------------------------
// Page messages (ipc-v1.md §3.5)
// ---------------------------------------------------------------------------

/// The body after the verified prefix, decoded strictly: unknown keys, repeated keys and missing fields are `invalid`.
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
enum PageMessage {
    Hello {},
    Subscribe { stream: String, params: Map<String, Value> },
    Unsubscribe { sub: u32 },
    Act { op: ActOp, node_id: String, key: String, value: Value },
    Invoke { package: String, command: String, args: Value },
    Emit { stream: String, data: Value },
    Intent { intent: UiIntent, node_id: String },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ActOp {
    SetProperty,
}

/// What the native handler did with one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingress {
    /// Checks 1-2 failed (or the id was unreadable): no reply, counted.
    Dropped,
    /// Check 3 failed: a typed error reply is queued.
    Replied,
    /// Queued for decode and routing on the next frame.
    Accepted,
}

// ---------------------------------------------------------------------------
// Serving the page API
// ---------------------------------------------------------------------------

/// The `fe-portal` handler plus `/__fe/portal-v1.js`, which only an Admitted mount gets, carrying this generation's token.
pub fn serve(mounts: &PortalMounts, bridge: &PortalBridge, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let mut response = super::protocol::serve(mounts, request);
    // A reserved-path 404 already passed the method, worker, origin, mount and path checks.
    if response.status() != StatusCode::NOT_FOUND {
        return response;
    }
    let uri = request.uri().to_string();
    let Some((label, tail)) = origin::split(&uri) else {
        return response;
    };
    let path = tail.split(|c: char| c == '?' || c == '#').next().unwrap_or_default();
    if path != PAGE_API_PATH {
        return response;
    }
    let Some(mount) = mounts.lookup(&label) else {
        return response;
    };
    let Some(token) = bridge.document_token(&mount, mounts.generation()) else {
        return response;
    };
    let body = PAGE_API_TEMPLATE.replacen(TOKEN_PLACEHOLDER, &token, 1).into_bytes();
    let length = body.len() as u64;
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(PAGE_API_CONTENT_TYPE));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(CONTENT_SECURITY_POLICY, mount.csp().clone());
    *response.body_mut() = if request.method() == Method::HEAD { Vec::new() } else { body };
    response
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// Bridge state shared by the native message handler, the frame pump and the protocol workers (token minting).
#[derive(Clone)]
pub struct PortalBridge {
    shared: Arc<Mutex<BridgeState>>,
}

impl Default for PortalBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl PortalBridge {
    /// An unbound, hidden bridge.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(BridgeState::new())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BridgeState> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The portal surface was shown or hidden (check 2; query runs pause while hidden).
    pub fn set_visible(&self, visible: bool) {
        self.lock().visible = visible;
    }

    /// This generation's document token for an Admitted `mount`, minted on first use (ipc-v1.md §1.4).
    pub fn document_token(&self, mount: &Mount, generation: u64) -> Option<String> {
        if mount.mode != MountMode::Admitted || mount.is_faulted() {
            return None;
        }
        let mut state = self.lock();
        let current = state
            .document
            .as_ref()
            .filter(|document| document.mount_id == mount.mount_id && document.generation == generation)
            .map(|document| document.token.clone());
        if current.is_some() {
            return current;
        }
        let token = state.tokens.next(mount.mount_id, generation);
        state.document = Some(DocumentToken {
            mount_id: mount.mount_id,
            generation,
            token: token.clone(),
        });
        Some(token)
    }

    /// Checks 1-3 on one page post; nothing is decoded here (§2).
    pub fn ingress(&self, mounts: &PortalMounts, source: &str, body: &str, now: Instant) -> Ingress {
        self.lock().ingress(mounts, source, body, now)
    }

    /// Per frame: follow the mount table, route accepted messages, collect replies and events, pace deltas.
    pub fn pump(&self, mounts: &PortalMounts, broker: Option<&Arc<dyn PortalBroker>>, now: Instant) -> Vec<PortalIntent> {
        let mut state = self.lock();
        state.sync(mounts, broker, now);
        let mut intents = Vec::new();
        state.process_inbox(now, &mut intents);
        state.poll_calls(now);
        state.drain_broker();
        state.flush_deltas(now);
        intents
    }

    /// Envelopes that still pass checks 1-2 at send time; the rest are discarded (§1.2).
    pub fn take_outbox(&self, mounts: &PortalMounts) -> Vec<String> {
        let mut state = self.lock();
        let live = mounts
            .current()
            .filter(|mount| mount.mode == MountMode::Admitted && !mount.is_faulted())
            .map(|mount| mount.mount_id);
        let generation = mounts.generation();
        let visible = state.visible;
        std::mem::take(&mut state.outbox)
            .into_iter()
            .filter(|out| visible && Some(out.mount_id) == live && out.generation == generation)
            .map(|out| out.json)
            .collect()
    }

    /// Counters for the fixed `ipc-stats` operation.
    pub fn stats(&self) -> IpcStats {
        let state = self.lock();
        let live = state.subs.len() as u64;
        let bound = state.binding.as_ref().map(|binding| binding.mount_id);
        let mut stats = IpcStats {
            mount_id: state.counters_mount,
            mount: state.counters,
            total: state.total,
        };
        stats.total.live_subscriptions = live;
        stats.mount.live_subscriptions = if bound.is_some() && bound == state.counters_mount { live } else { 0 };
        stats
    }

    /// The webview is gone: close the broker client now.
    pub fn shutdown(&self) {
        let mut state = self.lock();
        state.unbind();
        state.retain_queues(None);
        state.visible = false;
    }
}

#[derive(Debug, Clone)]
struct DocumentToken {
    mount_id: u64,
    generation: u64,
    token: String,
}

/// An accepted post waiting for decode; `remainder` is `{` plus the body after the verified id.
struct Inbound {
    mount_id: u64,
    generation: u64,
    id: u32,
    remainder: String,
}

/// One envelope bound to the document it was written for.
struct Outbound {
    mount_id: u64,
    generation: u64,
    json: String,
}

/// Where a reply goes.
struct ReplyTo {
    mount_id: u64,
    generation: u64,
    id: u32,
    token: String,
}

/// Per-mount buckets: they survive reloads, so a page cannot reset its write budget by navigating.
struct Buckets {
    mount_id: u64,
    message: TokenBucket,
    set_property: TokenBucket,
    invoke: TokenBucket,
    intent: TokenBucket,
    audit: TokenBucket,
    emit: BTreeMap<String, TokenBucket>,
}

impl Buckets {
    fn new(mount_id: u64, now: Instant) -> Self {
        Self {
            mount_id,
            message: TokenBucket::new(limits::MESSAGE_RATE, now),
            set_property: TokenBucket::new(limits::SET_PROPERTY_RATE, now),
            invoke: TokenBucket::new(limits::INVOKE_RATE, now),
            intent: TokenBucket::new(limits::INTENT_RATE, now),
            audit: TokenBucket::new(limits::AUDIT_REFUSAL_RATE, now),
            emit: BTreeMap::new(),
        }
    }
}

struct OpenClient {
    broker: Arc<dyn PortalBroker>,
    info: BrokerClient,
}

/// The host-minted binding of the live Admitted mount (ipc-v1.md §6.5).
struct Binding {
    mount_id: u64,
    node_id: String,
    package_id: String,
    content_hash: String,
    version: String,
    publisher_did: String,
    declared: PortalSection,
    /// Declared ∩ granted capability strings; empty without a broker client.
    capabilities: BTreeSet<String>,
    client: Option<OpenClient>,
    /// A broker was present at bind time (refused or not); a later one triggers one rebind.
    broker_attempted: bool,
    paused: bool,
}

enum CallKind {
    Act { node_id: String, key: String },
    Invoke,
}

struct PendingCall {
    id: u32,
    kind: CallKind,
    reply: PendingReply,
    deadline: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Node,
    Readings,
    Query,
    Bus,
}

/// One page subscription for the current document.
struct Subscription {
    broker_sub: u64,
    kind: StreamKind,
    /// `node` subscriptions narrowed to one node; scope is proven by the first snapshot.
    node_filter: Option<String>,
    /// The subscribe request whose reply waits for that proof.
    awaiting: Option<u32>,
    scoped: Vec<Value>,
    seq: u64,
    dropped: u64,
    changes: ChangeSet,
    items: VecDeque<Value>,
    last_delta: Option<Instant>,
}

impl Subscription {
    fn new(broker_sub: u64, kind: StreamKind, node_filter: Option<String>, awaiting: Option<u32>) -> Self {
        Self {
            broker_sub,
            kind,
            node_filter,
            awaiting,
            scoped: Vec::new(),
            seq: 0,
            dropped: 0,
            changes: ChangeSet::default(),
            items: VecDeque::new(),
            last_delta: None,
        }
    }

    fn has_pending(&self) -> bool {
        !self.changes.is_empty() || !self.items.is_empty()
    }

    /// Merge one broker delta into the pending delivery (§5.5 coalescing).
    fn merge(&mut self, data: Value) {
        match self.kind {
            StreamKind::Node => {
                let Value::Object(mut map) = data else { return };
                let Some(Value::Array(changes)) = map.remove("changes") else { return };
                for change in changes {
                    if let Some(filter) = &self.node_filter {
                        if change_node_id(&change) != Some(filter.as_str()) {
                            continue;
                        }
                    }
                    self.dropped += self.changes.push(change);
                }
            }
            StreamKind::Readings => {
                let rows = match data {
                    Value::Object(mut map) => match map.remove("readings") {
                        Some(Value::Array(rows)) => rows,
                        _ => return,
                    },
                    _ => return,
                };
                for row in rows {
                    self.push_item(row, limits::MAX_READINGS_PER_DELIVERY);
                }
            }
            StreamKind::Query | StreamKind::Bus => self.push_item(data, limits::MAX_EVENTS_PER_DELIVERY),
        }
    }

    fn push_item(&mut self, item: Value, cap: usize) {
        self.items.push_back(item);
        while self.items.len() > cap {
            self.items.pop_front();
            self.dropped += 1;
        }
    }

    /// Resync: what was pending is superseded and counted as dropped.
    fn discard_pending(&mut self) {
        self.dropped += (self.changes.len() + self.items.len()) as u64;
        self.changes = ChangeSet::default();
        self.items.clear();
    }

    /// The coalesced delta body, in the bus's own data shape per stream.
    fn take_delta(&mut self) -> Value {
        match self.kind {
            StreamKind::Node => json!({ "changes": self.changes.take() }),
            StreamKind::Readings => json!({ "readings": std::mem::take(&mut self.items) }),
            StreamKind::Query | StreamKind::Bus => json!({ "events": std::mem::take(&mut self.items) }),
        }
    }
}

/// `(op, node_id, key)`: later changes with the same key replace earlier ones (last state wins).
type ChangeKey = (String, String, String);

/// Pending scene changes in arrival order with last-wins collapsing per node (and per key for properties).
#[derive(Default)]
struct ChangeSet {
    next: u64,
    items: BTreeMap<u64, (Option<ChangeKey>, Value)>,
    latest: HashMap<ChangeKey, u64>,
}

impl ChangeSet {
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    /// Add one change; returns how many older changes the cap pushed out.
    fn push(&mut self, change: Value) -> u64 {
        let key = change_key(&change);
        if let Some(previous) = key.as_ref().and_then(|key| self.latest.remove(key)) {
            self.items.remove(&previous);
        }
        let seq = self.next;
        self.next += 1;
        if let Some(key) = &key {
            self.latest.insert(key.clone(), seq);
        }
        self.items.insert(seq, (key, change));
        let mut dropped = 0;
        while self.items.len() > limits::MAX_CHANGES_PER_DELIVERY {
            if let Some((_, (Some(key), _))) = self.items.pop_first() {
                self.latest.remove(&key);
            }
            dropped += 1;
        }
        dropped
    }

    fn take(&mut self) -> Vec<Value> {
        self.latest.clear();
        std::mem::take(&mut self.items).into_values().map(|(_, change)| change).collect()
    }
}

/// The node a `SceneChange` JSON names (`node_id`, or `node.node_id` for `node_added`).
fn change_node_id(change: &Value) -> Option<&str> {
    change
        .get("node_id")
        .or_else(|| change.get("node").and_then(|node| node.get("node_id")))
        .and_then(Value::as_str)
}

fn change_key(change: &Value) -> Option<ChangeKey> {
    let op = change.get("op").and_then(Value::as_str)?;
    let node_id = change_node_id(change)?;
    let key = match op {
        "node_transform" | "node_renamed" => "",
        "property_changed" => change.get("key").and_then(Value::as_str)?,
        _ => return None,
    };
    Some((op.to_owned(), node_id.to_owned(), key.to_owned()))
}

/// What a broker event does once the subscription borrow ends.
enum Effect {
    Nothing,
    Deliver(DeliveryKind, Value),
    DeliverAndEnd(Value),
    ScopeProven { request: u32, nodes: Vec<Value> },
    ScopeMissing { request: u32, broker_sub: u64 },
    EndAwaiting { request: u32, error: IpcError },
}

struct BridgeState {
    visible: bool,
    tokens: TokenSource,
    document: Option<DocumentToken>,
    binding: Option<Binding>,
    /// The document generation the subscriptions and pending calls belong to.
    generation: u64,
    buckets: Option<Buckets>,
    inbox: VecDeque<Inbound>,
    calls: Vec<PendingCall>,
    subs: BTreeMap<u32, Subscription>,
    next_sub: u32,
    outbox: VecDeque<Outbound>,
    counters_mount: Option<u64>,
    counters: IpcCounters,
    total: IpcCounters,
}

impl Drop for BridgeState {
    fn drop(&mut self) {
        self.unbind();
    }
}

impl BridgeState {
    fn new() -> Self {
        Self {
            visible: false,
            tokens: TokenSource::new(),
            document: None,
            binding: None,
            generation: 0,
            buckets: None,
            inbox: VecDeque::new(),
            calls: Vec::new(),
            subs: BTreeMap::new(),
            next_sub: 1,
            outbox: VecDeque::new(),
            counters_mount: None,
            counters: IpcCounters::default(),
            total: IpcCounters::default(),
        }
    }

    fn count(&mut self, mount_id: Option<u64>, bump: impl Fn(&mut IpcCounters)) {
        bump(&mut self.total);
        if let Some(mount_id) = mount_id {
            if self.counters_mount != Some(mount_id) {
                self.counters_mount = Some(mount_id);
                self.counters = IpcCounters::default();
            }
            bump(&mut self.counters);
        }
    }

    fn buckets(&mut self, mount_id: u64, now: Instant) -> &mut Buckets {
        if self.buckets.as_ref().is_none_or(|buckets| buckets.mount_id != mount_id) {
            self.buckets = Some(Buckets::new(mount_id, now));
        }
        self.buckets.get_or_insert_with(|| Buckets::new(mount_id, now))
    }

    fn pending(&self) -> usize {
        self.inbox.len() + self.calls.len() + self.subs.values().filter(|sub| sub.awaiting.is_some()).count()
    }

    // ---- checks 1-3 ------------------------------------------------------------------------

    fn ingress(&mut self, mounts: &PortalMounts, source: &str, body: &str, now: Instant) -> Ingress {
        // 1. Sender: the exact origin of the live Admitted mount, read only within the P-2 cap.
        let label = (source.len() <= limits::MAX_SOURCE_BYTES)
            .then(|| origin::parse(source))
            .flatten();
        let mount = label
            .as_ref()
            .and_then(|label| mounts.lookup(label))
            .filter(|mount| mount.mode == MountMode::Admitted);
        let Some(mount) = mount else {
            self.count(None, |c| c.dropped_origin += 1);
            return Ingress::Dropped;
        };
        // 2. Current: visible, and the body starts with this generation's token (constant-time).
        let generation = mounts.generation();
        let current = self
            .document
            .as_ref()
            .filter(|document| self.visible && document.mount_id == mount.mount_id && document.generation == generation)
            .and_then(|document| verified_prefix(body, &document.token).map(|end| (end, document.token.clone())));
        let Some((prefix_end, token)) = current else {
            self.count(Some(mount.mount_id), |c| c.dropped_origin += 1);
            return Ingress::Dropped;
        };
        // 3. Budget: id, size, message rate, pending replies.
        let Some((id, rest)) = split_id(&body[prefix_end..]) else {
            self.count(Some(mount.mount_id), |c| c.invalid += 1);
            return Ingress::Dropped;
        };
        let to = ReplyTo {
            mount_id: mount.mount_id,
            generation,
            id,
            token,
        };
        if body.len() > limits::MAX_ENVELOPE_BYTES {
            self.reply(&to, Err(IpcError::Invalid));
            return Ingress::Replied;
        }
        if !self.buckets(mount.mount_id, now).message.try_take(now) || self.pending() >= limits::MAX_PENDING_REPLIES {
            self.reply(&to, Err(IpcError::RateLimited));
            return Ingress::Replied;
        }
        let Some(fields) = rest.strip_prefix(',') else {
            self.reply(&to, Err(IpcError::Invalid));
            return Ingress::Replied;
        };
        self.inbox.push_back(Inbound {
            mount_id: mount.mount_id,
            generation,
            id,
            remainder: format!("{{{fields}"),
        });
        Ingress::Accepted
    }

    // ---- following the mount table -----------------------------------------------------------

    fn sync(&mut self, mounts: &PortalMounts, broker: Option<&Arc<dyn PortalBroker>>, now: Instant) {
        let live = mounts
            .current()
            .filter(|mount| mount.mode == MountMode::Admitted && !mount.is_faulted());
        let live_id = live.as_ref().map(|mount| mount.mount_id);
        let generation = mounts.generation();
        // A broker injected after the mount bound gets one chance to open a client.
        let late_broker = broker.is_some() && self.binding.as_ref().is_some_and(|binding| !binding.broker_attempted);
        if late_broker || self.binding.as_ref().map(|binding| binding.mount_id) != live_id {
            self.unbind();
            self.retain_queues(live_id.map(|id| (id, generation)));
            if let Some(mount) = &live {
                self.bind(mount, broker, now);
            }
            self.generation = generation;
        } else if generation != self.generation {
            // Subscriptions belong to one document generation (§5.6).
            self.close_document();
            self.retain_queues(live_id.map(|id| (id, generation)));
            self.generation = generation;
        }
        let paused = !self.visible;
        if let Some(binding) = self.binding.as_mut() {
            if let Some(open) = binding.client.as_ref() {
                if binding.paused != paused {
                    open.broker.set_paused(open.info.client, paused);
                    binding.paused = paused;
                }
            }
        }
    }

    fn bind(&mut self, mount: &Mount, broker: Option<&Arc<dyn PortalBroker>>, now: Instant) {
        let Some(publisher_did) = mount.publisher_did.clone() else {
            return;
        };
        let client = broker.and_then(|broker| {
            let request = BindRequest {
                mount_id: mount.mount_id,
                repository_node_id: &mount.node_id,
                package_id: &mount.package_id,
                content_hash: &mount.content_hash,
                publisher_did: &publisher_did,
                portal: &mount.portal,
            };
            match broker.open(&request) {
                Ok(info) => Some(OpenClient {
                    broker: Arc::clone(broker),
                    info,
                }),
                Err(error) => {
                    tracing::warn!(package_id = %mount.package_id, "portal bridge has no broker client: {}", error.code());
                    None
                }
            }
        });
        let capabilities = match &client {
            Some(open) => declared_capabilities(&mount.portal)
                .into_iter()
                .filter(|capability| open.info.granted.contains(capability))
                .collect(),
            None => BTreeSet::new(),
        };
        self.buckets(mount.mount_id, now);
        self.binding = Some(Binding {
            mount_id: mount.mount_id,
            node_id: mount.node_id.clone(),
            package_id: mount.package_id.clone(),
            content_hash: mount.content_hash.clone(),
            version: mount.archive.manifest().version.clone(),
            publisher_did,
            declared: mount.portal.clone(),
            capabilities,
            client,
            broker_attempted: broker.is_some(),
            paused: false,
        });
    }

    /// End the document: its subscriptions close at the broker and its pending calls are abandoned.
    fn close_document(&mut self) {
        if let Some(open) = self.binding.as_ref().and_then(|binding| binding.client.as_ref()) {
            for sub in self.subs.values() {
                open.broker.unsubscribe(open.info.client, sub.broker_sub);
            }
        }
        self.subs.clear();
        self.calls.clear();
    }

    /// Unmount or revoke: no subscription, call or client survives (spec §6.6).
    fn unbind(&mut self) {
        self.close_document();
        if let Some(open) = self.binding.take().and_then(|binding| binding.client) {
            open.broker.close(open.info.client);
        }
    }

    fn retain_queues(&mut self, keep: Option<(u64, u64)>) {
        self.inbox
            .retain(|entry| keep == Some((entry.mount_id, entry.generation)));
        self.outbox
            .retain(|entry| keep == Some((entry.mount_id, entry.generation)));
    }

    // ---- replies and deliveries ----------------------------------------------------------------

    fn reply_to(&self, id: u32) -> Option<ReplyTo> {
        let mount_id = self.binding.as_ref()?.mount_id;
        let document = self
            .document
            .as_ref()
            .filter(|document| document.mount_id == mount_id && document.generation == self.generation)?;
        Some(ReplyTo {
            mount_id,
            generation: self.generation,
            id,
            token: document.token.clone(),
        })
    }

    fn reply(&mut self, to: &ReplyTo, result: Result<Value, IpcError>) {
        let envelope = match &result {
            Ok(data) => json!({ "v": 1, "n": to.token, "re": to.id, "ok": true, "data": data }),
            Err(error) => json!({
                "v": 1, "n": to.token, "re": to.id, "ok": false,
                "error": { "code": error.code(), "message": error.message() }
            }),
        };
        match result {
            Ok(_) => self.count(Some(to.mount_id), |c| c.accepted += 1),
            Err(error) => self.count(Some(to.mount_id), |c| error.count(c)),
        }
        self.push_outbox(to.mount_id, to.generation, envelope.to_string());
    }

    fn reply_current(&mut self, id: u32, result: Result<Value, IpcError>) {
        if let Some(to) = self.reply_to(id) {
            self.reply(&to, result);
        }
    }

    fn deliver(&mut self, sub_id: u32, kind: DeliveryKind, data: Value) {
        let Some(to) = self.reply_to(0) else { return };
        let Some(sub) = self.subs.get_mut(&sub_id) else { return };
        let dropped = std::mem::take(&mut sub.dropped);
        let seq = sub.seq;
        sub.seq += 1;
        let mut envelope = json!({
            "v": 1, "n": to.token, "sub": sub_id, "seq": seq, "kind": kind.as_str(), "data": data
        });
        if dropped > 0 {
            envelope["dropped"] = json!(dropped);
        }
        self.count(Some(to.mount_id), |c| c.deliveries += 1);
        self.push_outbox(to.mount_id, to.generation, envelope.to_string());
    }

    fn push_outbox(&mut self, mount_id: u64, generation: u64, json: String) {
        if self.outbox.len() >= limits::MAX_OUTBOX {
            self.outbox.pop_front();
        }
        self.outbox.push_back(Outbound {
            mount_id,
            generation,
            json,
        });
    }

    // ---- checks 4-6 ------------------------------------------------------------------------

    fn process_inbox(&mut self, now: Instant, intents: &mut Vec<PortalIntent>) {
        while let Some(inbound) = self.inbox.pop_front() {
            let bound = self.binding.as_ref().map(|binding| binding.mount_id);
            let to = (bound == Some(inbound.mount_id) && inbound.generation == self.generation)
                .then(|| self.reply_to(inbound.id))
                .flatten();
            let Some(to) = to else {
                // The document changed between receipt and routing: a stale post.
                self.count(Some(inbound.mount_id), |c| c.dropped_origin += 1);
                continue;
            };
            match serde_json::from_str::<PageMessage>(&inbound.remainder) {
                Ok(message) => self.route(to, message, now, intents),
                Err(_) => self.reply(&to, Err(IpcError::Invalid)),
            }
        }
    }

    fn route(&mut self, to: ReplyTo, message: PageMessage, now: Instant, intents: &mut Vec<PortalIntent>) {
        let audit = match &message {
            PageMessage::Subscribe { stream, .. } => Some(("subscribe", stream.clone())),
            PageMessage::Act { key, .. } => Some(("act", format!("set_property:{key}"))),
            PageMessage::Invoke { package, command, .. } => Some(("invoke", format!("{package}/{command}"))),
            _ => None,
        };
        let outcome = match message {
            PageMessage::Hello {} => Ok(Some(self.hello())),
            PageMessage::Subscribe { stream, params } => self.subscribe(&to, &stream, &params),
            PageMessage::Unsubscribe { sub } => self.unsubscribe(sub).map(Some),
            PageMessage::Act {
                op: ActOp::SetProperty,
                node_id,
                key,
                value,
            } => self.act(&to, node_id, key, &value, now),
            PageMessage::Invoke { package, command, args } => self.invoke(&to, &package, &command, &args, now),
            PageMessage::Emit { stream, data } => self.emit(&stream, &data, now).map(Some),
            PageMessage::Intent { intent, node_id } => self.intent(intent, node_id, now, intents).map(Some),
        };
        if let (Err(IpcError::CapabilityDenied), Some((operation, target))) = (&outcome, &audit) {
            self.audit_refusal(operation, target, now);
        }
        match outcome {
            Ok(Some(data)) => self.reply(&to, Ok(data)),
            Ok(None) => {}
            Err(error) => self.reply(&to, Err(error)),
        }
    }

    fn client(&self) -> Result<(&Binding, &OpenClient), IpcError> {
        let binding = self.binding.as_ref().ok_or(IpcError::Unavailable)?;
        let open = binding.client.as_ref().ok_or(IpcError::Unavailable)?;
        Ok((binding, open))
    }

    /// Check-5 refusals are audited within the audit budget; past it they are counted only.
    fn audit_refusal(&mut self, operation: &str, target: &str, now: Instant) {
        let Some(mount_id) = self.binding.as_ref().map(|binding| binding.mount_id) else {
            return;
        };
        if !self.buckets(mount_id, now).audit.try_take(now) {
            return;
        }
        if let Ok((_, open)) = self.client() {
            let code = IpcError::CapabilityDenied.code();
            open.broker
                .audit_refusal(open.info.client, operation, truncate(target, MAX_AUDIT_TARGET_BYTES), code);
        }
    }

    fn hello(&self) -> Value {
        let Some(binding) = &self.binding else {
            return json!({ "api": API_NAME, "capabilities": [], "context": null, "limits": limits::hello_limits() });
        };
        let petal_id = binding.client.as_ref().map(|open| open.info.petal_id.clone());
        json!({
            "api": API_NAME,
            "capabilities": binding.capabilities,
            "context": {
                "package_id": binding.package_id,
                "content_hash": binding.content_hash,
                "version": binding.version,
                "publisher_did": binding.publisher_did,
                "repository_node_id": binding.node_id,
                "petal_id": petal_id,
            },
            "limits": limits::hello_limits(),
        })
    }

    fn subscribe(&mut self, to: &ReplyTo, stream: &str, params: &Map<String, Value>) -> Result<Option<Value>, IpcError> {
        let (plan, broker_sub) = {
            let (binding, open) = self.client()?;
            let plan = plan_subscription(&binding.declared, &open.info.petal_id, stream, params)?;
            if !binding.capabilities.contains(&plan.capability) {
                return Err(IpcError::CapabilityDenied);
            }
            if self.subs.len() >= limits::MAX_LIVE_SUBSCRIPTIONS {
                return Err(IpcError::TooManySubscriptions);
            }
            let broker_sub = open.broker.subscribe(open.info.client, &plan.address, &plan.params)?;
            (plan, broker_sub)
        };
        let sub_id = self.next_sub;
        self.next_sub = self.next_sub.checked_add(1).unwrap_or(1);
        let awaiting = plan.node_filter.is_some().then_some(to.id);
        self.subs.insert(sub_id, Subscription::new(broker_sub, plan.kind, plan.node_filter, awaiting));
        if awaiting.is_some() {
            return Ok(None);
        }
        if plan.kind == StreamKind::Readings {
            // The readings stream is deltas only; the page still gets one empty snapshot first (§5.2).
            self.reply(to, Ok(json!({ "sub": sub_id })));
            self.deliver(sub_id, DeliveryKind::Snapshot, json!({ "readings": [] }));
            return Ok(None);
        }
        Ok(Some(json!({ "sub": sub_id })))
    }

    fn unsubscribe(&mut self, sub_id: u32) -> Result<Value, IpcError> {
        let sub = self.subs.remove(&sub_id).ok_or(IpcError::NotFound)?;
        if let Ok((_, open)) = self.client() {
            open.broker.unsubscribe(open.info.client, sub.broker_sub);
        }
        Ok(json!({}))
    }

    fn act(&mut self, to: &ReplyTo, node_id: String, key: String, value: &Value, now: Instant) -> Result<Option<Value>, IpcError> {
        let mount_id = {
            let (binding, _) = self.client()?;
            let patterns = binding
                .declared
                .commands
                .set_property
                .as_ref()
                .map(|set| set.keys.as_slice())
                .unwrap_or(&[]);
            // Undeclared and host-reserved keys stop here: nothing reaches the DB (§6.1 step 1).
            if !binding.capabilities.contains("act:set_property") || !key_allowed(patterns, &key) {
                return Err(IpcError::CapabilityDenied);
            }
            binding.mount_id
        };
        if !is_node_id(&node_id) || json_len(value) > limits::MAX_ARGUMENT_BYTES {
            return Err(IpcError::Invalid);
        }
        if !self.buckets(mount_id, now).set_property.try_take(now) {
            return Err(IpcError::RateLimited);
        }
        let reply = {
            let (_, open) = self.client()?;
            open.broker.set_property(open.info.client, &node_id, &key, value)?
        };
        self.calls.push(PendingCall {
            id: to.id,
            kind: CallKind::Act { node_id, key },
            reply,
            deadline: now + limits::REPLY_DEADLINE,
        });
        Ok(None)
    }

    fn invoke(&mut self, to: &ReplyTo, package: &str, command: &str, args: &Value, now: Instant) -> Result<Option<Value>, IpcError> {
        let mount_id = {
            let (binding, _) = self.client()?;
            let declared = binding
                .declared
                .commands
                .invoke
                .iter()
                .any(|target| target.package == package && target.command == command);
            if !declared || !binding.capabilities.contains(format!("invoke:{package}/{command}").as_str()) {
                return Err(IpcError::CapabilityDenied);
            }
            binding.mount_id
        };
        if json_len(args) > limits::MAX_ARGUMENT_BYTES {
            return Err(IpcError::Invalid);
        }
        if !self.buckets(mount_id, now).invoke.try_take(now) {
            return Err(IpcError::RateLimited);
        }
        let reply = {
            let (_, open) = self.client()?;
            open.broker.invoke(open.info.client, package, command, args)?
        };
        self.calls.push(PendingCall {
            id: to.id,
            kind: CallKind::Invoke,
            reply,
            deadline: now + limits::REPLY_DEADLINE,
        });
        Ok(None)
    }

    fn emit(&mut self, stream: &str, data: &Value, now: Instant) -> Result<Value, IpcError> {
        let (mount_id, max_event_bytes, max_rate_hz) = {
            let (binding, _) = self.client()?;
            let declared = binding.declared.streams.iter().find(|declared| declared.id == stream);
            let granted = binding.capabilities.contains(format!("emit:{stream}").as_str());
            let Some(declared) = declared.filter(|_| granted) else {
                return Err(IpcError::CapabilityDenied);
            };
            (binding.mount_id, declared.max_event_bytes as usize, declared.max_rate_hz)
        };
        if json_len(data) > max_event_bytes {
            return Err(IpcError::Invalid);
        }
        let allowed = self
            .buckets(mount_id, now)
            .emit
            .entry(stream.to_owned())
            .or_insert_with(|| TokenBucket::for_stream(max_rate_hz, now))
            .try_take(now);
        if !allowed {
            return Err(IpcError::RateLimited);
        }
        let (_, open) = self.client()?;
        open.broker.emit(open.info.client, stream, data)?;
        Ok(json!({}))
    }

    fn intent(&mut self, intent: UiIntent, node_id: String, now: Instant, intents: &mut Vec<PortalIntent>) -> Result<Value, IpcError> {
        let (mount_id, petal_id, package_id) = {
            let (binding, open) = self.client()?;
            let capability = format!("intent:{}", intent.as_str());
            if !binding.capabilities.contains(capability.as_str()) {
                return Err(IpcError::CapabilityDenied);
            }
            (binding.mount_id, open.info.petal_id.clone(), binding.package_id.clone())
        };
        if !is_node_id(&node_id) {
            return Err(IpcError::Invalid);
        }
        if !self.buckets(mount_id, now).intent.try_take(now) {
            return Err(IpcError::RateLimited);
        }
        // Uniform `{}`: whether the node exists in the petal is fe-ui's to check, so no existence oracle.
        intents.push(PortalIntent {
            intent,
            node_id,
            petal_id,
            package_id,
        });
        Ok(json!({}))
    }

    // ---- broker results ------------------------------------------------------------------------

    fn poll_calls(&mut self, now: Instant) {
        for call in std::mem::take(&mut self.calls) {
            let outcome = match call.reply.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) if now < call.deadline => {
                    self.calls.push(call);
                    continue;
                }
                // A timeout or a vanished writer: the write may or may not have landed; never retried.
                Err(_) => Err(IpcError::UnknownOutcome),
            };
            let data = outcome.map(|value| match call.kind {
                CallKind::Act { node_id, key } => json!({ "node_id": node_id, "key": key }),
                CallKind::Invoke => json!({ "result": value }),
            });
            self.reply_current(call.id, data);
        }
    }

    fn drain_broker(&mut self) {
        let Some((broker, client)) = self
            .binding
            .as_ref()
            .and_then(|binding| binding.client.as_ref())
            .map(|open| (Arc::clone(&open.broker), open.info.client))
        else {
            return;
        };
        for event in broker.take_events(client, limits::MAX_BROKER_EVENTS_PER_FRAME) {
            let sub_id = self
                .subs
                .iter()
                .find(|(_, sub)| sub.broker_sub == event.subscription)
                .map(|(id, _)| *id);
            if let Some(sub_id) = sub_id {
                self.absorb(sub_id, event, &broker, client);
            }
        }
    }

    fn absorb(&mut self, sub_id: u32, event: BrokerEvent, broker: &Arc<dyn PortalBroker>, client: u64) {
        let effect = {
            let Some(sub) = self.subs.get_mut(&sub_id) else { return };
            sub.dropped += event.dropped_before;
            match event.kind {
                DeliveryKind::Snapshot => match (sub.node_filter.as_deref(), sub.awaiting) {
                    (Some(filter), Some(request)) => {
                        sub.scoped.extend(nodes_matching(&event.data, filter));
                        let last = event.data.get("last").and_then(Value::as_bool).unwrap_or(true);
                        if !last {
                            Effect::Nothing
                        } else if sub.scoped.is_empty() {
                            Effect::ScopeMissing {
                                request,
                                broker_sub: sub.broker_sub,
                            }
                        } else {
                            sub.awaiting = None;
                            Effect::ScopeProven {
                                request,
                                nodes: std::mem::take(&mut sub.scoped),
                            }
                        }
                    }
                    (Some(filter), None) => {
                        let nodes = nodes_matching(&event.data, filter);
                        let mut data = event.data;
                        if let Value::Object(map) = &mut data {
                            map.insert("nodes".into(), Value::Array(nodes));
                        }
                        Effect::Deliver(DeliveryKind::Snapshot, data)
                    }
                    (None, _) => Effect::Deliver(DeliveryKind::Snapshot, event.data),
                },
                DeliveryKind::Delta => {
                    sub.merge(event.data);
                    Effect::Nothing
                }
                DeliveryKind::ResyncRequired => {
                    sub.discard_pending();
                    match sub.awaiting {
                        Some(_) => Effect::Nothing,
                        None => Effect::Deliver(DeliveryKind::ResyncRequired, event.data),
                    }
                }
                DeliveryKind::Ended => match sub.awaiting {
                    Some(request) => Effect::EndAwaiting {
                        request,
                        error: end_error(&event.data),
                    },
                    None => Effect::DeliverAndEnd(event.data),
                },
            }
        };
        match effect {
            Effect::Nothing => {}
            Effect::Deliver(kind, data) => self.deliver(sub_id, kind, data),
            Effect::DeliverAndEnd(data) => {
                self.deliver(sub_id, DeliveryKind::Ended, data);
                self.subs.remove(&sub_id);
            }
            Effect::ScopeProven { request, nodes } => {
                self.reply_current(request, Ok(json!({ "sub": sub_id })));
                self.deliver(sub_id, DeliveryKind::Snapshot, json!({ "part": 0, "last": true, "nodes": nodes }));
            }
            Effect::ScopeMissing { request, broker_sub } => {
                // Outside the binding petal: the same not_found REST gives (spec R9).
                self.subs.remove(&sub_id);
                broker.unsubscribe(client, broker_sub);
                self.reply_current(request, Err(IpcError::NotFound));
            }
            Effect::EndAwaiting { request, error } => {
                self.subs.remove(&sub_id);
                self.reply_current(request, Err(error));
            }
        }
    }

    /// Deltas go out at most once per [`limits::DELIVERY_INTERVAL`] per subscription, leading edge first.
    fn flush_deltas(&mut self, now: Instant) {
        let due: Vec<(u32, Value)> = self
            .subs
            .iter_mut()
            .filter(|(_, sub)| sub.awaiting.is_none() && sub.has_pending())
            .filter(|(_, sub)| {
                sub.last_delta
                    .is_none_or(|at| now.saturating_duration_since(at) >= limits::DELIVERY_INTERVAL)
            })
            .map(|(id, sub)| {
                sub.last_delta = Some(now);
                (*id, sub.take_delta())
            })
            .collect();
        for (sub_id, data) in due {
            self.deliver(sub_id, DeliveryKind::Delta, data);
        }
    }
}

// ---------------------------------------------------------------------------
// Pure checks
// ---------------------------------------------------------------------------

/// End of `{"v":1,"n":"<token>","id":` when `body` starts with it; the token bytes compare in constant time.
fn verified_prefix(body: &str, token: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let token_start = PREFIX_HEAD.len();
    let token_end = token_start + token.len();
    let end = token_end + PREFIX_TAIL.len();
    if bytes.len() < end
        || &bytes[..token_start] != PREFIX_HEAD.as_bytes()
        || &bytes[token_end..end] != PREFIX_TAIL.as_bytes()
    {
        return None;
    }
    let difference = bytes[token_start..token_end]
        .iter()
        .zip(token.as_bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    (difference == 0).then_some(end)
}

/// The decimal envelope id (at most [`limits::MAX_ID_DIGITS`] digits, within `u32`) and what follows it.
fn split_id(rest: &str) -> Option<(u32, &str)> {
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > limits::MAX_ID_DIGITS {
        return None;
    }
    let id = rest[..digits].parse::<u64>().ok().and_then(|id| u32::try_from(id).ok())?;
    Some((id, &rest[digits..]))
}

/// One planned broker subscription: check 5 passed against the manifest and the parameters are typed.
#[derive(Debug, Clone, PartialEq)]
struct Plan {
    kind: StreamKind,
    address: String,
    params: Value,
    capability: String,
    node_filter: Option<String>,
}

/// Page `subscribe` → bus address and params (bus.md §8 portal mapping).
fn plan_subscription(
    declared: &PortalSection,
    petal_id: &str,
    stream: &str,
    params: &Map<String, Value>,
) -> Result<Plan, IpcError> {
    match stream {
        "node" => {
            if !declared.subscriptions.node {
                return Err(IpcError::CapabilityDenied);
            }
            only_string_keys(params, &["petal_id", "node_id"])?;
            if params.get("petal_id").and_then(Value::as_str).is_some_and(|petal| petal != petal_id) {
                return Err(IpcError::NotFound);
            }
            let node_filter = params.get("node_id").and_then(Value::as_str).map(str::to_owned);
            if node_filter.as_deref().is_some_and(|node| !is_node_id(node)) {
                return Err(IpcError::Invalid);
            }
            Ok(Plan {
                kind: StreamKind::Node,
                address: "host/node-changes".into(),
                params: json!({}),
                capability: "subscribe:node".into(),
                node_filter,
            })
        }
        "readings" => {
            if !declared.subscriptions.readings {
                return Err(IpcError::CapabilityDenied);
            }
            only_string_keys(params, &["node_id", "metric"])?;
            let mut bus = Map::new();
            if let Some(node) = params.get("node_id").and_then(Value::as_str) {
                if !is_node_id(node) {
                    return Err(IpcError::Invalid);
                }
                bus.insert("anchors".into(), json!([node]));
            }
            if let Some(metric) = params.get("metric").and_then(Value::as_str) {
                bus.insert("metrics".into(), json!([metric]));
            }
            Ok(Plan {
                kind: StreamKind::Readings,
                address: "host/readings".into(),
                params: Value::Object(bus),
                capability: "subscribe:readings".into(),
                node_filter: None,
            })
        }
        "query" => {
            let name = params.get("name").and_then(Value::as_str).ok_or(IpcError::Invalid)?;
            let query = declared
                .subscriptions
                .queries
                .iter()
                .find(|query| query.name == name)
                .ok_or(IpcError::CapabilityDenied)?;
            if params.keys().any(|key| key != "name" && key != "args") {
                return Err(IpcError::Invalid);
            }
            let args = match params.get("args") {
                None => Map::new(),
                Some(Value::Object(args)) => args.clone(),
                Some(_) => return Err(IpcError::Invalid),
            };
            check_query_args(&query.params, &args)?;
            Ok(Plan {
                kind: StreamKind::Query,
                address: "host/query".into(),
                params: json!({ "query": name, "vars": args }),
                capability: format!("subscribe:query/{name}"),
                node_filter: None,
            })
        }
        source => {
            if !declared.hooks.iter().any(|hook| hook.source == source) {
                return Err(IpcError::CapabilityDenied);
            }
            if !params.is_empty() {
                return Err(IpcError::Invalid);
            }
            Ok(Plan {
                kind: StreamKind::Bus,
                address: source.to_owned(),
                params: json!({}),
                capability: format!("subscribe:{source}"),
                node_filter: None,
            })
        }
    }
}

fn only_string_keys(params: &Map<String, Value>, allowed: &[&str]) -> Result<(), IpcError> {
    let ok = params
        .iter()
        .all(|(key, value)| allowed.contains(&key.as_str()) && value.is_string());
    if ok {
        Ok(())
    } else {
        Err(IpcError::Invalid)
    }
}

/// Every declared parameter, of its declared type, and nothing else (README §5 query params).
fn check_query_args(declared: &BTreeMap<String, String>, args: &Map<String, Value>) -> Result<(), IpcError> {
    if args.len() != declared.len() {
        return Err(IpcError::Invalid);
    }
    for (name, kind) in declared {
        let value = args.get(name).ok_or(IpcError::Invalid)?;
        let typed = match kind.as_str() {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "node_id" => value.as_str().is_some_and(is_node_id),
            _ => false,
        };
        if !typed {
            return Err(IpcError::Invalid);
        }
    }
    Ok(())
}

/// The capability strings a manifest declares (ipc-v1.md §4.3).
fn declared_capabilities(portal: &PortalSection) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if portal.subscriptions.node {
        out.insert("subscribe:node".to_owned());
    }
    if portal.subscriptions.readings {
        out.insert("subscribe:readings".to_owned());
    }
    out.extend(portal.subscriptions.queries.iter().map(|query| format!("subscribe:query/{}", query.name)));
    out.extend(portal.hooks.iter().map(|hook| format!("subscribe:{}", hook.source)));
    if portal.commands.set_property.is_some() {
        out.insert("act:set_property".to_owned());
    }
    out.extend(portal.commands.invoke.iter().map(|target| format!("invoke:{}/{}", target.package, target.command)));
    out.extend(portal.streams.iter().map(|stream| format!("emit:{}", stream.id)));
    out.extend(portal.ui_intents.iter().map(|intent| format!("intent:{intent}")));
    out
}

/// README §5 key grammar against the declared patterns; `node_kind` and `portal.*` stay host-reserved regardless.
fn key_allowed(patterns: &[String], key: &str) -> bool {
    let grammar = key.len() <= 128
        && key.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        });
    let reserved = key == "node_kind" || key == "portal" || key.starts_with("portal.");
    grammar
        && !reserved
        && patterns.iter().any(|pattern| match pattern.strip_suffix(".*") {
            Some(base) => key.strip_prefix(base).is_some_and(|rest| rest.len() > 1 && rest.starts_with('.')),
            None => pattern == key,
        })
}

/// A node id as the page may name one; scope is always proven at use.
fn is_node_id(node_id: &str) -> bool {
    !node_id.is_empty()
        && node_id.len() <= MAX_NODE_ID_BYTES
        && node_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn nodes_matching(data: &Value, node_id: &str) -> Vec<Value> {
    data.get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter(|node| node.get("node_id").and_then(Value::as_str) == Some(node_id))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// A bus `ended` reason for a subscription whose reply was still waiting.
fn end_error(data: &Value) -> IpcError {
    match data.get("reason").and_then(Value::as_str) {
        Some("revoked" | "consent-revoked") => IpcError::Revoked,
        Some("policy-denied") => IpcError::PolicyDenied,
        Some("role-lost") => IpcError::RoleDenied,
        _ => IpcError::Unavailable,
    }
}

fn json_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// ---------------------------------------------------------------------------
// Document tokens
// ---------------------------------------------------------------------------

/// BLAKE3 keyed by a per-process secret seeded from OS randomness; one 128-bit token per document.
struct TokenSource {
    key: [u8; 32],
    counter: u64,
}

impl TokenSource {
    fn new() -> Self {
        Self {
            key: blake3::derive_key(TOKEN_CONTEXT, &seed_material()),
            counter: 0,
        }
    }

    /// 32 lowercase hex characters.
    fn next(&mut self, mount_id: u64, generation: u64) -> String {
        self.counter = self.counter.wrapping_add(1);
        let mut hasher = blake3::Hasher::new_keyed(&self.key);
        hasher.update(&self.counter.to_le_bytes());
        hasher.update(&mount_id.to_le_bytes());
        hasher.update(&generation.to_le_bytes());
        hasher.update(&unix_nanos().to_le_bytes());
        hasher.finalize().to_hex()[..32].to_owned()
    }
}

/// Seed: std's OS-randomized SipHash keys, plus `CoCreateGuid` (a CSPRNG v4 GUID) on Windows.
fn seed_material() -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut seed = Vec::with_capacity(96);
    for round in 0u64..4 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(round);
        seed.extend_from_slice(&hasher.finish().to_le_bytes());
    }
    seed.extend_from_slice(&unix_nanos().to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    #[cfg(windows)]
    for _ in 0..2 {
        // SAFETY: CoCreateGuid takes no arguments and returns the GUID by value.
        if let Ok(guid) = unsafe { windows::Win32::System::Com::CoCreateGuid() } {
            seed.extend_from_slice(&guid.to_u128().to_le_bytes());
        }
    }
    seed
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prefix, id and key-pattern rules: the string checks that run before anything is decoded.
    #[test]
    fn envelope_prefix_id_and_key_rules() {
        let token = "0123456789abcdef0123456789abcdef";
        let good = format!(r#"{{"v":1,"n":"{token}","id":7,"t":"hello"}}"#);
        let end = verified_prefix(&good, token).expect("prefix");
        assert_eq!(split_id(&good[end..]), Some((7, r#","t":"hello"}"#)));
        for body in [
            format!(r#"{{"v":1,"n":"{}","id":7}}"#, "f".repeat(32)),
            format!(r#"{{"n":"{token}","v":1,"id":7}}"#),
            format!(r#"{{ "v":1,"n":"{token}","id":7}}"#),
            r#"{"v":1,"n":"short","id":7}"#.to_owned(),
            String::new(),
        ] {
            assert_eq!(verified_prefix(&body, token), None, "{body}");
        }
        for (rest, expected) in [
            ("4294967295,", Some((u32::MAX, ","))),
            ("4294967296,", None),
            ("12345678901,", None),
            (",", None),
            ("-1,", None),
        ] {
            assert_eq!(split_id(rest), expected, "{rest}");
        }
        let patterns = ["fleet.*".to_owned(), "console.note".to_owned()];
        for (key, allowed) in [
            ("fleet.speed", true),
            ("fleet.a.b", true),
            ("fleet", false),
            ("fleet.", false),
            ("fleetx.speed", false),
            ("console.note", true),
            ("console.note.x", false),
            ("node_kind", false),
            ("portal.bundle_asset", false),
            ("Fleet.speed", false),
        ] {
            assert_eq!(key_allowed(&patterns, key), allowed, "{key}");
        }
        assert!(!key_allowed(&["portal.*".to_owned()], "portal.bundle_asset"), "reserved whatever the patterns say");
    }
}
