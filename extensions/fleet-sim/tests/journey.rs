//! Journey: the built component, run in the sandbox host, moves its truck along the route, emits
//! simulation readings and reproduces its fingerprint. Needs `fe-ext build` first (skips without dist/component.wasm).

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

#[test]
fn sandbox_fleet_moves_the_truck_and_emits_readings_reproducibly() {
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

    // Eleven temperature readings (500 ms) and five altitude readings (1000 ms) over 5.5 s of fleet time.
    let readings = trace["readings"].as_array().expect("readings");
    let count = |metric: &str| readings.iter().filter(|reading| reading["metric"] == metric).count();
    assert_eq!((count("temperature_c"), count("altitude_m")), (11, 5));

    // 10 m/s for 5.5 s along the first 100 m leg (+x): the truck sits near x = 55.
    let nodes = trace["nodes"].as_array().expect("nodes");
    let node = |id: &str| nodes.iter().find(|node| node["id"] == id).unwrap_or_else(|| panic!("node {id}"));
    let truck_x = node("truck-1")["position"][0].as_f64().expect("x");
    assert!((50.0..60.0).contains(&truck_x), "truck x = {truck_x}");

    // The instance marked the fleet running, and released its driver claim when the host stopped it.
    let fleet = &node("fleet-1")["properties"];
    assert_eq!((fleet["fleet.status"].as_str(), fleet["fleet.driver"].as_str()), (Some("running"), Some("")));
}
