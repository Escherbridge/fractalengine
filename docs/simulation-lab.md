# Simulation Lab

`fe-sim` is FractalEngine's P2P simulation lab: synthetic IoT device fleets,
simulated peers, and time acceleration, all in-process — demo and test the
whole peer-to-peer fabric (replication, the timeseries shard fabric,
distributed queries, membership churn, partitions) without real hardware or
a real network.

The lab rides the **same `VerseReplicator` / `VirtualReplica` traits** the
real iroh-docs transport implements (`fe-sync`'s virtual-transport seam).
Production code never depends on `fe-sim`; a sim sync thread binds no iroh
endpoint at all (no `DocsStack`, no relay) and drives the byte-identical
command loop production uses — pending-writes, the inbound pump, startup
reconciliation, fabric bookkeeping. Simulation and production cannot drift
apart by construction, because there is only one implementation of the
replication logic they both run.

## CLI

```bash
fe-sim                     # run the built-in default two-peer script
fe-sim run <script.json>   # run one declarative scenario file
fe-sim print <script.json> # parse-validate + print the merged plan (no run)
```

`run` executes the scenario to completion and prints the outcome: readings
ingested, dropped deliveries, gossip deliveries, per-peer reading counts, and
every query's result (rows, covered/missing shards, missing hosts), followed
by a JSON fingerprint block. `print` validates a script and lists its planned
reading schedule without running anything — useful for checking a scenario
file before committing it.

The CLI is the deterministic leg only. The interactive control surface
(start/step/inject-fault/stop against a *running* session) is the REST/MCP
bridge described below — same `ScenarioScript` document either way.

## Scenario JSON anatomy

A scenario is one JSON document: a `fleet` (the synthetic world), optional
`timeseries` settings (the verse's replication mode), a `seed`, and a list of
scripted `events` (faults and queries).

```json
{
  "name": "sim-sharded-query",
  "seed": 21,
  "timeseries": { "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 120000 },
  "fleet": {
    "verse_name": "Sim Sharded Verse",
    "peers": ["alice", "bob", "carol"],
    "ingest_peer": "alice",
    "anchors": ["tower-a", "tower-b", "tower-c"],
    "sensors": [
      { "anchor": "tower-a", "metric": "temperature_c", "units": "C", "cadence_ms": 60000,
        "model": { "type": "sine", "baseline": 15, "amplitude": 8, "period_ms": 3600000 } }
    ],
    "start_ms": 1750000000000,
    "duration_ms": 600000
  },
  "events": [
    { "kind": "query", "at_ms": 601000, "peer": "bob", "label": "fleet-aggregate",
      "query": { "type": "window_aggregate", "metric": "temperature_c", "start_ms": 0, "end_ms": 660000 },
      "timeout_ms": 5000 }
  ]
}
```

### `fleet`

| Field | Meaning |
|---|---|
| `verse_name` | Name of the verse the run creates |
| `peers` | Simulated peer names, in spawn order — the first is the verse creator; every peer opens the replica |
| `ingest_peer` | Which peer's DB thread writes the real `iot_reading` rows (through the production F5 emission seam — rows are durable-first, then replicated) |
| `anchors` | Anchor node names; one node per entry is created in the fleet's petal on the ingest peer |
| `sensors` | One `SensorSpec` per synthetic sensor: `anchor`, `metric`, `units`, `cadence_ms`, and a `model` |
| `start_ms` | Simulated epoch-ms at fleet start (the scenario clock's origin) |
| `duration_ms` | Sensors fire on cadence while inside this window; tick `k` fires at `start_ms + k * cadence_ms` |

A reading's value is a **pure function** of `(model config, tick, at_ms)` — no
RNG state, no wall time. Three sensor models (tagged by `"type"`):

- `sine` — `{ baseline, amplitude, period_ms }`
- `weather` — `{ baseline, amplitude, diurnal_ms, seed, jitter }` (jitter is a
  keyed splitmix64 hash of `(seed, index)`, so any past value is reproducible
  from its inputs alone — never real randomness)
- `random_walk` — `{ start, step, seed }`

### `timeseries` (optional; absent = mirror defaults)

`{ "mode": "mirror" | "sharded" | "balanced", "replication_factor": R, "bucket_width_ms": N }`
— the same per-verse fabric settings the production UI/API expose (see
`fe-sync/src/AGENTS.md` §sharding). `mirror` replicates every shard to every
peer; `sharded` places exactly one host; `balanced` targets `R` hosts
(clamped to the reachable peer count).

### `events`

Each event carries `at_ms` as an **offset from `fleet.start_ms`**. Tagged by
`"kind"`:

| Kind | Fields | Effect |
|---|---|---|
| `peer_offline` | `peer` | Peer's writes stop reaching the swarm; nothing is delivered to it until it returns |
| `peer_online` | `peer` | Peer rejoins — the hub replays convergence to every linkable subscriber (the rejoin convergence a real swarm performs) |
| `partition` | `groups` (`Vec<Vec<String>>`) | Splits the network into named peer groups; any pair split across groups of an active partition cannot exchange deliveries |
| `heal` | — | Clears every active partition and replays convergence across healed links |
| `set_latency` | `latency_ms` | Sets the per-link one-way delivery latency (simulated ms) |
| `query` | `peer`, `label`, `query`, `timeout_ms` | Submits a distributed query on the named peer; fans out over the hub's gossip plane, merges per-host partials, and records the outcome under `label` |

Events rank **before** same-instant ticks in the merged action list — a
scripted fault lands before the tick's traffic sees it, so "take alice
offline at the same millisecond a reading fires" behaves as expected.

`query.query` is one of four shapes (tagged by `"type"`), mirroring
`fe-sync`'s `TsQueryKind`:

- `window_aggregate` — `{ metric, start_ms, end_ms }`: avg/min/max/count of
  `metric` per anchor over `[start_ms, end_ms)` (window bounds are offsets
  from `fleet.start_ms`)
- `readings_in_window` — `{ metric, start_ms, end_ms }`: raw rows in that window
- `latest_per_anchor` — `{ metric? }`: latest reading per `(anchor, metric)`
- `all_readings` — every reading of the petal

Query outcomes carry the same **honesty metadata** real distributed queries
do: `covered_shards` / `missing_shards` / `missing_hosts`, so a scenario can
assert exactly what a real degraded fleet would report — never a silently
guessed answer.

## Determinism guarantees

- **Seeded identities.** Each simulated peer's keypair derives from
  `(seed, peer name)` via a blake3 key derivation, not from random bytes —
  needed because shard placement breaks utilization ties by peer DID, so
  random identities would make shard-to-host placement (and therefore every
  coverage/missing assertion) vary run to run.
- **Pure, replayable delivery order.** The simulated network (`SimNet`) is a
  `(due_ms, seq)` min-heap of pending deliveries, drained in heap order as
  the clock advances — determinism comes from the heap, never thread
  scheduling. Replays and snapshots walk the hub's write order, not hash-map
  order.
- **`SCENARIO_RUN_LOCK`: one session per process.** The simulation lab
  installs a process-global clock-source override (every HLC stamp in the
  process reads simulated time while a session is live), so only one
  scenario may run at a time per process; a second run queues behind the
  lock rather than racing the override. The HLC is snapshotted before
  install and restored to `max(snapshot, real now)` on teardown — on normal
  stop, on error, and on abandonment.
- **Canonical fingerprints.** `ScenarioOutcome::canonical_fingerprint`
  captures, per peer: each reading's anchor name, metric, units,
  `recorded_at_ms`, exact value bits, and HLC wall bits; the final shard
  placement (`{anchor}/{bucket}` → peer names); and per-query rows/covered/
  missing shards. Two runs of the same script produce identical
  fingerprints — this is what the two CI scenarios below pin.

## CI scenarios (A21)

Two checked-in scenarios under `fe-sim/scenarios/` run in CI as regression
tests, pinned by fingerprint:

- **`sharded_query.json`** — three peers, `sharded` mode, R=1. Proves a
  distributed aggregate query's merged result equals the union ground truth
  with full shard coverage, and that the fingerprint is stable run to run.
- **`offline_degraded.json`** — the same fleet, but `alice` (the ingest
  peer) goes offline mid-run; a query during the outage reports her
  exclusive shards as `missing` (`missing_hosts: ["alice"]`); once she
  returns and the fleet re-converges, the same query reports full coverage
  with zero missing shards.

Together they prove the fabric's core promise: queries are *honest* about
what they could and couldn't see, both mid-outage and after convergence.

## REST/MCP control surface

The CLI's `run`/`print` are the deterministic leg; the interactive surface
drives a **long-lived session** against a running relay — `start`, `step`,
`inject-fault`, `status`, and `stop`, matching the one-shot runner exactly
(`run_scenario` is literally `start` → `step(u32::MAX)` → `stop`).

| REST | MCP tool | Effect |
|---|---|---|
| `POST /api/v1/sim/start` | `sim_start` | Starts a session from a `ScenarioScript` object, its JSON text, or a built-in name (`default` \| `sharded_query` \| `offline_degraded`) |
| `POST /api/v1/sim/step` | `sim_step` | Executes the next `n` actions (`1..=MAX_SIM_STEP`) |
| `POST /api/v1/sim/inject-fault` | `sim_inject_fault` | Applies a fault (`peer_offline` / `peer_online` / `partition` / `heal` / `set_latency`) immediately, under the same settle barrier a scripted fault gets; `query` events are rejected here (queries are scripted-only) |
| `GET /api/v1/sim/status` | `sim_status` | Snapshots the session without stepping the clock or the hub |
| `POST /api/v1/sim/stop` | `sim_stop` | Final settle → fingerprint → shutdown → outcome |

- **Guard order: Owner role → seam presence → arguments.** A session is
  process-level — it overrides the host's HLC source — so the check is
  Owner-of-any-scope, not scope-specific; a non-owner always sees a plain
  403, never a validation-detail 400.
- **Relay-only, double opt-in.** This surface exists only on
  `fractalengine-relay`, and only when **both** the `sim-control` cargo
  feature was compiled in **and** the runtime `FE_SIM_ALLOW=1` environment
  variable is set. The GUI binary never exposes it. See
  [fractalengine-relay/README.md](../fractalengine-relay/README.md) for the
  build/run commands and why a sim-control relay must serve no production
  verses.
- A session's own faults (`inject_fault`) reproduce scripted ones bit for
  bit — injecting the same churn live, at the same simulated instant, as a
  script that encodes it reproduces the identical fingerprint.

## Validation limits

Checked at parse/start time, before anything runs:

- `start_ms` must be at or before the real wall-clock time at validation
  (the past is the normal case; a future `start_ms` is rejected)
- every timestamp the scenario touches (fleet end, event offsets, query
  windows, interactive `set_latency` injections) must stay below `2^48` ms —
  HLC packs wall time into the upper 48 bits, so anything at or past that
  bound would silently truncate
- at most **16 peers** per fleet (each peer is two threads plus an in-memory
  database)
- at most **100,000** planned readings per fleet (bounds run time and
  memory; the CI scenarios use 30)

A scenario that violates any of these fails validation before a single peer
spawns, rather than running partway and hanging or silently truncating.

## See also

- [README.md](../README.md) — "Peer-to-Peer Sync & Distributed Data" for how
  the lab fits the rest of the P2P stack
- [fractalengine-relay/README.md](../fractalengine-relay/README.md) — build
  flags, env vars, and the Windows/per-handle-lock notes for running a relay
  at all
- [docs/bi-egress.md](bi-egress.md) — the BI egress path readings ingested
  by a sim fleet (or a real one) eventually feed
- `fe-sim/src/AGENTS.md` — full design rationale, module-by-module
