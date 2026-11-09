# extensions/fleet-sim

Example simulation extension (`kind: simulation`, track extension_platform P3.3): drives `fleet.config` nodes
(FleetConfig v2, `fe-sim/model`) as a WIT 2.0 component. The manifest is a byte copy of
`fe-sdk/schema/examples/fleet-sim.extension.json`; keep the two in step.

## Build and run

Standalone crate (own `[workspace]`), built for `wasm32-wasip2` by `fe-ext build`, never a host workspace member.
Needs `extensions/fleet-sim` in the root `Cargo.toml` `exclude` list (see the track findings).

    fe-ext build --project extensions/fleet-sim
    fe-ext run --sandbox --project extensions/fleet-sim --fixture extensions/fleet-sim/fixtures/basic.json
    cargo test --manifest-path extensions/fleet-sim/Cargo.toml          # unit (plan.rs) + tests/journey.rs

`tests/journey.rs` skips quietly when `dist/component.wasm` is absent. It is the fleet-sim leg of J8.

## §driver

- Nodes are found by `scene.query-nodes` (one 200-node page) and kept when `fleet.config` parses and validates
  (`FleetModel::validate`, the same table the lab uses). The property may be an object or a JSON string.
- `fleet.status` (absent | running | stopped | finished) is the only run state in the scene, so a restart or a second
  peer reads the same truth. `fleet.driver` is the installation id of the one instance that drives the fleet
  (first tick claims it; deactivate and uninstall release it by staging `""`). An instance never drives a fleet
  another installation claimed.
- The fleet clock lives in `kv` (`fleet/<node_id>`: `origin_ms`, `last_elapsed_ms`): elapsed = `now-ms - origin`,
  capped at `duration_ms`. `start` deletes the key (replay from the origin); `stop` only flips the status.
- Sensor readings: ticks in `(last_elapsed, elapsed]` per sensor (`plan::due_ticks`), value =
  `evaluate(model, tick, model.start_ms + tick * cadence)` exactly like `plan_readings`, so a lab run and a live run
  of the same config agree on values. `recorded-at` is the real time `origin + tick * cadence`. Anchors resolve by
  node NAME to a node id; metrics outside the manifest list are skipped with a warning, because one undeclared
  metric would make the host refuse the whole readings batch.
- Movers: when a `mover_tick_ms` boundary is crossed, progress comes from `plan::mover_progress` (same maths as
  `plan_movers`), the position and heading (`routes.sample` tangent, yaw about +Y) are staged, and one event goes to
  the `positions` stream. Work stops early when `window.room().ops` is zero; the next tick resumes.
- Readings go through `ReadingBatcher`: more than the window holds queue up and stage on later ticks.

## Fixtures

`fixtures/basic.json`: probe, truck, a 200 m L-shaped route (`gpx_points`) and a validated fleet; the sandbox places
it on node `fleet-1`. Twelve 500 ms ticks put the truck near x = 55 and yield 11 temperature and 5 altitude readings.
