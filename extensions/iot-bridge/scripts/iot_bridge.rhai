// iot_bridge.rhai — push device readings into a node; pull node state to drive a device.
// Data contract: property keys are namespaced "iot.*" (see AGENTS.md). NaN/Infinity and
// timestamp sanity are rejected upstream in Rust (device::Reading::validate) before this
// script ever runs; this script clamps otherwise-finite sensor noise into physical bounds.

const MIN_TEMPERATURE_C = -50.0;
const MAX_TEMPERATURE_C = 150.0;
const MIN_HUMIDITY_PCT = 0.0;
const MAX_HUMIDITY_PCT = 100.0;

/// Clamp `value` into `[lo, hi]`.
fn clamp(value, lo, hi) {
    if value < lo { lo } else if value > hi { hi } else { value }
}

/// Validate + write one device reading onto the bound node (`iot.temperature`,
/// `iot.humidity`, `iot.last_seen`).
fn ingest(node_id, device_reading) {
    if !device_reading.contains("temperature") || !device_reading.contains("humidity") || !device_reading.contains("timestamp") {
        throw "iot_bridge.ingest: reading missing required field(s) (temperature, humidity, timestamp)";
    }
    if device_reading.timestamp <= 0 {
        throw "iot_bridge.ingest: non-positive timestamp";
    }

    let temperature = clamp(device_reading.temperature, MIN_TEMPERATURE_C, MAX_TEMPERATURE_C);
    let humidity = clamp(device_reading.humidity, MIN_HUMIDITY_PCT, MAX_HUMIDITY_PCT);

    node_set_property(node_id, "iot.temperature", temperature);
    node_set_property(node_id, "iot.humidity", humidity);
    node_set_property(node_id, "iot.last_seen", device_reading.timestamp);
}

/// Read desired-state properties from the bound node and return a safe device command.
/// Missing or malformed properties fall back to safe defaults (no setpoint override,
/// power off) — this never emits a command that would turn the device on without an
/// explicit, well-typed instruction to do so.
fn actuate(node_id) {
    let props = node_get_properties(node_id);
    let cmd = #{ setpoint: (), power: false };

    if props.contains("iot.setpoint") {
        let sp = props["iot.setpoint"];
        if type_of(sp) == "f64" || type_of(sp) == "i64" {
            cmd.setpoint = sp;
        }
    }
    if props.contains("iot.power") {
        let pw = props["iot.power"];
        if type_of(pw) == "bool" {
            cmd.power = pw;
        }
    }

    let history = query_select("SELECT * FROM iot_reading WHERE node_id = $node_id ORDER BY ts DESC LIMIT 1", #{ node_id: node_id });
    cmd.history_rows = history.len();
    cmd
}

/// Per-tick entry point: ingest a device reading, then compute the next device command.
fn tick(node_id, device_reading) {
    let seen = ext_storage_get("iot_bridge.tick_count");
    let count = if type_of(seen) == "()" { 1 } else { seen + 1 };
    ext_storage_set("iot_bridge.tick_count", count);

    ingest(node_id, device_reading);
    actuate(node_id)
}
