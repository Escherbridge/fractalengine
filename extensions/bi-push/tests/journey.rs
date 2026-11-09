//! Journey: a `host/query` snapshot becomes the JSON or CSV body the sink receives, carrying the rows `/api/v1/query`
//! returned; the built component activates cleanly in the sandbox. Needs `fe-ext build` first for the sandbox leg.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use bi_push::payload::{parse_snapshot, render, Format};

const SNAPSHOT: &str = r#"{"data":[{"node_id":"sensor-1","metric":"temperature_c","value":21.5,"units":"C","recorded_at_ms":3000100},{"node_id":"sensor-1","metric":"humidity_pct","value":40,"units":"%","recorded_at_ms":3000000}],"row_count":2,"truncated":false}"#;

#[test]
fn snapshot_rows_reach_the_sink_unchanged_as_json_and_as_csv() {
    let snapshot = parse_snapshot(SNAPSHOT).expect("snapshot");

    // JSON keeps the REST shape and the exact rows.
    let body: serde_json::Value = serde_json::from_slice(&render(&snapshot, Format::Json)).expect("json body");
    let original: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("original");
    assert_eq!(body, original);

    // CSV: one header, one line per row, same cell values.
    let csv = String::from_utf8(render(&snapshot, Format::Csv)).expect("csv body");
    assert_eq!(
        csv,
        "node_id,metric,value,units,recorded_at_ms\r\nsensor-1,temperature_c,21.5,C,3000100\r\nsensor-1,humidity_pct,40,%,3000000\r\n"
    );

    // A payload that is not a query result is refused, never pushed.
    assert!(parse_snapshot(r#"{"nodes":[]}"#).is_err());

    // Sandbox leg: activation subscribes and a run is a pure function of its inputs.
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !project.join("dist/component.wasm").is_file() {
        return;
    }
    let fixture = project.join("fixtures/basic.json");
    let first = fe_ext::sandbox::run_project(project, &fixture, None).expect("first run");
    let second = fe_ext::sandbox::run_project(project, &fixture, None).expect("second run");
    assert_eq!(first.fingerprint, second.fingerprint);
    assert_eq!(first.trace["faults"].as_array().map(Vec::len), Some(0), "faults: {}", first.trace["faults"]);
}
