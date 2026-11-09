//! Websocket dial-out task: connect, deliver frames, send the guest's frames, redial with jittered backoff.

use std::time::Duration;

use fe_sdk::manifest_v2::{AuthSlot, WebsocketDialIngress};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;

use super::connect::{self, CONNECT_TIMEOUT};
use super::ingress::frame_body;
use super::{auth_header, backoff_ms, EndpointState, IngressFrame, Net, ReplyTo, ShapedCredential, Transport, WsFrame, WsOutbound};
use crate::bus::rate::TokenBucket;
use crate::bus::InstanceKey;

const DEFAULT_RECONNECT_MIN_MS: u64 = 1_000;
const DEFAULT_RECONNECT_MAX_MS: u64 = 60_000;
/// tungstenite's own limit: frames between the declared cap and this are dropped and counted; larger ones drop the connection.
const TRANSPORT_FRAME_CEILING: usize = 16 * 1024 * 1024;

/// A `websocket-dial` endpoint as the task needs it.
pub(super) struct DialConfig {
    endpoint: String,
    url: String,
    max_frame_bytes: usize,
    rate_hz: f64,
    auth: Option<AuthSlot>,
    reconnect_min_ms: u64,
    reconnect_max_ms: u64,
}

impl DialConfig {
    /// From the manifest declaration (`reconnect_ms` defaults to 1-60 s).
    pub(super) fn from_manifest(dial: &WebsocketDialIngress) -> Self {
        let (min, max) = dial
            .reconnect_ms
            .as_ref()
            .map_or((DEFAULT_RECONNECT_MIN_MS, DEFAULT_RECONNECT_MAX_MS), |reconnect| (reconnect.min, reconnect.max));
        Self {
            endpoint: dial.id.clone(),
            url: dial.url.clone(),
            max_frame_bytes: usize::try_from(dial.max_frame_bytes).unwrap_or(usize::MAX),
            rate_hz: dial.rate_hz,
            auth: dial.auth.clone(),
            reconnect_min_ms: min.max(1),
            reconnect_max_ms: max.max(min),
        }
    }
}

enum Ended {
    Stopped,
    Dropped(String),
}

/// Dial, serve, redial until the instance stops (its stop switch flips or its session is dropped).
pub(super) async fn run(
    net: Net,
    key: InstanceKey,
    generation: u64,
    config: DialConfig,
    mut outbound: mpsc::Receiver<WsOutbound>,
    mut stop: watch::Receiver<bool>,
) {
    let mut attempt = 0u32;
    loop {
        if *stop.borrow() {
            return;
        }
        match net.credential(&config.auth) {
            Err(state) => net.set_endpoint_state(&key, generation, &config.endpoint, state),
            Ok(credential) => match session(&net, &key, generation, &config, credential, &mut outbound, &mut stop, &mut attempt).await {
                Ended::Stopped => return,
                Ended::Dropped(reason) => net.set_endpoint_state(&key, generation, &config.endpoint, EndpointState::Down(reason)),
            },
        }
        attempt = attempt.saturating_add(1);
        let wait = backoff_ms(attempt, config.reconnect_min_ms, config.reconnect_max_ms);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
            _ = stop.changed() => return,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn session(
    net: &Net,
    key: &InstanceKey,
    generation: u64,
    config: &DialConfig,
    credential: ShapedCredential,
    outbound: &mut mpsc::Receiver<WsOutbound>,
    stop: &mut watch::Receiver<bool>,
    attempt: &mut u32,
) -> Ended {
    let target = match connect::target(&config.url) {
        Ok(target) => target,
        Err(error) => return Ended::Dropped(error),
    };
    let mut request = match config.url.as_str().into_client_request() {
        Ok(request) => request,
        Err(error) => return Ended::Dropped(format!("bad websocket url: {error}")),
    };
    if let Some((scheme, credential, header)) = &credential {
        let Some((name, value)) = auth_header(*scheme, credential, header.as_deref()) else {
            net.set_endpoint_state(key, generation, &config.endpoint, EndpointState::CredentialMissing);
            return Ended::Dropped("credential does not fit the auth scheme".into());
        };
        match (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(&value)) {
            (Ok(name), Ok(mut value)) => {
                value.set_sensitive(true);
                request.headers_mut().insert(name, value);
            }
            _ => return Ended::Dropped("credential is not a valid header".into()),
        }
    }
    let stream = match connect::connect(&target).await {
        Ok(stream) => stream,
        Err(error) => return Ended::Dropped(error),
    };
    let limit = config.max_frame_bytes.saturating_mul(4).clamp(64 * 1024, TRANSPORT_FRAME_CEILING);
    let ws_config = WebSocketConfig::default().max_message_size(Some(limit)).max_frame_size(Some(limit));
    let handshake = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::client_async_with_config(request, stream, Some(ws_config))).await;
    let mut socket = match handshake {
        Ok(Ok((socket, _response))) => socket,
        Ok(Err(error)) => return Ended::Dropped(format!("websocket handshake failed: {error}")),
        Err(_) => return Ended::Dropped("websocket handshake timed out".into()),
    };
    let Some(connection) = net.dial_opened(key, generation, &config.endpoint) else {
        let _ = socket.close(None).await;
        return Ended::Stopped;
    };
    *attempt = 0;
    let mut bucket = TokenBucket::new(config.rate_hz, net.now());
    let ended = loop {
        tokio::select! {
            _ = stop.changed() => {
                let _ = socket.close(None).await;
                break Ended::Stopped;
            }
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => deliver(net, key, generation, config, connection, WsFrame::Text(text.as_str().to_string()), &mut bucket),
                Some(Ok(Message::Binary(bytes))) => deliver(net, key, generation, config, connection, WsFrame::Binary(bytes.to_vec()), &mut bucket),
                Some(Ok(Message::Close(_))) | None => break Ended::Dropped("closed by the server".into()),
                Some(Ok(_)) => {}
                Some(Err(error)) => break Ended::Dropped(error.to_string()),
            },
            outgoing = outbound.recv() => match outgoing {
                Some(WsOutbound::Frame(frame)) => {
                    let message = match frame {
                        WsFrame::Text(text) => Message::text(text),
                        WsFrame::Binary(bytes) => Message::binary(bytes),
                    };
                    if let Err(error) = socket.send(message).await {
                        break Ended::Dropped(error.to_string());
                    }
                }
                Some(WsOutbound::Close { code, reason }) => {
                    let _ = socket.close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() })).await;
                    break Ended::Dropped("closed by the extension".into());
                }
                None => {
                    let _ = socket.close(None).await;
                    break Ended::Stopped;
                }
            },
        }
    };
    net.dial_closed(key, generation, &config.endpoint, connection);
    ended
}

/// Size and rate checks, then the frame goes to the guest's inbox; replies return on this connection.
fn deliver(net: &Net, key: &InstanceKey, generation: u64, config: &DialConfig, connection: u64, frame: WsFrame, bucket: &mut TokenBucket) {
    if frame.len() > config.max_frame_bytes {
        net.count(key, generation, |counters| counters.ingress_dropped_size += 1);
        return;
    }
    if !bucket.try_take(net.now()) {
        net.count(key, generation, |counters| counters.ingress_dropped_rate += 1);
        return;
    }
    let (content_type, body) = frame_body(frame);
    let frame = IngressFrame {
        endpoint: config.endpoint.clone(),
        transport: Transport::Websocket,
        connection: Some(connection),
        caller_did: None,
        method: None,
        path: None,
        query: Vec::new(),
        headers: Vec::new(),
        topic: None,
        content_type,
        body,
        received_at_ms: net.now(),
    };
    net.deliver_frame(key, generation, frame, ReplyTo::Dialed(config.endpoint.clone()));
}
