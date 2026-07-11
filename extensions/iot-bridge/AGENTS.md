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
against its own tiny in-memory `HostState`, with capability-gated (`require`) semantics.

**The swap has landed.** fe-plugin-test now hosts the same contract on
`RhaiTestRunner` (backed by `MockStorage`), so the integration test
`tests/bridge_loop.rs::iot_bridge_script_runs_on_fe_plugin_test_rhai_runner` runs the
real `iot_bridge.rhai` `tick()` through
`fe_plugin_test::rhai_runner::RhaiTestRunner::new(host)` + `eval_script`, asserting the
ingest/actuate cycle writes node properties, runs a SELECT, and bumps its per-extension
KV counter through the **canonical shared host fns** — not this crate's private
duplicate. That is the "swap onto the now-real host-fn surface."

`IotHostAdapter` is deliberately **kept** as the runtime host, not deleted:
`RhaiTestRunner` registers all five host fns unconditionally (capability-agnostic by
design), whereas the adapter's `require()` gate turns a missing capability into
`TickError::CapabilityDenied`. That fail-closed authorization path is what `BridgeLoop`
and the `missing_capability_fails_closed` tests depend on. So the two coexist: the
adapter for capability-gated **runtime**, `RhaiTestRunner` for shared-surface **script
verification**. `BridgeLoop`, `iot_bridge.rhai`, and the property-key contract are
unchanged either way.

## Running it

```sh
cd extensions/iot-bridge
cargo check   # standalone workspace ([workspace] table in Cargo.toml); no root build needed
cargo test    # unit tests in src/*.rs + tests/bridge_loop.rs
```
