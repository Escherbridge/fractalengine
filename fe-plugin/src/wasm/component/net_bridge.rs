//! Joins a 2.0 instance to the network brokers (`crate::net`): `ingress`/`egress` imports go to
//! [`Net`], and `Net::dispatch` drives the guest's `ingress-hooks`/`egress-hooks` through
//! [`NetHooks`]; see src/net/AGENTS.md (seam E1/N3) and src/wasm/AGENTS.md §broker-seam.

use fe_sdk::manifest_v2::{is_credential_header, Method, Qos};

use super::bindings::exports::fractalengine::plugin::ingress_hooks as wit_ingress;
use super::bindings::fractalengine::plugin::bus::{CallId, SubscriptionId};
use super::bindings::fractalengine::plugin::egress as wit_egress;
use super::bindings::fractalengine::plugin::types::{self as wit, Binding, ConnectionId};
use super::broker::ExtensionBroker;
use super::bus_bridge::{instance_key, wit_host_error, BusBroker};
use super::instance::{ExtensionInstance, InstanceHandle};
use crate::net::{self, ConnectionEvent, IncomingResponse, IngressFrame, IngressReply, Net, NetHooks, Transport, WsFrame};

/// Credential-bearing headers never reach a guest, in either direction; the host adds its own.
fn guest_visible(headers: Vec<(String, String)>) -> Vec<(String, String)> {
    headers.into_iter().filter(|(name, _)| !is_credential_header(name)).collect()
}

fn method_from_wit(method: wit_egress::HttpMethod) -> Method {
    match method {
        wit_egress::HttpMethod::Get => Method::Get,
        wit_egress::HttpMethod::Post => Method::Post,
        wit_egress::HttpMethod::Put => Method::Put,
        wit_egress::HttpMethod::Patch => Method::Patch,
        wit_egress::HttpMethod::Delete => Method::Delete,
    }
}

fn qos_from_wit(qos: wit_egress::MqttQos) -> Qos {
    match qos {
        wit_egress::MqttQos::AtMostOnce => Qos::AtMostOnce,
        wit_egress::MqttQos::AtLeastOnce => Qos::AtLeastOnce,
    }
}

fn ws_frame_from_wit(frame: wit::WsFrame) -> WsFrame {
    match frame {
        wit::WsFrame::Text(text) => WsFrame::Text(text),
        wit::WsFrame::Binary(bytes) => WsFrame::Binary(bytes),
    }
}

fn transport_to_wit(transport: Transport) -> wit_ingress::Transport {
    match transport {
        Transport::Http => wit_ingress::Transport::Http,
        Transport::Websocket => wit_ingress::Transport::Websocket,
        Transport::Mqtt => wit_ingress::Transport::Mqtt,
    }
}

/// The guest's view of one inbound frame; credential headers are dropped even if forwarded.
pub(crate) fn frame_to_wit(frame: IngressFrame) -> wit_ingress::IngressFrame {
    wit_ingress::IngressFrame {
        endpoint: frame.endpoint,
        transport: transport_to_wit(frame.transport),
        connection: frame.connection,
        caller_did: frame.caller_did,
        method: frame.method,
        path: frame.path,
        query: frame.query,
        headers: guest_visible(frame.headers),
        topic: frame.topic,
        content_type: frame.content_type,
        body: frame.body,
        received_at_ms: frame.received_at_ms,
    }
}

fn reply_from_wit(reply: wit_ingress::IngressReply) -> IngressReply {
    IngressReply { status: reply.status, content_type: reply.content_type, body: reply.body }
}

/// The guest's view of an egress response; credential headers are dropped.
pub(crate) fn response_to_wit(response: IncomingResponse) -> wit_egress::IncomingResponse {
    wit_egress::IncomingResponse {
        status: response.status,
        content_type: response.content_type,
        headers: guest_visible(response.headers),
        body: response.body,
    }
}

fn event_to_wit(event: ConnectionEvent) -> wit_ingress::ConnectionEvent {
    match event {
        ConnectionEvent::Opened => wit_ingress::ConnectionEvent::Opened,
        ConnectionEvent::Closed => wit_ingress::ConnectionEvent::Closed,
    }
}

/// `ingress` and `egress` imports answered by [`Net`]; `bus` keeps the `not-available` defaults.
#[derive(Clone)]
pub struct NetBroker {
    net: Net,
}

impl NetBroker {
    /// Broker over the host's one [`Net`].
    pub fn new(net: Net) -> Self {
        Self { net }
    }
}

impl ExtensionBroker for NetBroker {
    fn egress_http(&self, binding: &Binding, endpoint: &str, request: wit_egress::OutgoingRequest) -> Result<u64, wit::HostError> {
        let request = net::OutgoingRequest {
            method: method_from_wit(request.method),
            path: request.path,
            query: request.query,
            headers: request.headers,
            content_type: request.content_type,
            body: request.body,
        };
        self.net.http_request(&instance_key(binding), endpoint, request).map_err(wit_host_error)
    }

    fn egress_mqtt(
        &self,
        binding: &Binding,
        endpoint: &str,
        topic: &str,
        payload: Vec<u8>,
        qos: wit_egress::MqttQos,
        retain: bool,
    ) -> Result<(), wit::HostError> {
        self.net
            .mqtt_publish(&instance_key(binding), endpoint, topic, payload, qos_from_wit(qos), retain)
            .map_err(wit_host_error)
    }

    fn ingress_send(&self, binding: &Binding, endpoint: &str, connection: Option<ConnectionId>, frame: wit::WsFrame) -> Result<(), wit::HostError> {
        self.net
            .send(&instance_key(binding), endpoint, connection, ws_frame_from_wit(frame))
            .map_err(wit_host_error)
    }

    fn ingress_close(&self, binding: &Binding, endpoint: &str, connection: Option<ConnectionId>, code: u16, reason: &str) -> Result<(), wit::HostError> {
        self.net
            .close(&instance_key(binding), endpoint, connection, code, reason)
            .map_err(wit_host_error)
    }
}

/// The one broker a host installs: `bus` through [`BusBroker`], `ingress`/`egress` through [`NetBroker`].
#[derive(Clone)]
pub struct HostBroker {
    bus: BusBroker,
    net: NetBroker,
}

impl HostBroker {
    /// Broker over the host's bus and network brokers.
    pub fn new(bus: BusBroker, net: NetBroker) -> Self {
        Self { bus, net }
    }
}

impl ExtensionBroker for HostBroker {
    fn bus_subscribe(&self, binding: &Binding, source: &str, params: &str) -> Result<SubscriptionId, wit::HostError> {
        self.bus.bus_subscribe(binding, source, params)
    }

    fn bus_unsubscribe(&self, binding: &Binding, subscription: SubscriptionId) {
        self.bus.bus_unsubscribe(binding, subscription);
    }

    fn bus_refresh(&self, binding: &Binding, subscription: SubscriptionId) -> Result<(), wit::HostError> {
        self.bus.bus_refresh(binding, subscription)
    }

    fn bus_publish(&self, binding: &Binding, stream: &str, event: &str) -> Result<(), wit::HostError> {
        self.bus.bus_publish(binding, stream, event)
    }

    fn bus_call(&self, binding: &Binding, target: &str, args: &str) -> Result<CallId, wit::HostError> {
        self.bus.bus_call(binding, target, args)
    }

    fn egress_http(&self, binding: &Binding, endpoint: &str, request: wit_egress::OutgoingRequest) -> Result<u64, wit::HostError> {
        self.net.egress_http(binding, endpoint, request)
    }

    fn egress_mqtt(
        &self,
        binding: &Binding,
        endpoint: &str,
        topic: &str,
        payload: Vec<u8>,
        qos: wit_egress::MqttQos,
        retain: bool,
    ) -> Result<(), wit::HostError> {
        self.net.egress_mqtt(binding, endpoint, topic, payload, qos, retain)
    }

    fn ingress_send(&self, binding: &Binding, endpoint: &str, connection: Option<ConnectionId>, frame: wit::WsFrame) -> Result<(), wit::HostError> {
        self.net.ingress_send(binding, endpoint, connection, frame)
    }

    fn ingress_close(&self, binding: &Binding, endpoint: &str, connection: Option<ConnectionId>, code: u16, reason: &str) -> Result<(), wit::HostError> {
        self.net.ingress_close(binding, endpoint, connection, code, reason)
    }
}

/// A faulted hook already degraded the instance; an ingress call then answers `None` (fe-api: 204, no body)
/// and the scheduler stops the instance on its next tick.
impl NetHooks for ExtensionInstance {
    fn on_ingress(&mut self, frame: IngressFrame) -> Option<IngressReply> {
        let frame = frame_to_wit(frame);
        match ExtensionInstance::on_ingress(self, &frame) {
            Ok(reply) => reply.map(reply_from_wit),
            Err(fault) => {
                tracing::warn!(installation = %self.binding().installation_id, %fault, "on-ingress faulted");
                None
            }
        }
    }

    fn on_connection(&mut self, endpoint: &str, connection: u64, event: ConnectionEvent) {
        if let Err(fault) = ExtensionInstance::on_connection(self, endpoint, connection, event_to_wit(event)) {
            tracing::warn!(installation = %self.binding().installation_id, %fault, "on-connection faulted");
        }
    }

    fn on_egress_result(&mut self, request: u64, outcome: Result<IncomingResponse, crate::bus::HostError>) {
        let outcome = outcome.map(response_to_wit).map_err(wit_host_error);
        if let Err(fault) = ExtensionInstance::on_egress_result(self, request, outcome) {
            tracing::warn!(installation = %self.binding().installation_id, %fault, "on-egress-result faulted");
        }
    }
}

/// The handle the scheduler ticks; a poisoned lock drops the delivery (an HTTP caller sees 204).
impl NetHooks for InstanceHandle {
    fn on_ingress(&mut self, frame: IngressFrame) -> Option<IngressReply> {
        match self.lock() {
            Ok(mut instance) => NetHooks::on_ingress(&mut *instance, frame),
            Err(error) => {
                tracing::warn!(%error, "on-ingress dropped");
                None
            }
        }
    }

    fn on_connection(&mut self, endpoint: &str, connection: u64, event: ConnectionEvent) {
        match self.lock() {
            Ok(mut instance) => NetHooks::on_connection(&mut *instance, endpoint, connection, event),
            Err(error) => tracing::warn!(%error, "on-connection dropped"),
        }
    }

    fn on_egress_result(&mut self, request: u64, outcome: Result<IncomingResponse, crate::bus::HostError>) {
        match self.lock() {
            Ok(mut instance) => NetHooks::on_egress_result(&mut *instance, request, outcome),
            Err(error) => tracing::warn!(%error, "on-egress-result dropped"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No credential header reaches the guest through an inbound frame or an egress response.
    #[test]
    fn credential_headers_never_reach_the_guest() {
        let secret = |name: &str| (name.to_string(), "s3cret".to_string());
        let mixed = vec![secret("Authorization"), ("x-trace".into(), "1".into()), secret("cookie"), secret("x-fe-token")];

        let frame = IngressFrame {
            endpoint: "hook".into(),
            transport: Transport::Http,
            connection: None,
            caller_did: Some("did:key:caller".into()),
            method: Some("POST".into()),
            path: Some(String::new()),
            query: Vec::new(),
            headers: mixed.clone(),
            topic: None,
            content_type: None,
            body: Vec::new(),
            received_at_ms: 1,
        };
        let response = IncomingResponse { status: 200, content_type: None, headers: mixed, body: Vec::new() };

        let expected = vec![("x-trace".to_string(), "1".to_string())];
        assert_eq!(frame_to_wit(frame).headers, expected);
        assert_eq!(response_to_wit(response).headers, expected);
    }
}
