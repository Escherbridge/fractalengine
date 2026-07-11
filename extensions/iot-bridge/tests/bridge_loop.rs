//! Integration test for the IoT bridge vertical slice.
//!
//! Runs several ticks of a mock thermostat through `iot_bridge.rhai` via [`BridgeLoop`]
//! and checks the full round trip: device -> node properties, and node desired-state ->
//! device commands, plus the fail-closed and resilience guarantees. The final test
//! drives the same script through fe-plugin-test's shared `RhaiTestRunner` surface —
//! see AGENTS.md "Integration".

use fe_sdk::property::PropertyValue;
use iot_bridge::capability::Capabilities;
use iot_bridge::{BridgeLoop, TickOutcome};

const NODE_ID: &str = "device_thermostat_01";

fn full_bridge() -> BridgeLoop {
    BridgeLoop::new(NODE_ID, Capabilities::full())
}

#[test]
fn ingest_reflects_device_readings_on_node_properties() {
    let mut bridge = full_bridge();

    bridge.run_tick().expect("tick should not hard-fail");

    let props = bridge.host_state().properties_for(NODE_ID);
    assert!(props.contains_key("iot.temperature"));
    assert!(props.contains_key("iot.humidity"));
    assert!(props.contains_key("iot.last_seen"));
}

#[test]
fn setpoint_property_drives_the_mock_device() {
    let mut bridge = full_bridge();

    // Simulate an external actor (operator UI / other extension) setting desired state.
    bridge.seed_node_property("iot.setpoint", PropertyValue::Number(30.0));
    bridge.seed_node_property("iot.power", PropertyValue::Bool(true));

    bridge.run_tick().expect("tick should not hard-fail");

    assert_eq!(bridge.device().setpoint, 30.0);
    assert!(bridge.device().power);
}

#[test]
fn device_temperature_converges_toward_setpoint_over_several_ticks() {
    let mut bridge = full_bridge();
    bridge.seed_node_property("iot.setpoint", PropertyValue::Number(30.0));
    bridge.seed_node_property("iot.power", PropertyValue::Bool(true));

    for _ in 0..10 {
        bridge.run_tick().expect("tick should not hard-fail");
    }

    assert!((bridge.device().temperature - 30.0).abs() < 1.0);
}

#[test]
fn missing_capability_fails_closed_with_a_clean_error() {
    let mut bridge = BridgeLoop::new(NODE_ID, Capabilities::empty());

    let result = bridge.run_tick();

    assert!(result.is_err(), "tick should fail closed when no capabilities are granted");
    assert!(
        bridge.host_state().properties_for(NODE_ID).is_empty(),
        "no property should have been written on a denied capability"
    );
}

#[test]
fn bad_reading_skips_the_tick_without_stopping_the_loop() {
    let mut bridge = full_bridge();

    // Force a NaN into the device to simulate a corrupted sensor reading.
    bridge.device_mut().temperature = f64::NAN;
    let outcome = bridge.run_tick().expect("a bad reading must be skipped, not a hard error");
    assert!(matches!(outcome, TickOutcome::Skipped { .. }));

    // The loop must still be usable afterwards.
    bridge.device_mut().temperature = 21.0;
    let outcome = bridge.run_tick().expect("subsequent tick should succeed");
    assert!(matches!(outcome, TickOutcome::Applied { .. }));
}

/// Integration: the real `iot_bridge.rhai` script now runs against fe-plugin-test's
/// shared `RhaiTestRunner` host-fn surface (`node_get_properties`, `node_set_property`,
/// `query_select`, `ext_storage_get`/`set`, backed by `MockStorage`) — the swap called
/// out in AGENTS.md "Integration". This exercises the canonical host functions the real
/// engine registers, not this crate's private `IotHostAdapter` duplicate.
#[test]
fn iot_bridge_script_runs_on_fe_plugin_test_rhai_runner() {
    use fe_plugin_test::assertions::{assert_kv_set, assert_node_property_set, assert_query_ran};
    use fe_plugin_test::mock_host::MockHostEnv;
    use fe_plugin_test::rhai_runner::RhaiTestRunner;

    let host = MockHostEnv::new();
    // Seed desired-state so actuate() reads it back through node_get_properties.
    host.storage
        .seed_node_property(NODE_ID, "iot.setpoint", PropertyValue::Number(30.0));
    host.storage
        .seed_node_property(NODE_ID, "iot.power", PropertyValue::Bool(true));

    let runner = RhaiTestRunner::new(host);

    // The script only defines functions; append a top-level `tick(..)` call so eval
    // drives one ingest+actuate cycle through the real shared host fns.
    let script = format!(
        "{}\n tick(\"{}\", #{{ temperature: 21.5, humidity: 40.0, timestamp: 1720000000 }});",
        iot_bridge::SCRIPT_SOURCE, NODE_ID
    );
    runner
        .eval_script(&script)
        .expect("iot_bridge.rhai tick must run on the shared RhaiTestRunner surface");

    let host = runner.host();
    // ingest() wrote the three device properties via node_set_property.
    assert_node_property_set(&host, NODE_ID, "iot.temperature").unwrap();
    assert_node_property_set(&host, NODE_ID, "iot.humidity").unwrap();
    assert_node_property_set(&host, NODE_ID, "iot.last_seen").unwrap();
    // actuate() ran the history SELECT via query_select.
    assert_query_ran(&host, "SELECT").unwrap();
    // tick() bumped its per-extension KV counter via ext_storage_set.
    assert_kv_set(&host, "iot_bridge.tick_count").unwrap();
}
