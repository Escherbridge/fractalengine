# fe-sim-model

The pure fleet model split out of `fe-sim` (extension_platform P5.1, R30). Dependencies: `serde` only, so it compiles for `wasm32` and the first-party simulation extension (`extensions/fleet-sim`) can embed it.

## What lives here

- `sensors.rs`: `SensorModel` and `evaluate` (pure function of config, tick, simulated time; keyed splitmix64 noise, no RNG state).
- `fleet.rs`: `FleetModel` (anchors, sensors, movers, `mover_tick_ms`, `start_ms`, `duration_ms`), `plan_readings`, `plan_movers`, `route_progress`, caps and `SIM_TIME_LIMIT_MS`.

## What stays in fe-sim (lab-only)

`FleetConfig` in `fe-sim/src/fleet.rs` wraps the model with the v1 lab fields (`peers`, `ingest_peer`, `verse_name`) and the real-clock bound (`start_ms <= now`). `FleetConfig::model()` is the bridge. `fe-sim/src/{fleet,sensors}.rs` re-export these items, so scenario/session code and the existing test filters are unchanged.

## FleetConfig v2 movers

`MoverSpec { node, route, speed_mps, loop, start_offset_s }`. A v2 document is a `FleetModel` JSON (no peers needed); a lab `FleetConfig` accepts the same `movers` array.

- The model never reads a route: `plan_movers` takes a `route_length_m(name)` lookup from the driver and emits `progress` in `[0, 1]`; the driver feeds it to the single route sampler shared with fe-terrain (`position_at_progress`), so there is one position implementation.
- `loop` wraps (`fract`), otherwise progress clamps at 1. Before `start_offset_s` a mover sits at progress 0.
- Plans are totally ordered by `(at_ms, index)` and pure, so two runs give the same fingerprint.
- Mover samples have their own cap (`MAX_PLANNED_READINGS`), separate from the sensor cap.
