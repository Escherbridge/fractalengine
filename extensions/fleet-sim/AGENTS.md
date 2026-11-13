# extensions/fleet-sim

Example simulation extension (`kind: simulation`, track extension_platform P3.3): drives `fleet.config` nodes
(FleetConfig v2, `fe-sim/model`) as a WIT 2.0 component. The manifest is a byte copy of
`fe-sdk/schema/examples/fleet-sim.extension.json`; keep the two in step.

## Build and run

Standalone crate (own `[workspace]`), built for `wasm32-wasip2` by `fe-ext build`, never a host workspace member.
Root `Cargo.toml` lists it under `exclude` for clarity (see the track findings). It tracks its own `Cargo.lock`,
created by the first `cargo check --manifest-path` (never hand-written; see `.gitignore`).

    fe-ext build --project extensions/fleet-sim
    fe-ext run --sandbox --project extensions/fleet-sim --fixture extensions/fleet-sim/fixtures/basic.json
    cargo test --manifest-path extensions/fleet-sim/Cargo.toml          # unit tests only (plan.rs)

The sandbox journey is the fleet-sim leg of J8 (`tools/fe-ext/tests/j8_cli.rs`, `cargo test -p fe-ext -- j8`): it runs
once `dist/component.wasm` exists and checks the fixture outcomes below plus `pack` and `inspect`. It lives there, not
in this crate, so the example never pulls the host stack (fe-plugin, wasmtime) into its own target directory and
lock file, and so a batch filter reaches it (G3, 2026-11-08).

## §package-and-install

Recipe the GUI QA path uses (key files stay outside the repo; `fe-ext keygen` never overwrites):

    fe-ext keygen publisher.key                      # publisher key; a second `keygen index.key` is the index key
    fe-ext build --project extensions/fleet-sim
    fe-ext validate --project extensions/fleet-sim
    fe-ext pack --project extensions/fleet-sim --key publisher.key --out packages/   # sealed, signed .hexon
    fe-ext inspect packages/<id>-0.1.0.hexon          # ids, content hash, capability digest, signer
    fe-ext index build --packages packages --base-url <public-root> --key index.key --out site
    fe-ext index verify site --index-key <did:key of index.key>
    fe-ext admin ...                                  # install-file verb on the running app's local channel

- `pack` is the only step that needs the publisher key; `sign` re-seals under another key, `countersign` adds a
  signer without moving `content_hash`.
- Install is GUI-only plus the local `fe-ext admin` channel, never REST or MCP; the GUI consent dialog always
  confirms the capability grants (governance.md). A registry install names the source and package instead of a file.
- `fe-ext index` and `fe-ext admin` are wired by I-FEXT/A-CLI; exact flag spelling is in `tools/fe-ext/AGENTS.md`.

## §driver

- Nodes are found by `scene.query-nodes` (one 200-node page) and kept when `fleet.config` parses and validates
  (`FleetModel::validate`, the same table the lab uses). The property may be an object or a JSON string.
- `fleet.status` (absent | running | stopped | finished) is the only run state in the scene, so a restart or a second
  peer reads the same truth. `fleet.driver` is the installation id of the one instance that drives the fleet;
  deactivate and uninstall release it by staging `""`. An instance never drives a fleet another installation claimed.
- **The claim travels alone (R30, G5).** An unclaimed fleet's tick stages only `fleet.driver` and records the fleet
  clock origin in `kv`; it drives nothing. Driving starts on the tick whose scene read shows the claim as its own,
  so if two peers claim at once and the other claim wins the last-writer race, this peer wrote no readings or
  transforms for that fleet. Other fleets' writes may share the claim's window; "alone" is per fleet. The origin is
  the claim tick, so a run's readings are the same as when the claim and the first drive shared a tick.
- An instance that sees another driver drops its own `kv` clock, so a later takeover (after a release) restarts from
  its own origin instead of replaying a burst. There is no lease yet: a host that dies without deactivating keeps its
  claim until someone clears `fleet.driver` (the `start`/`stop` commands do not touch it).
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
