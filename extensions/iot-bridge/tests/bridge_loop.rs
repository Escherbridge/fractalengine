//! Integration test for the IoT bridge vertical slice.
//!
//! Runs several ticks of a mock thermostat through `iot_bridge.rhai` via [`BridgeLoop`]
//! and checks the full round trip: device -> node properties, and node desired-state ->
//! device commands, plus the fail-closed and resilience guarantees. See AGENTS.md
//! "Integration" for how this swaps onto fe-plugin-test's `RhaiTestRunner` once it
//! exposes the HOST-FN CONTRACT.

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

/// Shape-compatibility check: values this adapter produces must round-trip cleanly
/// through fe-plugin-test's `MockHostEnv`/assertion helpers, since that's what the real
/// `RhaiTestRunner`-backed test will use once worker 2 lands the HOST-FN CONTRACT there
/// (see AGENTS.md "Integration"). `RhaiTestRunner` itself can't be extended today (its
/// `Engine` is private), so this test only exercises the shared `MockHostEnv` shape.
#[test]
fn iot_bridge_properties_are_representable_in_fe_plugin_test_mock_host() {
    use fe_plugin_test::assertions::assert_property_set;
    use fe_plugin_test::mock_host::MockHostEnv;

    let mut bridge = full_bridge();
    bridge.run_tick().expect("tick should not hard-fail");

    let props = bridge.host_state().properties_for(NODE_ID);
    let temperature = props.get("iot.temperature").expect("temperature must be set");
    let expected = match temperature {
        PropertyValue::Number(n) => n.to_string(),
        other => panic!("expected a numeric temperature, got {other:?}"),
    };

    let mut host = MockHostEnv::new();
    host.spy.set_property_calls.push((NODE_ID.to_string(), "iot.temperature".to_string(), expected.clone()));
    assert_property_set(&host, NODE_ID, "iot.temperature", &expected).unwrap();
}
