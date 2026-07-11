//! IoT bridge vertical slice: push device readings into a node, pull node state to
//! drive a device. See AGENTS.md for the property-key data contract and how to run it.

pub mod adapter;
pub mod capability;
pub mod device;

use rhai::{Dynamic, Map, AST};

use adapter::{HostState, IotHostAdapter, TickError};
use capability::Capabilities;
use device::{MockThermostat, Reading};
use fe_sdk::property::PropertyValue;

/// Compiled Rhai source for the extension's ingest/actuate/tick logic.
pub const SCRIPT_SOURCE: &str = include_str!("../scripts/iot_bridge.rhai");

/// Outcome of a single [`BridgeLoop::run_tick`] call.
#[derive(Debug, Clone)]
pub enum TickOutcome {
    /// The reading was ingested and a device command was computed and applied.
    Applied { tick: u64, command: Map },
    /// The tick was skipped (bad data); the loop continues on the next call.
    Skipped { tick: u64, reason: String },
}

/// A hard, fail-closed bridge fault — currently only raised on a missing capability.
#[derive(Debug, Clone)]
pub struct BridgeError(pub String);

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BridgeError {}

/// Wires a [`MockThermostat`] to the `iot_bridge.rhai` script via [`IotHostAdapter`].
pub struct BridgeLoop {
    node_id: String,
    device: MockThermostat,
    adapter: IotHostAdapter,
    ast: AST,
    tick_count: u64,
    log: Vec<TickOutcome>,
}

impl BridgeLoop {
    /// Build a bridge bound to `node_id`, granted `caps`. Panics only on a malformed
    /// script (a build-time programmer error, never a runtime data fault).
    pub fn new(node_id: impl Into<String>, caps: Capabilities) -> Self {
        let adapter = IotHostAdapter::new(caps);
        let ast = adapter.compile(SCRIPT_SOURCE).expect("scripts/iot_bridge.rhai must compile");
        Self {
            node_id: node_id.into(),
            device: MockThermostat::new(20.0),
            adapter,
            ast,
            tick_count: 0,
            log: Vec::new(),
        }
    }

    /// Run one ingest+actuate cycle: device reading -> node -> device command.
    ///
    /// Never returns `Err` for bad sensor data — that tick is skipped and logged, and
    /// the loop remains usable on the next call. Only returns `Err` when a host
    /// capability is denied (fail-closed; a real authorization fault, not a data fault).
    pub fn run_tick(&mut self) -> Result<TickOutcome, BridgeError> {
        self.tick_count += 1;
        self.device.step();
        let reading = self.device.reading(self.tick_count);

        if let Err(reason) = reading.validate() {
            return Ok(self.record(TickOutcome::Skipped { tick: self.tick_count, reason }));
        }

        match self.adapter.call_tick(&self.ast, &self.node_id, reading_to_map(&reading)) {
            Ok(command) => {
                self.apply_command(&command);
                Ok(self.record(TickOutcome::Applied { tick: self.tick_count, command }))
            }
            Err(TickError::CapabilityDenied { capability, operation }) => {
                Err(BridgeError(format!("capability '{capability}' denied for operation '{operation}'")))
            }
            Err(TickError::ScriptFault(reason)) => {
                Ok(self.record(TickOutcome::Skipped { tick: self.tick_count, reason }))
            }
        }
    }

    fn record(&mut self, outcome: TickOutcome) -> TickOutcome {
        self.log.push(outcome.clone());
        outcome
    }

    fn apply_command(&mut self, cmd: &Map) {
        let setpoint = cmd.get("setpoint").and_then(|d| d.as_float().ok());
        let power = cmd.get("power").and_then(|d| d.as_bool().ok());
        self.device.apply_command(setpoint, power);
    }

    /// Directly set a node property, bypassing the script — simulates an external actor
    /// (e.g. an operator UI or another extension) driving the bound node's desired state.
    pub fn seed_node_property(&self, key: &str, value: PropertyValue) {
        self.adapter.seed_property(&self.node_id, key, value);
    }

    pub fn device(&self) -> &MockThermostat {
        &self.device
    }

    /// Mutable device access — used by tests to inject a corrupted reading.
    pub fn device_mut(&mut self) -> &mut MockThermostat {
        &mut self.device
    }

    pub fn host_state(&self) -> HostState {
        self.adapter.state()
    }

    pub fn log(&self) -> &[TickOutcome] {
        &self.log
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }
}

fn reading_to_map(reading: &Reading) -> Map {
    let mut map = Map::new();
    map.insert("temperature".into(), Dynamic::from(reading.temperature));
    map.insert("humidity".into(), Dynamic::from(reading.humidity));
    map.insert("timestamp".into(), Dynamic::from(reading.timestamp as i64));
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_writes_node_properties_from_device_reading() {
        let mut bridge = BridgeLoop::new("device_01", Capabilities::full());
        bridge.run_tick().unwrap();

        let props = bridge.host_state().properties_for("device_01");
        assert!(props.contains_key("iot.temperature"));
        assert!(props.contains_key("iot.humidity"));
        assert!(props.contains_key("iot.last_seen"));
    }

    #[test]
    fn seeded_setpoint_drives_device() {
        let mut bridge = BridgeLoop::new("device_01", Capabilities::full());
        bridge.seed_node_property("iot.setpoint", PropertyValue::Number(30.0));
        bridge.seed_node_property("iot.power", PropertyValue::Bool(true));

        bridge.run_tick().unwrap();

        assert_eq!(bridge.device().setpoint, 30.0);
        assert!(bridge.device().power);
    }

    #[test]
    fn missing_capability_fails_closed() {
        let mut bridge = BridgeLoop::new("device_01", Capabilities::empty());
        assert!(bridge.run_tick().is_err());
        assert!(bridge.host_state().properties_for("device_01").is_empty());
    }

    #[test]
    fn bad_reading_is_skipped_and_loop_continues() {
        let mut bridge = BridgeLoop::new("device_01", Capabilities::full());
        bridge.device_mut().temperature = f64::NAN;

        let outcome = bridge.run_tick().unwrap();
        assert!(matches!(outcome, TickOutcome::Skipped { .. }));

        bridge.device_mut().temperature = 21.0;
        let outcome = bridge.run_tick().unwrap();
        assert!(matches!(outcome, TickOutcome::Applied { .. }));
    }
}
