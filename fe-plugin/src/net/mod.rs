//! Network brokers: declared ingress (HTTP/WS accept on the API mount, WS dial, MQTT) and egress; see net/AGENTS.md.

mod connect;
mod egress;
mod ingress;
pub mod mqtt;
mod mqtt_session;
mod ws_dial;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use fe_sdk::manifest_v2::{AuthScheme, AuthSlot, Egress, Ingress};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::bus::rate::TokenBucket;
use crate::bus::sources::{AuditAction, AuditEntry, AuditSink, RoleSource};
use crate::bus::{HostError, InstanceKey, InstanceSpec};
use crate::tick::Clock;

pub use egress::{IncomingResponse, OutgoingRequest};
pub use ingress::{HttpIngressRequest, IngressRefusal, WsAcceptRequest, WsSession};

/// The API server's body limit for ingress mounts; the endpoint's own `max_body_bytes` is checked after auth.
pub const INGRESS_BODY_CEILING: usize = 1024 * 1024;
/// How long fe-api waits for `on-ingress` before answering 504.
pub const INGRESS_REPLY_TIMEOUT_MS: u64 = 5_000;
/// Deliveries queued per instance before new frames are refused or dropped.
pub const INBOX_CAPACITY: usize = 256;
/// Egress requests in flight per instance.
pub const MAX_EGRESS_IN_FLIGHT: usize = 16;
/// Outbound frames or publishes queued per connection or session.
pub const OUTBOUND_QUEUE: usize = 64;

/// Headers the host never forwards to a guest and never lets a guest set, whatever the manifest says
/// (P2P-QA steer 2026-11-08 20:20; manifest-v2.md V16 is the second layer); one list in fe-sdk.
pub use fe_sdk::manifest_v2::is_credential_header;

/// A host-side secret named by a manifest `auth.credential` slot; never replicated, never shown to a guest.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    /// Required for `basic` and `mqtt-password`.
    pub username: Option<String>,
    pub secret: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential").field("username", &self.username).field("secret", &"<redacted>").finish()
    }
}

/// A filled credential with its scheme and optional header name.
type ShapedCredential = Option<(AuthScheme, Credential, Option<String>)>;

/// The operator-filled local credential store.
pub trait CredentialStore: Send + Sync {
    /// The secret in `slot`, or `None` while the operator has not filled it.
    fn get(&self, slot: &str) -> Option<Credential>;
}

/// A store with every slot empty: credentialed endpoints stay down with `credential-missing`.
pub struct NoCredentials;

impl CredentialStore for NoCredentials {
    fn get(&self, _slot: &str) -> Option<Credential> {
        None
    }
}

/// Services the binary injects; `runtime` is the plugin worker's Tokio runtime (None: no dial-outs, no egress).
#[derive(Clone)]
pub struct NetSources {
    pub roles: Arc<dyn RoleSource>,
    pub audit: Arc<dyn AuditSink>,
    pub credentials: Arc<dyn CredentialStore>,
    pub runtime: Option<tokio::runtime::Handle>,
}

/// One endpoint's state in extension status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "state", content = "reason")]
pub enum EndpointState {
    /// A passive endpoint (HTTP/WS accept mount, HTTP egress) ready to serve.
    Mounted,
    /// A dial-out connected.
    Up,
    /// A dial-out disconnected; the host retries with backoff.
    Down(String),
    /// The `auth.credential` slot is empty or does not fit the scheme.
    CredentialMissing,
    /// This host cannot serve it (no network runtime, a client that failed to build).
    NotAvailable(String),
}

/// The `ingress.*` / `egress.*` status counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NetCounters {
    pub ingress_frames: u64,
    pub ingress_dropped_size: u64,
    pub ingress_dropped_rate: u64,
    pub ingress_dropped_queue: u64,
    pub ingress_refused: u64,
    pub connections_open: u64,
    pub egress_requests: u64,
    pub egress_refused: u64,
    pub egress_failed: u64,
    pub mqtt_published: u64,
}

/// WIT `ingress-hooks.transport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Http,
    Websocket,
    Mqtt,
}

/// WIT `ingress-frame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressFrame {
    pub endpoint: String,
    pub transport: Transport,
    pub connection: Option<u64>,
    pub caller_did: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub topic: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub received_at_ms: u64,
}

/// WIT `ingress-reply`: an HTTP response, or one frame back on the same websocket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressReply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// WIT `connection-event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionEvent {
    Opened,
    Closed,
}

/// WIT `ws-frame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsFrame {
    Text(String),
    Binary(Vec<u8>),
}

impl WsFrame {
    fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Binary(bytes) => bytes.len(),
        }
    }
}

/// What a websocket connection's writer task sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsOutbound {
    Frame(WsFrame),
    Close { code: u16, reason: String },
}

/// The guest's `ingress-hooks` and `egress-hooks` exports as the worker calls them (E1 implements this).
pub trait NetHooks {
    fn on_ingress(&mut self, frame: IngressFrame) -> Option<IngressReply>;
    fn on_connection(&mut self, endpoint: &str, connection: u64, event: ConnectionEvent);
    fn on_egress_result(&mut self, request: u64, outcome: Result<IncomingResponse, HostError>);
}

/// Where an `on-ingress` reply goes.
enum ReplyTo {
    Http(oneshot::Sender<Option<IngressReply>>),
    Connection(u64),
    Dialed(String),
    Nowhere,
}

enum Delivery {
    Ingress { frame: Box<IngressFrame>, reply: ReplyTo },
    Connection { endpoint: String, connection: u64, event: ConnectionEvent },
    EgressResult { request: u64, outcome: Result<IncomingResponse, HostError> },
}

/// A spawned dial-out: its stop switch and, for senders, its outbound queue.
struct Session<T> {
    outbound: Option<mpsc::Sender<T>>,
    stop: watch::Sender<bool>,
}

/// An accepted websocket connection on an ingress mount.
struct AcceptedConnection {
    key: InstanceKey,
    endpoint: String,
    caller_did: String,
    max_frame_bytes: usize,
    bucket: TokenBucket,
    outbound: mpsc::Sender<WsOutbound>,
}

struct NetInstance {
    spec: InstanceSpec,
    generation: u64,
    inbox: VecDeque<Delivery>,
    endpoints: BTreeMap<String, EndpointState>,
    counters: NetCounters,
    http_callers: HashMap<(String, String), TokenBucket>,
    egress_buckets: HashMap<String, TokenBucket>,
    egress_in_flight: usize,
    egress_clients: HashMap<String, reqwest::Client>,
    dialers: HashMap<String, Session<WsOutbound>>,
    dial_connections: HashMap<String, u64>,
    mqtt: HashMap<String, Session<mqtt_session::MqttPublish>>,
}

impl NetInstance {
    /// Queue a delivery; frames are refused when the inbox is full, completions never are.
    fn push(&mut self, delivery: Delivery) -> bool {
        let completion = matches!(delivery, Delivery::EgressResult { .. } | Delivery::Connection { .. });
        if !completion && self.inbox.len() >= INBOX_CAPACITY {
            self.counters.ingress_dropped_queue += 1;
            return false;
        }
        self.inbox.push_back(delivery);
        true
    }

    fn ingress(&self, id: &str) -> Option<&Ingress> {
        self.spec.manifest.ingress.iter().find(|endpoint| ingress_id(endpoint) == id)
    }
}

struct NetState {
    next_id: u64,
    instances: HashMap<InstanceKey, NetInstance>,
    retired: HashMap<String, NetCounters>,
    connections: HashMap<u64, AcceptedConnection>,
}

/// The network brokers on one host: cheap to clone, shared by the worker, fe-api and the dial-out tasks.
#[derive(Clone)]
pub struct Net {
    inner: Arc<Mutex<NetState>>,
    clock: Arc<dyn Clock>,
    sources: NetSources,
}

impl Net {
    /// One broker per host, on the same injected clock as the scheduler.
    pub fn new(clock: Arc<dyn Clock>, sources: NetSources) -> Self {
        let state = NetState { next_id: 1, instances: HashMap::new(), retired: HashMap::new(), connections: HashMap::new() };
        Self { inner: Arc::new(Mutex::new(state)), clock, sources }
    }

    fn lock(&self) -> MutexGuard<'_, NetState> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    fn audit(&self, entry: AuditEntry) {
        self.sources.audit.record(entry);
    }

    fn denial(&self, actor: &str, installation_id: Option<String>, target: String, outcome: &str) -> AuditEntry {
        AuditEntry {
            action: AuditAction::Denial,
            actor_did: actor.to_string(),
            caller_kind: None,
            installation_id,
            target,
            args_digest: None,
            outcome: outcome.to_string(),
            at_ms: self.now(),
        }
    }

    /// Mount an admitted instance's endpoints and start its dial-outs (websocket dial, MQTT).
    pub fn register_instance(&self, spec: &InstanceSpec) -> Result<(), HostError> {
        let key = spec.key();
        let mut spawns: Vec<Spawn> = Vec::new();
        let generation = {
            let mut state = self.lock();
            if state.instances.contains_key(&key) {
                return Err(HostError::InvalidInput("an instance of this package is already bound at this petal".into()));
            }
            let generation = state.next_id;
            state.next_id += 1;
            let mut instance = NetInstance {
                spec: spec.clone(),
                generation,
                inbox: VecDeque::new(),
                endpoints: BTreeMap::new(),
                counters: NetCounters::default(),
                http_callers: HashMap::new(),
                egress_buckets: HashMap::new(),
                egress_in_flight: 0,
                egress_clients: HashMap::new(),
                dialers: HashMap::new(),
                dial_connections: HashMap::new(),
                mqtt: HashMap::new(),
            };
            let runtime = self.sources.runtime.is_some();
            let unavailable = || EndpointState::NotAvailable("this host has no network runtime".into());
            for endpoint in &spec.manifest.ingress {
                let id = ingress_id(endpoint).to_string();
                let status = match endpoint {
                    Ingress::HttpServer(_) | Ingress::WebsocketAccept(_) => EndpointState::Mounted,
                    Ingress::WebsocketDial(_) | Ingress::Mqtt(_) if !runtime => unavailable(),
                    Ingress::WebsocketDial(dial) => {
                        let (outbound, receiver) = mpsc::channel(OUTBOUND_QUEUE);
                        let (stop, stopped) = watch::channel(false);
                        instance.dialers.insert(id.clone(), Session { outbound: Some(outbound), stop });
                        spawns.push(Spawn::Dial(ws_dial::DialConfig::from_manifest(dial), receiver, stopped));
                        EndpointState::Down("connecting".into())
                    }
                    Ingress::Mqtt(mqtt) => {
                        let (stop, stopped) = watch::channel(false);
                        instance.mqtt.insert(id.clone(), Session { outbound: None, stop });
                        spawns.push(Spawn::Mqtt(mqtt_session::MqttConfig::ingress(spec, mqtt), None, stopped));
                        EndpointState::Down("connecting".into())
                    }
                };
                instance.endpoints.insert(id, status);
            }
            for endpoint in &spec.manifest.egress {
                let (id, status) = match endpoint {
                    Egress::Http(http) if runtime => match egress::build_client(http) {
                        Ok(client) => {
                            instance.egress_clients.insert(http.id.clone(), client);
                            (http.id.clone(), EndpointState::Mounted)
                        }
                        Err(error) => (http.id.clone(), EndpointState::NotAvailable(error)),
                    },
                    Egress::Http(http) => (http.id.clone(), unavailable()),
                    Egress::Mqtt(mqtt) if runtime => {
                        let (outbound, receiver) = mpsc::channel(OUTBOUND_QUEUE);
                        let (stop, stopped) = watch::channel(false);
                        instance.mqtt.insert(mqtt.id.clone(), Session { outbound: Some(outbound), stop });
                        spawns.push(Spawn::Mqtt(mqtt_session::MqttConfig::egress(spec, mqtt), Some(receiver), stopped));
                        (mqtt.id.clone(), EndpointState::Down("connecting".into()))
                    }
                    Egress::Mqtt(mqtt) => (mqtt.id.clone(), unavailable()),
                };
                instance.endpoints.insert(id, status);
            }
            state.retired.remove(&spec.installation_id);
            state.instances.insert(key.clone(), instance);
            generation
        };
        // Spawned after the lock is released; each task re-checks `generation` before touching state.
        if let Some(runtime) = &self.sources.runtime {
            for spawn in spawns {
                let net = self.clone();
                let key = key.clone();
                match spawn {
                    Spawn::Dial(config, outbound, stop) => {
                        runtime.spawn(ws_dial::run(net, key, generation, config, outbound, stop));
                    }
                    Spawn::Mqtt(config, publishes, stop) => {
                        runtime.spawn(mqtt_session::run(net, key, generation, config, publishes, stop));
                    }
                }
            }
        }
        Ok(())
    }

    /// Unmount an instance: dial-outs stop, accepted sockets close, queued frames are dropped (callers see 404).
    pub fn unregister_instance(&self, key: &InstanceKey) {
        let mut state = self.lock();
        let Some(instance) = state.instances.remove(key) else { return };
        state.retired.insert(instance.spec.installation_id.clone(), instance.counters.clone());
        for session in instance.dialers.values() {
            let _ = session.stop.send(true);
        }
        for session in instance.mqtt.values() {
            let _ = session.stop.send(true);
        }
        let accepted: Vec<u64> = state.connections.iter().filter(|(_, connection)| &connection.key == key).map(|(id, _)| *id).collect();
        for id in accepted {
            if let Some(connection) = state.connections.remove(&id) {
                let _ = connection.outbound.try_send(WsOutbound::Close { code: 1001, reason: "extension stopped".into() });
            }
        }
    }

    /// Hand at most `max_calls` deliveries to one instance's hooks on the worker; returns how many ran.
    pub fn dispatch(&self, key: &InstanceKey, hooks: &mut dyn NetHooks, max_calls: usize) -> usize {
        let mut calls = 0;
        while calls < max_calls {
            let delivery = self.lock().instances.get_mut(key).and_then(|instance| instance.inbox.pop_front());
            let Some(delivery) = delivery else { break };
            calls += 1;
            match delivery {
                Delivery::Ingress { frame, reply } => {
                    let answer = hooks.on_ingress(*frame);
                    self.route_reply(key, reply, answer);
                }
                Delivery::Connection { endpoint, connection, event } => hooks.on_connection(&endpoint, connection, event),
                Delivery::EgressResult { request, outcome } => hooks.on_egress_result(request, outcome),
            }
        }
        calls
    }

    fn route_reply(&self, key: &InstanceKey, reply: ReplyTo, answer: Option<IngressReply>) {
        match reply {
            ReplyTo::Http(sender) => {
                let _ = sender.send(answer);
            }
            ReplyTo::Connection(connection) => {
                if let Some(frame) = answer.map(reply_frame) {
                    let state = self.lock();
                    if let Some(open) = state.connections.get(&connection) {
                        let _ = open.outbound.try_send(WsOutbound::Frame(frame));
                    }
                }
            }
            ReplyTo::Dialed(endpoint) => {
                if let Some(frame) = answer.map(reply_frame) {
                    let state = self.lock();
                    let outbound = state.instances.get(key).and_then(|instance| instance.dialers.get(&endpoint)).and_then(|session| session.outbound.clone());
                    drop(state);
                    if let Some(outbound) = outbound {
                        let _ = outbound.try_send(WsOutbound::Frame(frame));
                    }
                }
            }
            ReplyTo::Nowhere => {}
        }
    }

    /// Endpoint states for extension status.
    pub fn endpoint_states(&self, key: &InstanceKey) -> Vec<(String, EndpointState)> {
        self.lock()
            .instances
            .get(key)
            .map(|instance| instance.endpoints.iter().map(|(id, state)| (id.clone(), state.clone())).collect())
            .unwrap_or_default()
    }

    /// Counters for extension status; kept after the instance stops until [`Net::forget`].
    pub fn counters_for_installation(&self, installation_id: &str) -> Option<NetCounters> {
        let state = self.lock();
        if let Some(counters) = state.retired.get(installation_id) {
            return Some(counters.clone());
        }
        state.instances.values().find(|instance| instance.spec.installation_id == installation_id).map(|instance| {
            let mut counters = instance.counters.clone();
            counters.connections_open = state.connections.values().filter(|open| open.key == instance.spec.key()).count() as u64;
            counters
        })
    }

    /// Drop a stopped installation's counters (uninstall).
    pub fn forget(&self, installation_id: &str) {
        self.lock().retired.remove(installation_id);
    }

    // ---- hooks for the dial-out tasks ----------------------------------------------------------

    /// Run `change` on the instance only if it is still the generation the task was spawned for.
    fn with_generation<R>(&self, key: &InstanceKey, generation: u64, change: impl FnOnce(&mut NetState) -> R) -> Option<R> {
        let mut state = self.lock();
        let current = state.instances.get(key).is_some_and(|instance| instance.generation == generation);
        current.then(|| change(&mut state))
    }

    fn set_endpoint_state(&self, key: &InstanceKey, generation: u64, endpoint: &str, status: EndpointState) {
        self.with_generation(key, generation, |state| {
            if let Some(instance) = state.instances.get_mut(key) {
                instance.endpoints.insert(endpoint.to_string(), status);
            }
        });
    }

    fn count(&self, key: &InstanceKey, generation: u64, change: impl FnOnce(&mut NetCounters)) {
        self.with_generation(key, generation, |state| {
            if let Some(instance) = state.instances.get_mut(key) {
                change(&mut instance.counters);
            }
        });
    }

    /// A credential for `slot` shaped for `scheme`, or why the endpoint must stay down.
    fn credential(&self, auth: &Option<AuthSlot>) -> Result<ShapedCredential, EndpointState> {
        let Some(auth) = auth else { return Ok(None) };
        let credential = self.sources.credentials.get(&auth.credential).ok_or(EndpointState::CredentialMissing)?;
        let needs_username = matches!(auth.scheme, AuthScheme::Basic | AuthScheme::MqttPassword);
        if needs_username && credential.username.is_none() {
            return Err(EndpointState::CredentialMissing);
        }
        Ok(Some((auth.scheme, credential, auth.header.clone())))
    }
}

enum Spawn {
    Dial(ws_dial::DialConfig, mpsc::Receiver<WsOutbound>, watch::Receiver<bool>),
    Mqtt(mqtt_session::MqttConfig, Option<mpsc::Receiver<mqtt_session::MqttPublish>>, watch::Receiver<bool>),
}

fn ingress_id(endpoint: &Ingress) -> &str {
    match endpoint {
        Ingress::HttpServer(http) => &http.id,
        Ingress::WebsocketAccept(accept) => &accept.id,
        Ingress::WebsocketDial(dial) => &dial.id,
        Ingress::Mqtt(mqtt) => &mqtt.id,
    }
}

/// A websocket reply frame: text for `text/*` and `application/json` that is valid UTF-8, else binary.
fn reply_frame(reply: IngressReply) -> WsFrame {
    let textual = reply.content_type.as_deref().is_some_and(|kind| {
        let kind = kind.trim().to_ascii_lowercase();
        kind.starts_with("text/") || kind.starts_with("application/json")
    });
    if textual {
        match String::from_utf8(reply.body) {
            Ok(text) => WsFrame::Text(text),
            Err(error) => WsFrame::Binary(error.into_bytes()),
        }
    } else {
        WsFrame::Binary(reply.body)
    }
}

/// Jittered exponential backoff in `[min, max]`; `attempt` starts at 1.
fn backoff_ms(attempt: u32, min_ms: u64, max_ms: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let max_ms = max_ms.max(min_ms);
    let ceiling = min_ms.saturating_mul(1u64 << attempt.min(16)).min(max_ms);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(attempt);
    let span = ceiling.saturating_sub(min_ms);
    min_ms + if span == 0 { 0 } else { hasher.finish() % (span + 1) }
}

/// Standard base64 (RFC 4648) for HTTP basic auth.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let triple = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((triple >> (18 - 6 * index)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The header a credential adds to an HTTP request or websocket handshake.
fn auth_header(scheme: AuthScheme, credential: &Credential, header: Option<&str>) -> Option<(String, String)> {
    match scheme {
        AuthScheme::Bearer => Some(("authorization".into(), format!("Bearer {}", credential.secret))),
        AuthScheme::Basic => {
            let username = credential.username.as_deref()?;
            Some(("authorization".into(), format!("Basic {}", base64(format!("{username}:{}", credential.secret).as_bytes()))))
        }
        AuthScheme::Header => Some((header?.to_ascii_lowercase(), credential.secret.clone())),
        AuthScheme::MqttPassword => None,
    }
}
