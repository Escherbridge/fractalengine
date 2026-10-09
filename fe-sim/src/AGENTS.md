# fe-sim/src — module notes

Design rationale for the simulation-lab modules (F8/M3, decision D3: full lab
simulation, in-process, on the SAME `VerseReplicator` trait as production —
prod and sim cannot drift by construction). Nothing on the production path
depends on this crate.

## §hlc-sim (the HLC source override)

`clock.rs` owns the accelerable [`SimClock`] and the process-global HLC source
shim: `install_hlc_source(clock)` wires `fe_database::op_log::
set_wall_clock_source` to read the SimClock, so every HLC stamp in the process
(sim readings included) reads simulated time; `uninstall_hlc_source` restores
real time. The override is a plain `fn` pointer — no closure state; the shim
reads the process-global CURRENT clock. Production processes never touch it.

- `SCENARIO_RUN_LOCK` serializes every install window process-wide. The
  override is process-global and cargo test runs library tests on parallel
  threads, so two concurrent scenarios (or the clock tests) would clobber each
  other's install — a run would stamp from another run's clock or from real
  time. `run_scenario` takes the lock first and holds it through the uninstall
  guard's drop.
- Determinism discipline in the driver (`scenario.rs`): advance the clock to
  the tick's simulated millisecond, THEN send the ingest batch, THEN await the
  DB reply before the next action. Each batch's HLC wall bits therefore equal
  the tick's millisecond exactly (counter 0 — every batch lands in a fresh
  simulated millisecond). Asserted in the scenario tests.
- `init_hlc(0)` runs per peer DB-thread spawn (harness) and resets the
  process-global HLC; because spawns happen under the run lock with the source
  installed, the reset itself reads the SimClock — deterministic.
- `auto_pump` (thread advancing sim time at `speed`× real time) is
  **demo-only**; deterministic scenarios always step manually (`advance_ms`).
  The speed factor is recorded config, never a stepped run's driver.

## §sensors (pure models — A18)

A reading's value is a pure function of `(model config, tick, at_ms)` — no RNG
state, no wall time, no hidden mutable state. "Randomness" is a keyed
splitmix64 hash (`noise_unit(seed, index)`), so any past value is reproducible
from its inputs alone. The serde shape is the declarative fleet JSON:
`{"type": "sine" | "weather" | "random_walk", ...}` — tagged, round-trip
pinned by test.

## §fleet (declarative config + pure planner)

`FleetConfig` is DATA: peers, ingest peer, anchors, one `SensorSpec` per
sensor (anchor + metric + units + cadence + model). `plan_fleet` is a pure
function of the config — it precomputes every reading's fire time, tick, and
value in total `(at_ms, sensor index)` order. Tick `k` fires at
`start_ms + k * cadence_ms` for `k >= 1` while inside `duration_ms`. The
runner never improvises an order; it walks the plan.

`rfc3339_from_ms` renders the `recorded_at` the seam requires — always
sim-clock-derived, never server time (determinism).

## §net (SimNet hub semantics — A19)

One hub per scenario, keyed by peer **DID** (the A3 role gate resolves those
DIDs; a factory-supplied label would desynchronize authorship — see
§scenario-runner). Core semantics, each pinned by a unit test:

- **Doc**: latest-per-key `DocEntry` map per namespace (iroh-docs parity). A
  write lands in the doc even when the writer is offline (the durable local
  write a real offline node performs) but fans out to nobody.
- **Deliveries**: `(due_ms, seq)` min-heap; `step()` drains what is due at the
  clock's current time in heap order — determinism comes from the heap, never
  thread scheduling. Link state is RE-CHECKED at drain time: a partition that
  hits while a message is in flight loses it (counted in
  `dropped_deliveries`), like a real cut.
- **Membership/churn**: `set_peer_online(did, bool)`; returning peers get a
  convergence replay of the doc state (the rejoin convergence a real swarm
  performs; idempotent under the union/row merge semantics).
- **Partitions**: `partition(groups)` splits by DID groups; `heal()` clears
  all cuts and replays doc state across healed links (idempotent).
- **Own writes never echo** to the author's own subscription (iroh InsertLocal
  parity; the inbound handler filters own-author rows anyway).
- **Backpressure**: subscriber channels are bounded (1024, `MockVerseReplicator`
  parity); a full channel drops with a warn and a count (the §replication-
  backpressure posture — never a blocking send on the hub).
- `SimVerseReplicator` implements `VerseReplicator` + `VirtualReplica` over
  the hub with the same loud-failure lifecycle as `IrohDocsReplicator`
  (closed replicas reject writes; a marked-open-failed replica is loudly
  non-replicating). `start_sync` is a no-op — membership is scripted in the
  hub, not dialed.

## §scenario-runner (scripted deterministic scenarios)

A `ScenarioScript` = fleet + fault events, all declarative JSON. The driver
merges plan + events into one action list (events rank BEFORE same-instant
ticks — a fault lands, then the tick's traffic sees it), then walks it:
advance the clock to each action, apply/ingest, `step()` the hub.

- **Manifest quiescence**: after `OpenVerseReplica` on every peer, the driver
  sleeps 500ms before publishing the verse manifest — the hub fans out only
  to CURRENT subscribers, so a write racing a joiner's subscribe would never
  be delivered (the same reasoning as scenario 10's 500ms; the settle budget
  backstops tail latency). The clock does not move during it.
- **Settle**: step + poll; advances the clock only while deliveries are in
  flight (scripted latency puts their due times in the simulated future).
  Nothing stamps time after the plan ends, so this moves messages, never
  content. Built-in scripts heal every fault before the end, so the converged
  target (every peer holds the full reading union) is exact.
- **Fingerprint** (`CanonicalReading`): anchor NAME (fleet-local identity — DB
  node ids are fresh ULIDs per run), metric, units, `recorded_at_ms`, exact
  value bits, HLC wall bits. Deliberately excluded: `reading_id` (a fresh
  server-side ULID per ingest), `source_did` (fresh keypairs per run), HLC
  counter bits (a concurrent pump apply may share the millisecond).
- SQL strings in the runner interpolate only ULIDs and fleet ids from
  create-command results — no user input ever reaches RawQuery.

## §honest-limits (for F9/A21 — recorded, not hidden)

- **The gossip plane is NOT virtualized.** A sim sync thread never joins
  gossip topics, so `VersePeers` rosters stay empty and fabric placement sees
  only the local peer. Mirror-mode replication (the default fabric) is
  unaffected — every reading fans out to all subscribers regardless of the
  roster — but `sharded`/`balanced` placement and the distributed-query
  compute transport would need the roster virtualized (or seeded) before sim
  scenarios can exercise them. That is F9/A21's seam to build.
- The bin's CLI is the deterministic leg only (`run`/`print`). The REST/MCP
  control surface and interactive fault injection are F9/A20.
- Sim scenarios ride the harness's in-memory SurrealDB peers; nothing here
  touches the SurrealKV production store.

## §one-ulp (observed, non-blocking)

The durable round-trip of a REPLICATED reading can differ from the origin
peer's row by 1 ULP in the f64 for irrational values (observed on weather
sensor values in the default scenario: origin vs replica differ in the last
bit of `value`). It is deterministic (identical across runs per peer), so the
determinism fingerprint is unaffected — but exact-equality assertions that
compare aggregates ACROSS peers (A15-style merges) should use an epsilon for
irrational inputs, or the fabric owners should trace the Surreal number
round-trip on the apply path. Recorded for the timeseries-fabric owners; not
an fe-sim defect.
