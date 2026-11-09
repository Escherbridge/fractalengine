//! The broker seam behind `bus`, `egress` and `ingress` (E5 implements it); see
//! src/wasm/AGENTS.md §broker-seam. The guest never holds I/O: every call is queued host-side.

use super::bindings::fractalengine::plugin::bus::{CallId, SubscriptionId};
use super::bindings::fractalengine::plugin::egress::{MqttQos, OutgoingRequest, RequestId};
use super::bindings::fractalengine::plugin::types::{Binding, ConnectionId, HostError, Json, WsFrame};
use super::bindings::fractalengine::plugin::{bus, egress, ingress};
use super::state::{parse_json_arg, InstanceState};

fn not_available(surface: &str) -> HostError {
    HostError::NotAvailable(format!("{surface} has no broker on this host"))
}

/// Host-side broker for the network and bus imports; `binding` names the calling instance.
///
/// Every method has a `not-available` default so a host without the E5 broker still links the
/// world. Implementations must not block the plugin worker (spec R15): queue and complete later
/// through the instance's `on_*` hook wrappers.
pub trait ExtensionBroker: Send + Sync {
    /// `bus.subscribe`: `source` is a declared `bus.consume` address, `params` the per-source JSON.
    fn bus_subscribe(&self, binding: &Binding, source: &str, params: &str) -> Result<SubscriptionId, HostError> {
        let _ = (binding, source, params);
        Err(not_available("bus"))
    }
    /// `bus.unsubscribe`; a foreign or stale handle is ignored.
    fn bus_unsubscribe(&self, binding: &Binding, subscription: SubscriptionId) {
        let _ = (binding, subscription);
    }
    /// `bus.refresh` (host/query only).
    fn bus_refresh(&self, binding: &Binding, subscription: SubscriptionId) -> Result<(), HostError> {
        let _ = (binding, subscription);
        Err(not_available("bus"))
    }
    /// `bus.publish` on an exposed stream (`stream` is the id, not the full address).
    fn bus_publish(&self, binding: &Binding, stream: &str, event: &str) -> Result<(), HostError> {
        let _ = (binding, stream, event);
        Err(not_available("bus"))
    }
    /// `bus.call` of `<package_id>/<command>`; the result arrives through `on-call-result`.
    fn bus_call(&self, binding: &Binding, target: &str, args: &str) -> Result<CallId, HostError> {
        let _ = (binding, target, args);
        Err(not_available("bus"))
    }
    /// `egress.http-request` to a declared endpoint; the result arrives through `on-egress-result`.
    fn egress_http(&self, binding: &Binding, endpoint: &str, request: OutgoingRequest) -> Result<RequestId, HostError> {
        let _ = (binding, endpoint, request);
        Err(not_available("egress"))
    }
    /// `egress.mqtt-publish` to a declared broker topic.
    fn egress_mqtt(
        &self,
        binding: &Binding,
        endpoint: &str,
        topic: &str,
        payload: Vec<u8>,
        qos: MqttQos,
        retain: bool,
    ) -> Result<(), HostError> {
        let _ = (binding, endpoint, topic, payload, qos, retain);
        Err(not_available("egress"))
    }
    /// `ingress.send` on a declared websocket endpoint.
    fn ingress_send(
        &self,
        binding: &Binding,
        endpoint: &str,
        connection: Option<ConnectionId>,
        frame: WsFrame,
    ) -> Result<(), HostError> {
        let _ = (binding, endpoint, connection, frame);
        Err(not_available("ingress"))
    }
    /// `ingress.close` on a declared websocket endpoint.
    fn ingress_close(
        &self,
        binding: &Binding,
        endpoint: &str,
        connection: Option<ConnectionId>,
        code: u16,
        reason: &str,
    ) -> Result<(), HostError> {
        let _ = (binding, endpoint, connection, code, reason);
        Err(not_available("ingress"))
    }
}

/// The default broker: every bus, egress and ingress call answers `not-available`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoBroker;

impl ExtensionBroker for NoBroker {}

impl bus::Host for InstanceState {
    fn subscribe(&mut self, source: String, params: Json) -> Result<SubscriptionId, HostError> {
        self.lease_active()?;
        parse_json_arg(&params)?;
        self.broker.bus_subscribe(&self.binding, &source, &params)
    }

    fn unsubscribe(&mut self, subscription: SubscriptionId) {
        self.broker.bus_unsubscribe(&self.binding, subscription);
    }

    fn refresh(&mut self, subscription: SubscriptionId) -> Result<(), HostError> {
        self.lease_active()?;
        self.broker.bus_refresh(&self.binding, subscription)
    }

    fn publish(&mut self, name: String, event: Json) -> Result<(), HostError> {
        self.lease_active()?;
        parse_json_arg(&event)?;
        self.broker.bus_publish(&self.binding, &name, &event)
    }

    fn call(&mut self, target: String, args: Json) -> Result<CallId, HostError> {
        self.lease_active()?;
        parse_json_arg(&args)?;
        self.broker.bus_call(&self.binding, &target, &args)
    }
}

impl egress::Host for InstanceState {
    fn http_request(&mut self, endpoint: String, request: OutgoingRequest) -> Result<RequestId, HostError> {
        self.lease_active()?;
        self.broker.egress_http(&self.binding, &endpoint, request)
    }

    fn mqtt_publish(
        &mut self,
        endpoint: String,
        topic: String,
        payload: Vec<u8>,
        qos: MqttQos,
        retain: bool,
    ) -> Result<(), HostError> {
        self.lease_active()?;
        self.broker
            .egress_mqtt(&self.binding, &endpoint, &topic, payload, qos, retain)
    }
}

impl ingress::Host for InstanceState {
    fn send(&mut self, endpoint: String, connection: Option<ConnectionId>, frame: WsFrame) -> Result<(), HostError> {
        self.lease_active()?;
        self.broker.ingress_send(&self.binding, &endpoint, connection, frame)
    }

    fn close(
        &mut self,
        endpoint: String,
        connection: Option<ConnectionId>,
        code: u16,
        reason: String,
    ) -> Result<(), HostError> {
        self.lease_active()?;
        self.broker
            .ingress_close(&self.binding, &endpoint, connection, code, &reason)
    }
}
