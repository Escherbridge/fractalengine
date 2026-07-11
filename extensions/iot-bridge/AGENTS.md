# iot-bridge

## What this proves

That an extension can interop with an IoT device bidirectionally through the engine's
node/property model: **push** device data into a node (`ingest`), and **pull** node
state to drive device operations (`actuate`). `BridgeLoop` wires an in-memory
`MockThermostat` to the `scripts/iot_bridge.rhai` script's `tick()` entry point, one
call per simulated device tick.

```
MockThermostat.step() -> Reading -> tick(node_id, reading)
                                       |-- ingest(): reading -> node properties
                                       `-- actuate(): node properties -> device command
                                                                            |
                                                        MockThermostat.apply_command() <-
```

## Data contract: property keys

All properties this extension reads/writes are namespaced `iot.*`:

| Key               | Direction        | Meaning                                   |
|-------------------|-------------------|--------------------------------------------|
| `iot.temperature` | device -> node    | last ingested temperature reading (°C)      |
| `iot.humidity`    | device -> node    | last ingested humidity reading (%)          |
| `iot.last_seen`   | device -> node    | timestamp of the last ingested reading      |
| `iot.setpoint`    | node -> device    | desired temperature; read by `actuate()`    |
| `iot.power`       | node -> device    | desired power state; read by `actuate()`    |

`iot.setpoint`/`iot.power` are written by some other actor (operator UI, another
extension) — `BridgeLoop::seed_node_property` simulates that in tests.

## Capabilities

`manifest.json` requests `["storage.read", "storage.write", "query.select"]`. Mapping
onto the HOST-FN CONTRACT: `storage.read` gates `node_get_properties`/`ext_storage_get`,
`storage.write` gates `node_set_property`/`ext_storage_set`, `query.select` gates
`query_select`. Missing a capability raises `TickError::CapabilityDenied`, which
`BridgeLoop::run_tick` propagates as `Err` (fail-closed) rather than silently no-op'ing.

`manifest.json` also carries fe-plugin's existing scope-pattern fields (`verse_scope`,
`petal_scope`, `property_scope`, `network_scope`, `external_http`) for forward
compatibility, since `property_scope: ["iot.*"]` already expresses the same intent as
the flat `capabilities` list for node property access. The flat list is what this crate
actually enforces today; reconcile the two once fe-plugin's `CapabilityManifest` grows a
`capabilities` field of its own.

## Hardening (device data is untrusted)

- `device::Reading::validate()` rejects non-finite (NaN/Infinity) temperature/humidity
  and non-positive timestamps *before* the script ever runs.
- `ingest()` (in the `.rhai` script) additionally requires all three reading fields to be
  present, and clamps otherwise-finite sensor noise into `[-50, 150]`°C / `[0, 100]`%.
- `actuate()` treats missing/malformed `iot.setpoint`/`iot.power` as safe defaults (no
  setpoint override, power off) — it never emits a command it can't type-check first.
- `MockThermostat::apply_command` clamps the setpoint into a hardware-safe range
  (`[-20, 80]`°C) as the last line of defense, independent of what the script computed.
- `BridgeLoop::run_tick` never hard-fails on bad *data* — a rejected reading or a script
  fault (`TickError::ScriptFault`) is recorded as `TickOutcome::Skipped` and the loop
  keeps going. Only `TickError::CapabilityDenied` propagates as `Err` — that's a real
  authorization fault, not a data-quality issue.

## Integration

`src/adapter.rs` (`IotHostAdapter`) implements the HOST-FN CONTRACT
(`node_get_properties`, `node_set_property`, `query_select`, `ext_storage_get`/`set`)
against its own tiny in-memory `HostState`, with the exact same capability-gated
semantics the real engine will use. This exists only because
`fe-plugin-test::RhaiTestRunner`'s `Engine` field is private and it currently only
registers the legacy `get_node`/`set_property`/`create_node`/`query_nodes` surface — it
cannot be extended from outside that crate, and this crate was told not to edit it.

**Once worker 2 lands the HOST-FN CONTRACT on `fe-plugin-test::RhaiTestRunner`:**
replace `IotHostAdapter::new`/`compile`/`call_tick` in `src/adapter.rs` with
`fe_plugin_test::rhai_runner::RhaiTestRunner::new(host)` + `eval_script`/`call_fn`, and
back `HostState` with `fe_plugin_test::mock_host::MockHostEnv` directly. Nothing above
that file (`BridgeLoop`, `iot_bridge.rhai`, the property-key contract) needs to change —
that's the whole point of keeping the adapter as one small, swappable module.

`tests/bridge_loop.rs`'s last test (`iot_bridge_properties_are_representable_in_fe_plugin_test_mock_host`)
is a shape-compatibility check confirming the values this adapter produces round-trip
through `MockHostEnv`/`assert_property_set` today, so that swap is low-risk.

## Running it

```sh
cd extensions/iot-bridge
cargo check   # standalone workspace ([workspace] table in Cargo.toml); no root build needed
cargo test    # unit tests in src/*.rs + tests/bridge_loop.rs
```
