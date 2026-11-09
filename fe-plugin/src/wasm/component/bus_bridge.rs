//! Joins a 2.0 instance to the E5 integration bus (`crate::bus`): the `bus` imports go to
//! [`Bus`](crate::bus::Bus), and `Bus::dispatch` drives the guest's `bus-hooks` through
//! [`BusHooks`](crate::bus::BusHooks); see src/wasm/AGENTS.md §broker-seam.

use crate::bus::{self, Bus, BusHooks};

use super::bindings::exports::fractalengine::plugin::bus_hooks as wit_hooks;
use super::bindings::fractalengine::plugin::types::{self as wit, Binding};
use super::broker::ExtensionBroker;
use super::instance::ExtensionInstance;

/// The bus key of a binding (one package bound to one petal, OD-E3).
pub fn instance_key(binding: &Binding) -> bus::InstanceKey {
    bus::InstanceKey::new(binding.petal_id.clone(), binding.package_id.clone())
}

/// Bus refusals keep their WIT case on the way back to the guest.
pub fn wit_host_error(error: bus::HostError) -> wit::HostError {
    match error {
        bus::HostError::CapabilityDenied(detail) => wit::HostError::CapabilityDenied(detail),
        bus::HostError::PolicyDenied(detail) => wit::HostError::PolicyDenied(detail),
        bus::HostError::InvalidInput(detail) => wit::HostError::InvalidInput(detail),
        bus::HostError::BudgetExceeded(detail) => wit::HostError::BudgetExceeded(detail),
        bus::HostError::RateLimited(detail) => wit::HostError::RateLimited(detail),
        bus::HostError::Revoked(detail) => wit::HostError::Revoked(detail),
        bus::HostError::NotAvailable(detail) => wit::HostError::NotAvailable(detail),
        bus::HostError::Backend(detail) => wit::HostError::Backend(detail),
        bus::HostError::UnknownOutcome(detail) => wit::HostError::UnknownOutcome(detail),
    }
}

/// `bus` imports answered by the integration bus; egress and ingress keep the `not-available`
/// defaults until the net bridge exists.
#[derive(Clone)]
pub struct BusBroker {
    bus: Bus,
}

impl BusBroker {
    /// Broker over the host's one bus.
    pub fn new(bus: Bus) -> Self {
        Self { bus }
    }
}

impl ExtensionBroker for BusBroker {
    fn bus_subscribe(&self, binding: &Binding, source: &str, params: &str) -> Result<u64, wit::HostError> {
        self.bus
            .subscribe(&instance_key(binding), source, params)
            .map_err(wit_host_error)
    }

    fn bus_unsubscribe(&self, binding: &Binding, subscription: u64) {
        self.bus.unsubscribe(&instance_key(binding), subscription);
    }

    fn bus_refresh(&self, binding: &Binding, subscription: u64) -> Result<(), wit::HostError> {
        self.bus
            .refresh(&instance_key(binding), subscription)
            .map_err(wit_host_error)
    }

    fn bus_publish(&self, binding: &Binding, stream: &str, event: &str) -> Result<(), wit::HostError> {
        self.bus
            .publish(&instance_key(binding), stream, event)
            .map_err(wit_host_error)
    }

    fn bus_call(&self, binding: &Binding, target: &str, args: &str) -> Result<u64, wit::HostError> {
        self.bus
            .call(&instance_key(binding), target, args)
            .map_err(wit_host_error)
    }
}

fn wit_delivery_kind(kind: bus::DeliveryKind) -> wit_hooks::DeliveryKind {
    match kind {
        bus::DeliveryKind::Snapshot => wit_hooks::DeliveryKind::Snapshot,
        bus::DeliveryKind::Delta => wit_hooks::DeliveryKind::Delta,
        bus::DeliveryKind::ResyncRequired => wit_hooks::DeliveryKind::ResyncRequired,
        bus::DeliveryKind::Ended => wit_hooks::DeliveryKind::Ended,
    }
}

fn wit_caller(caller: &bus::Caller) -> wit::Caller {
    wit::Caller {
        did: caller.did.clone(),
        kind: match caller.kind {
            bus::CallerKind::Ui => wit::CallerKind::Ui,
            bus::CallerKind::Rest => wit::CallerKind::Rest,
            bus::CallerKind::Mcp => wit::CallerKind::Mcp,
            bus::CallerKind::Extension => wit::CallerKind::Extension,
            bus::CallerKind::Portal => wit::CallerKind::Portal,
        },
        package_id: caller.package_id.clone(),
        local: caller.local,
    }
}

/// A faulted hook already degraded the instance and discarded its window; the bus only logs it
/// (commands answer `failed`), and the scheduler stops the instance on its next tick.
impl BusHooks for ExtensionInstance {
    fn on_events(&mut self, events: Vec<bus::BusEvent>) {
        let events: Vec<wit_hooks::BusEvent> = events
            .into_iter()
            .map(|event| wit_hooks::BusEvent {
                subscription: event.subscription,
                source: event.source.to_string(),
                seq: event.seq,
                kind: wit_delivery_kind(event.kind),
                dropped_before: event.dropped_before,
                at_ms: event.at_ms,
                data: event.data.to_string(),
            })
            .collect();
        if let Err(fault) = ExtensionInstance::on_events(self, &events) {
            tracing::warn!(installation = %self.binding().installation_id, %fault, "bus on-events faulted");
        }
    }

    fn on_command(&mut self, command: &str, args: &str, caller: &bus::Caller) -> Result<String, bus::CommandError> {
        match ExtensionInstance::on_command(self, command, args, &wit_caller(caller)) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(wit_hooks::CommandError::InvalidInput(detail))) => Err(bus::CommandError::InvalidInput(detail)),
            Ok(Err(wit_hooks::CommandError::Refused(detail))) => Err(bus::CommandError::Refused(detail)),
            Ok(Err(wit_hooks::CommandError::Failed(detail))) => Err(bus::CommandError::Failed(detail)),
            Err(fault) => Err(bus::CommandError::Failed(fault.to_string())),
        }
    }

    fn on_call_result(&mut self, call: u64, outcome: Result<String, bus::HostError>) {
        if let Err(fault) = ExtensionInstance::on_call_result(self, call, outcome.map_err(wit_host_error)) {
            tracing::warn!(installation = %self.binding().installation_id, %fault, "bus on-call-result faulted");
        }
    }
}
