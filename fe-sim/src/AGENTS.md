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
- **Replays and snapshots walk hub WRITE order** (`DocEntry.write_seq`), not
  `HashMap` order (F9 fix). fe-sync publishes a shard's `__shards` ledger row
  before its first reading; a replay that reordered them made a returning
  non-host see the reading for a not-yet-known shard and over-retain it
  (unknown shard ⇒ retain) — nondeterministically. Pinned by
  `convergence_replay_preserves_write_order`.
- **Delivery ledger**: `delivered` records every doc entry handed to a
  peer's subscriber; `visible_entries(peer, table)` = authored ∪ delivered.
  That is the scenario runner's exact convergence target (§scenario-runner).
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

## §gossip-plane (F9/A21 — the compute plane on the hub)

`SimTransportFactory::join_gossip_topic` hands the sync thread a
`SimGossipTopic` (fe-sync's `VirtualGossipTopic` seam): the distributed-query
fan-out (`SubmitComputeTask` request + per-host partial responses) rides the
SAME `(due_ms, seq)` heap, latency, partitions, and churn as doc rows.

- **Membership** is per topic key, one entry per join; the member's DID is
  its link key (same as the doc plane), its iroh `NodeId` tags the frames it
  publishes (`from`, `direct: true` — hub deliveries are 0-hop). The sync
  thread passes its own `local_did`/`local_node` (one ed25519 key — F20), so
  fe-sync's F23 forged-attribution gate authenticates sim envelopes exactly
  as it does direct iroh deliveries.
- **Broadcast = one delivery per CURRENT member linked to the publisher,
  self included** (iroh-gossip self-echo parity; fe-sync's `SELF_ECHO` gate
  drops our own request, and our own response routes to no collector).
  Targets are ordered by `(DID, token)` so seq assignment never depends on
  which sync thread joined first.
- **No history.** Unlike the doc plane there is no replay: a member offline
  or partitioned away at schedule time is never scheduled, and a frame whose
  link breaks in flight is dropped at drain (counted). Returning peers see
  nothing they missed — the degradation scenario depends on this. Pinned by
  `gossip_has_no_history_for_offline_or_cut_members`.
- **Leaving**: dropping the last handle (`Drop` — the sync thread's
  `CloseVerseReplica` unsubscribe) removes the membership, so later
  broadcasts never target it; a frame already in flight to it drains as
  `SubscriberGone` (counted, like a closed replica).
- Bounded member inbound (256), drop-and-count on full — the same posture as
  the doc subscribers. `gossip_deliveries()` counts frames handed over (the
  scenarios assert > 0: the fan-out really rode the virtual plane).

## §identity (seeded peers)

`SimPeer::spawn(net, name, dir, seed)` derives the keypair from
`peer::identity_seed(seed, name)` (blake3 `derive_key`) through the
harness's `TestPeer::spawn_with_identity`. Needed for determinism, not
cosmetics: a ≤4-peer fabric probes every candidate and breaks utilization
ties by DID (fe-sync `placement::choose_by_hash`), so random DIDs made shard
→ host placement — and therefore every coverage/missing assertion — vary per
run. Row sizes (the load input) are fixed-length (ULIDs, did:key, sim-clock
HLC digits), so placement is a pure function of `(script, seed)`.

## §scenario-runner (scripted deterministic scenarios)

A `ScenarioScript` = fleet + optional `timeseries` fabric settings (put on
the verse manifest as `ts_*`) + `seed` + events (faults and `query`), all
declarative JSON; the A21 scripts live in `fe-sim/scenarios/*.json`. The
driver merges plan + events into one action list (events rank BEFORE
same-instant ticks — a fault lands, then the tick's traffic sees it), then
walks it: advance the clock to each action, apply/ingest/query, `step()`.

- **Event `at_ms` is an offset from `start_ms`, converted to absolute time
  at merge** (F9 fix: F8 compared raw offsets against absolute tick times,
  so every fault applied before the first tick — the default script never
  actually exercised mid-run churn/partition).
- **Manifest quiescence**: after `OpenVerseReplica` on every peer, the driver
  sleeps 500ms before publishing the verse manifest — the hub fans out only
  to CURRENT subscribers, so a write racing a joiner's subscribe would never
  be delivered. The clock does not move during it. Then two barriers: the
  manifest row on every joiner, and every fabric knowing every `__peers`
  declaration + the mode (placement sees only the peers it knows).
- **Write barrier per tick** (`settle_writes`): after the ingest reply, wait
  until the hub doc holds every ingested reading. The DB→bridge→sync leg is
  async; without this, a row could hit the hub after the clock moved on —
  its due time (and whether a fault cuts it) would race.
- **Exact store barrier** (`settle_stores`, before every query and at the
  end): each peer's `iot_reading` count must EQUAL its target = rows it
  authored + rows the hub delivered to it that `placement::retention_decision`
  keeps under the ingest host's (complete) ledger. Not "≥": over-retention
  never settles and fails loudly. AND each fabric must know every
  `__shards` row visible to it — a trailing ledger row for a shard the peer
  does not host leaves its store count unchanged, yet a query planned before
  it lands silently omits that shard. Before a query the network is drained
  first (the clock advances to in-flight due times).
- **Queries** (`ScriptedEvent::Query`): `SubmitComputeTask` on the named
  peer with request id `sim-query-{label}`; the driver pumps the hub until
  the collector replies (the collector's deadline is REAL time —
  `timeout_ms`). A degraded query therefore waits its full deadline.
  **Clock-overrun guard**: if a query's network drain advanced past the next
  tick, the run bails (the tick's stamp would drift) — move the query or
  lower the latency.
- **Sync-event draining**: the sync thread's event sends are BLOCKING on a
  bounded(64) channel (`RowApplied` per retained inbound row); nothing else
  reads them in a sim run, so every settle/query poll drains all peers.
- **Fingerprint** (`ScenarioOutcome::canonical_fingerprint`): per-peer
  `CanonicalReading`s (anchor NAME, metric, units, `recorded_at_ms`, exact
  value bits, HLC wall bits), final placement (`{anchor}/{bucket}` → peer
  names), and per-query rows/covered/missing shards. Host lists are included
  EXCEPT for a fully-covered aggregate: F7's aggregate collector settles as
  soon as every shard is won, so a host whose answer was redundant may or may
  not be in `answered_hosts` (arrival order). Excluded: `reading_id`, DB
  node/petal ids, HLC counter bits.
- SQL strings in the runner interpolate only ULIDs and fleet ids from
  create-command results — no user input ever reaches RawQuery.

## §honest-limits (recorded, not hidden)

- **Inbound DB-channel drops**: fe-sync forwards inbound rows with
  `try_send` on the peer's bounded(64) DB channel and DROPS on full
  (§replication-backpressure). A burst (e.g. a full-doc convergence replay)
  above ~64 retained rows per peer can drop rows; the exact store barrier
  turns that into a loud timeout, never a silent pass. Keep fleets small
  (the A21 scripts: 30 readings) or raise the harness channel if a scenario
  needs bursts.
- Single ingest peer per fleet (the fleet config shape). The ingest host's
  ledger is therefore complete, which is what makes the store targets exact.
- The bin's CLI is the deterministic leg only (`run`/`print`). The REST/MCP
  control surface (`ScenarioSession`, DEC-C7) is F9/A20.
- Sim scenarios ride the harness's in-memory SurrealDB peers; nothing here
  touches the SurrealKV production store.

## §one-ulp (ROOT-CAUSED and FIXED — DEC-C6)

F8 saw replicated readings differ from the origin row by 1 ULP. Root cause:
**serde_json's default float parser is best-effort, not correctly rounded.**
The origin binds the `f64` straight into SurrealDB; the replica receives the
row as JSON bytes (`serde_json::to_vec` — ryu's shortest round-trip text, which
is exact) and the apply path parses them back
(`fe-database/src/handlers/replicated_row.rs` `apply_timeseries_row`,
`serde_json::from_slice`). For 17-significant-digit values the default parser
lands 1 ULP off (measured: 5395 / 200 000 sine-weather values; e.g.
`62.985254035088204` → `62.98525403508821`). SurrealDB, f32 intermediates,
and `format!` encodings were ruled out (both paths bind `serde_json::Value`).

Fix: the workspace `serde_json` dependency enables `float_roundtrip`
(correctly-rounded parsing; 0 / 200 000 mismatches). Workspace-wide by
construction (Cargo feature unification) — it also makes fe-api's JSON
ingestion store exactly what the client sent. Cost: ~2× float-parse time.
Pinned by `replication_json_round_trip_is_bit_exact` (fast tripwire) and by
the scenarios asserting replica rows bit-identical to origin rows. Query
VALUE assertions still use a 1e-9 epsilon — for summation order (per-shard
partial sums vs the oracle's tick-order sum), never transport loss.
