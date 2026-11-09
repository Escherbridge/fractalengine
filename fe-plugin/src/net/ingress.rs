//! Ingress (spec R8): the HTTP/WS accept mount fe-api routes into, the guest's `ingress` imports, dial-out frames.

use fe_sdk::manifest_v2::{Ingress, Method, RoleFloor};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use super::{
    is_credential_header, AcceptedConnection, ConnectionEvent, Delivery, EndpointState, IngressFrame, IngressReply,
    Net, NetState, ReplyTo, Transport, WsFrame, WsOutbound, OUTBOUND_QUEUE,
};
use crate::bus::rate::TokenBucket;
use crate::bus::{role_level, HostError, InstanceKey};

/// Close reasons are cut to fit a websocket close frame.
const MAX_CLOSE_REASON_BYTES: usize = 123;

/// One request on `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{endpoint}[/{sub_path}]`, after bearer auth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpIngressRequest {
    pub petal_id: String,
    pub package_id: String,
    pub endpoint: String,
    /// The bearer token's DID.
    pub caller_did: String,
    pub method: String,
    /// The path after the mount, no leading slash ("" at the mount).
    pub sub_path: String,
    pub query: Vec<(String, String)>,
    /// Every request header; only declared, non-credential ones reach the guest.
    pub headers: Vec<(String, String)>,
    pub content_type: Option<String>,
    /// At most [`super::INGRESS_BODY_CEILING`] (fe-api's extractor limit).
    pub body: Vec<u8>,
}

/// Why the mount refused; `NotFound` is fe-api's uniform not-found (undeclared, disabled, or below the floor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressRefusal {
    NotFound,
    TooManyRequests,
    PayloadTooLarge,
    MethodNotAllowed,
    UnsupportedMediaType,
}

impl IngressRefusal {
    /// The HTTP status fe-api answers.
    pub fn http_status(self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::TooManyRequests => 429,
            Self::PayloadTooLarge => 413,
            Self::MethodNotAllowed => 405,
            Self::UnsupportedMediaType => 415,
        }
    }
}

/// A websocket upgrade on an accept-mode mount, after bearer auth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsAcceptRequest {
    pub petal_id: String,
    pub package_id: String,
    pub endpoint: String,
    pub caller_did: String,
}

/// An accepted connection: pass inbound frames to `websocket_frame`, write what `outbound` yields, then report the close.
#[derive(Debug)]
pub struct WsSession {
    pub connection: u64,
    pub max_frame_bytes: usize,
    pub outbound: mpsc::Receiver<WsOutbound>,
}

/// A refusal plus, for role-floor refusals, the installation to audit.
type Refused = (IngressRefusal, Option<String>);

/// The unaudited uniform not-found.
const NOT_FOUND: Refused = (IngressRefusal::NotFound, None);

impl Net {
    /// Admit one HTTP ingress request (R8 table); the receiver resolves when `on-ingress` returns (`None` = 202).
    pub fn accept_http(&self, request: HttpIngressRequest) -> Result<oneshot::Receiver<Option<IngressReply>>, IngressRefusal> {
        let target = format!("{}/ingress/{}", request.package_id, request.endpoint);
        let caller = request.caller_did.clone();
        self.admit_http(request).map_err(|(refusal, audited)| {
            if let Some(installation) = audited {
                self.audit(self.denial(&caller, Some(installation), target, "not-found"));
            }
            refusal
        })
    }

    fn admit_http(&self, request: HttpIngressRequest) -> Result<oneshot::Receiver<Option<IngressReply>>, Refused> {
        let now = self.now();
        let key = InstanceKey::new(request.petal_id.clone(), request.package_id.clone());
        let mut state = self.lock();
        let instance = state.instances.get_mut(&key).ok_or(NOT_FOUND)?;
        let Some(Ingress::HttpServer(endpoint)) = instance.ingress(&request.endpoint).cloned() else {
            return Err((IngressRefusal::NotFound, None));
        };
        let floor = role_level(endpoint.role_floor.unwrap_or(RoleFloor::Editor));
        if !self.sources.roles.role_at(&request.caller_did, &request.petal_id).is_at_least(floor) {
            instance.counters.ingress_refused += 1;
            return Err((IngressRefusal::NotFound, Some(instance.spec.installation_id.clone())));
        }
        if !request.sub_path.is_empty() && !endpoint.sub_paths.unwrap_or(false) {
            return Err((IngressRefusal::NotFound, None));
        }
        let allowed = instance
            .http_callers
            .entry((endpoint.id.clone(), request.caller_did.clone()))
            .or_insert_with(|| TokenBucket::new(endpoint.rate_hz, now))
            .try_take(now);
        if !allowed {
            instance.counters.ingress_dropped_rate += 1;
            return Err((IngressRefusal::TooManyRequests, None));
        }
        if !parse_method(&request.method).is_some_and(|method| endpoint.methods.contains(&method)) {
            return Err((IngressRefusal::MethodNotAllowed, None));
        }
        if request.body.len() as u64 > endpoint.max_body_bytes {
            instance.counters.ingress_dropped_size += 1;
            return Err((IngressRefusal::PayloadTooLarge, None));
        }
        if let Some(accepted) = &endpoint.content_types {
            let media = media_type(request.content_type.as_deref());
            if !accepted.iter().any(|kind| kind.eq_ignore_ascii_case(&media)) {
                return Err((IngressRefusal::UnsupportedMediaType, None));
            }
        }
        let frame = IngressFrame {
            endpoint: endpoint.id.clone(),
            transport: Transport::Http,
            connection: None,
            caller_did: Some(request.caller_did),
            method: Some(request.method.to_ascii_uppercase()),
            path: Some(request.sub_path),
            query: request.query,
            headers: forwarded_headers(&request.headers, endpoint.forward_headers.as_deref().unwrap_or(&[])),
            topic: None,
            content_type: request.content_type,
            body: request.body,
            received_at_ms: now,
        };
        let (reply, receiver) = oneshot::channel();
        if !instance.push(Delivery::Ingress { frame: Box::new(frame), reply: ReplyTo::Http(reply) }) {
            return Err((IngressRefusal::TooManyRequests, None));
        }
        instance.counters.ingress_frames += 1;
        Ok(receiver)
    }

    /// Accept a websocket upgrade on an accept-mode mount; the guest hears `opened`.
    pub fn open_websocket(&self, request: WsAcceptRequest) -> Result<WsSession, IngressRefusal> {
        let now = self.now();
        let key = InstanceKey::new(request.petal_id.clone(), request.package_id.clone());
        let mut audited = None;
        let result = {
            let mut state = self.lock();
            let NetState { instances, connections, next_id, .. } = &mut *state;
            match instances.get_mut(&key) {
                None => Err(IngressRefusal::NotFound),
                Some(instance) => match instance.ingress(&request.endpoint).cloned() {
                    Some(Ingress::WebsocketAccept(endpoint)) => {
                        let floor = role_level(endpoint.role_floor.unwrap_or(RoleFloor::Editor));
                        let open = connections.values().filter(|open| open.key == key && open.endpoint == endpoint.id).count();
                        if !self.sources.roles.role_at(&request.caller_did, &request.petal_id).is_at_least(floor) {
                            instance.counters.ingress_refused += 1;
                            audited = Some(instance.spec.installation_id.clone());
                            Err(IngressRefusal::NotFound)
                        } else if open >= endpoint.max_connections as usize {
                            Err(IngressRefusal::TooManyRequests)
                        } else {
                            let id = *next_id;
                            *next_id += 1;
                            let max_frame_bytes = usize::try_from(endpoint.max_frame_bytes).unwrap_or(usize::MAX);
                            let (outbound, receiver) = mpsc::channel(OUTBOUND_QUEUE);
                            connections.insert(
                                id,
                                AcceptedConnection {
                                    key: key.clone(),
                                    endpoint: endpoint.id.clone(),
                                    caller_did: request.caller_did.clone(),
                                    max_frame_bytes,
                                    bucket: TokenBucket::new(endpoint.rate_hz, now),
                                    outbound,
                                },
                            );
                            instance.push(Delivery::Connection { endpoint: endpoint.id.clone(), connection: id, event: ConnectionEvent::Opened });
                            Ok(WsSession { connection: id, max_frame_bytes, outbound: receiver })
                        }
                    }
                    _ => Err(IngressRefusal::NotFound),
                },
            }
        };
        if let Some(installation) = audited {
            let target = format!("{}/ingress/{}", request.package_id, request.endpoint);
            self.audit(self.denial(&request.caller_did, Some(installation), target, "not-found"));
        }
        result
    }

    /// One frame from an accepted connection; over-size, over-rate or queue-full frames are dropped and counted.
    pub fn websocket_frame(&self, connection: u64, frame: WsFrame) {
        let now = self.now();
        let mut state = self.lock();
        let NetState { instances, connections, .. } = &mut *state;
        let Some(open) = connections.get_mut(&connection) else { return };
        let Some(instance) = instances.get_mut(&open.key) else { return };
        if frame.len() > open.max_frame_bytes {
            instance.counters.ingress_dropped_size += 1;
            return;
        }
        if !open.bucket.try_take(now) {
            instance.counters.ingress_dropped_rate += 1;
            return;
        }
        let (content_type, body) = frame_body(frame);
        let frame = IngressFrame {
            endpoint: open.endpoint.clone(),
            transport: Transport::Websocket,
            connection: Some(connection),
            caller_did: Some(open.caller_did.clone()),
            method: None,
            path: None,
            query: Vec::new(),
            headers: Vec::new(),
            topic: None,
            content_type,
            body,
            received_at_ms: now,
        };
        if instance.push(Delivery::Ingress { frame: Box::new(frame), reply: ReplyTo::Connection(connection) }) {
            instance.counters.ingress_frames += 1;
        }
    }

    /// The client closed or the socket failed; the guest hears `closed`.
    pub fn websocket_closed(&self, connection: u64) {
        let mut state = self.lock();
        let Some(open) = state.connections.remove(&connection) else { return };
        if let Some(instance) = state.instances.get_mut(&open.key) {
            instance.push(Delivery::Connection { endpoint: open.endpoint, connection, event: ConnectionEvent::Closed });
        }
    }

    /// WIT `ingress.send`: accept mode names one of the endpoint's connections; dial mode passes none.
    pub fn send(&self, key: &InstanceKey, endpoint: &str, connection: Option<u64>, frame: WsFrame) -> Result<(), HostError> {
        let outbound = self.outbound_for(key, endpoint, connection, Some(frame.len()))?;
        outbound.try_send(WsOutbound::Frame(frame)).map_err(queue_error)
    }

    /// WIT `ingress.close`: close an accepted connection, or the dialled one (the host redials with backoff).
    pub fn close(&self, key: &InstanceKey, endpoint: &str, connection: Option<u64>, code: u16, reason: &str) -> Result<(), HostError> {
        let outbound = self.outbound_for(key, endpoint, connection, None)?;
        let mut cut = reason.len().min(MAX_CLOSE_REASON_BYTES);
        while !reason.is_char_boundary(cut) {
            cut -= 1;
        }
        outbound.try_send(WsOutbound::Close { code, reason: reason[..cut].to_string() }).map_err(queue_error)
    }

    fn outbound_for(&self, key: &InstanceKey, endpoint: &str, connection: Option<u64>, frame_len: Option<usize>) -> Result<mpsc::Sender<WsOutbound>, HostError> {
        let state = self.lock();
        let instance = state.instances.get(key).ok_or_else(|| HostError::Revoked("instance is not active".into()))?;
        let too_big = |max: u64| frame_len.is_some_and(|len| len as u64 > max);
        match instance.ingress(endpoint) {
            Some(Ingress::WebsocketAccept(accept)) => {
                if too_big(accept.max_frame_bytes) {
                    return Err(HostError::InvalidInput(format!("frame is over max_frame_bytes ({})", accept.max_frame_bytes)));
                }
                let id = connection.ok_or_else(|| HostError::InvalidInput("accept mode names a connection".into()))?;
                state
                    .connections
                    .get(&id)
                    .filter(|open| &open.key == key && open.endpoint == endpoint)
                    .map(|open| open.outbound.clone())
                    .ok_or_else(|| HostError::InvalidInput(format!("connection {id} is not open on {endpoint}")))
            }
            Some(Ingress::WebsocketDial(dial)) => {
                if too_big(dial.max_frame_bytes) {
                    return Err(HostError::InvalidInput(format!("frame is over max_frame_bytes ({})", dial.max_frame_bytes)));
                }
                let current = instance.dial_connections.get(endpoint).copied();
                if current.is_none() {
                    return Err(HostError::NotAvailable(format!("{endpoint} is not connected")));
                }
                if connection.is_some_and(|id| Some(id) != current) {
                    return Err(HostError::InvalidInput("that dialled connection has closed".into()));
                }
                instance
                    .dialers
                    .get(endpoint)
                    .and_then(|session| session.outbound.clone())
                    .ok_or_else(|| HostError::NotAvailable(format!("{endpoint} is not connected")))
            }
            _ => Err(HostError::CapabilityDenied(format!("{endpoint} is not a declared websocket endpoint"))),
        }
    }

    // ---- dial-out plumbing (ws_dial, mqtt_session) ----------------------------------------------

    /// A dial-out connected: a fresh connection id, state `up`, and `opened` for the guest.
    pub(super) fn dial_opened(&self, key: &InstanceKey, generation: u64, endpoint: &str) -> Option<u64> {
        self.with_generation(key, generation, |state| {
            let id = state.next_id;
            state.next_id += 1;
            let instance = state.instances.get_mut(key)?;
            instance.dial_connections.insert(endpoint.to_string(), id);
            instance.endpoints.insert(endpoint.to_string(), EndpointState::Up);
            instance.push(Delivery::Connection { endpoint: endpoint.to_string(), connection: id, event: ConnectionEvent::Opened });
            Some(id)
        })
        .flatten()
    }

    /// A dial-out dropped: `closed` for the guest; the task sets the endpoint state.
    pub(super) fn dial_closed(&self, key: &InstanceKey, generation: u64, endpoint: &str, connection: u64) {
        self.with_generation(key, generation, |state| {
            if let Some(instance) = state.instances.get_mut(key) {
                if instance.dial_connections.get(endpoint) == Some(&connection) {
                    instance.dial_connections.remove(endpoint);
                }
                instance.push(Delivery::Connection { endpoint: endpoint.to_string(), connection, event: ConnectionEvent::Closed });
            }
        });
    }

    /// A frame a dial-out task already size- and rate-checked; false when the inbox was full.
    pub(super) fn deliver_frame(&self, key: &InstanceKey, generation: u64, frame: IngressFrame, reply: ReplyTo) -> bool {
        self.with_generation(key, generation, |state| {
            let Some(instance) = state.instances.get_mut(key) else { return false };
            let queued = instance.push(Delivery::Ingress { frame: Box::new(frame), reply });
            if queued {
                instance.counters.ingress_frames += 1;
            }
            queued
        })
        .unwrap_or(false)
    }
}

/// `content_type` and body bytes of a websocket frame (WIT: text frames carry `text/plain`).
pub(super) fn frame_body(frame: WsFrame) -> (Option<String>, Vec<u8>) {
    match frame {
        WsFrame::Text(text) => (Some("text/plain".to_string()), text.into_bytes()),
        WsFrame::Binary(bytes) => (None, bytes),
    }
}

fn parse_method(method: &str) -> Option<Method> {
    match method.to_ascii_uppercase().as_str() {
        "GET" => Some(Method::Get),
        "POST" => Some(Method::Post),
        "PUT" => Some(Method::Put),
        "PATCH" => Some(Method::Patch),
        "DELETE" => Some(Method::Delete),
        _ => None,
    }
}

/// The media type without parameters, lowercase (`application/json; charset=utf-8` -> `application/json`).
fn media_type(content_type: Option<&str>) -> String {
    content_type.unwrap_or_default().split(';').next().unwrap_or_default().trim().to_ascii_lowercase()
}

/// The declared `forward_headers`, lowercase, minus every credential header whatever the manifest says.
pub(super) fn forwarded_headers(headers: &[(String, String)], declared: &[String]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.trim().to_ascii_lowercase();
            let wanted = declared.iter().any(|declared| declared.eq_ignore_ascii_case(&name));
            (wanted && !is_credential_header(&name)).then(|| (name, value.clone()))
        })
        .collect()
}

fn queue_error(error: TrySendError<WsOutbound>) -> HostError {
    match error {
        TrySendError::Full(_) => HostError::RateLimited("the connection's outbound queue is full".into()),
        TrySendError::Closed(_) => HostError::NotAvailable("the connection has closed".into()),
    }
}
