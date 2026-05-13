// Property Transformer — converts Celsius to Fahrenheit.
//
// Called when a node's `raw_temperature_c` property changes.
// Writes the converted value to `temperature_f`.

fn on_property_change(node_id, key, value) {
    if key == "raw_temperature_c" {
        let f = parse_float(value) * 9.0 / 5.0 + 32.0;
        set_property(node_id, "temperature_f", f.to_string());
    }
}
