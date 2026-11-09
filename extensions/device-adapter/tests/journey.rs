//! Journey: recorded http, websocket and mqtt frames run through the built component in the sandbox host and
//! land as readings and a transform on the device's node; bad frames are refused with distinct statuses.
//! Needs `fe-ext build` first (skips without dist/component.wasm).

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

#[test]
fn sandbox_frames_become_readings_and_a_transform_and_bad_frames_are_refused() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !project.join("dist/component.wasm").is_file() {
        return;
    }
    let fixture = project.join("fixtures/basic.json");
    let first = fe_ext::sandbox::run_project(project, &fixture, None).expect("first run");
    let second = fe_ext::sandbox::run_project(project, &fixture, None).expect("second run");
    assert_eq!(first.fingerprint, second.fingerprint, "a sandbox run is a pure function of its inputs");

    let trace = &first.trace;
    assert_eq!(trace["faults"].as_array().map(Vec::len), Some(0), "faults: {}", trace["faults"]);

    // http acks 202 with the count; websocket and mqtt stay quiet on success; a malformed frame is 400, an unknown device 404.
    let statuses: Vec<(u64, Option<u64>)> = trace["ingress_replies"]
        .as_array()
        .expect("replies")
        .iter()
        .map(|entry| (entry["tick"].as_u64().expect("tick"), entry["reply"]["status"].as_u64()))
        .collect();
    assert_eq!(statuses, vec![(1, Some(202)), (2, None), (2, None), (3, Some(400)), (3, Some(404))]);

    // Two http readings, one websocket reading, one mqtt reading: all anchored on the node that carries device.id dev-1.
    let readings = trace["readings"].as_array().expect("readings");
    assert_eq!(readings.len(), 4);
    assert!(readings.iter().all(|reading| reading["node_id"] == "sensor-1"));
    let metrics: Vec<&str> = readings.iter().filter_map(|reading| reading["metric"].as_str()).collect();
    assert_eq!(metrics.iter().filter(|metric| **metric == "temperature_c").count(), 2);

    // The websocket frame moved and turned the node, and the newest frame time is on device.last_seen.
    let nodes = trace["nodes"].as_array().expect("nodes");
    let sensor = nodes.iter().find(|node| node["id"] == "sensor-1").expect("sensor-1");
    assert_eq!(sensor["position"], serde_json::json!([12.5, 0.0, -4.0]));
    assert_eq!(sensor["properties"]["device.last_seen"].as_u64(), Some(2_001_000));
}
