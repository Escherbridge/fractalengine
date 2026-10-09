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
  time. `ScenarioSession::start` takes the lock first and holds it until
  the session drops — after the uninstall guard (§session).
- Determinism discipline in the driver (`scenario.rs`): advance the clock to
  the tick's simulated millisecond, THEN send the ingest batch, THEN await the
  DB reply before the next action. Each batch's HLC wall bits therefore equal
  the tick's millisecond exactly (counter 0 — every batch lands in a fresh
  simulated millisecond). Asserted in the scenario tests.
- `init_hlc(0)` runs per peer DB-thread spawn (harness) and resets the
  process-global HLC; because spawns happen under the run lock with the source
  installed, the reset itself reads the SimClock — deterministic.
- **Snapshot/restore (DEC-C13).** That reset is process-wide and HLC is
  forward-only, so a session would leak sim time into any co-resident
  production DB thread: a FUTURE `start_ms` persisted into production stamps
  (and across restarts via `init_hlc(max_persisted)`), a PAST one broke
  op-log monotonicity. `HlcSourceGuard::install` snapshots the HLC
  (`fe_database::op_log::snapshot_hlc`) BEFORE installing the source; its
  `Drop` uninstalls, then `restore_hlc` sets `max(snapshot, real now)`. The
  guard lives in `ScenarioSession` between `run` and `_run_lock`, so restore
  runs after every peer thread has joined and before the lock releases — on
  `stop`, on error, and on abandonment. Pinned by
  `past_time_session_restores_the_process_hlc` (session.rs) and the
  fe-database `op_log` restore tests.
- **Time validation (`FleetConfig::validate`, DEC-C13).** `start_ms ≤ real
  now` (past is the normal case; future is rejected), `start_ms < 2^48`
  (`SIM_TIME_LIMIT_MS` — HLC's `wall << 16` truncates beyond), and
  `start_ms + duration_ms < 2^48`; `ScenarioScript::check_time_limit` extends
  that horizon over every event offset, query window, and `SetLatency` (also
  checked for interactive `set_latency` injections). Below 2^48 every time is
  chrono-representable, so `rfc3339_from_ms`'s `expect` cannot fire on a
  validated fleet (release builds are `panic=abort`). Size caps ride the
  same pass: ≤ 16 peers, ≤ 100 000 planned readings, cadence ≥ 1 ms;
  `plan_fleet` iterates a bounded `1..=duration/cadence` range (the old
  `saturating_mul` loop never terminated for `duration_ms: u64::MAX`).
- Residual (recorded): stamps a co-resident production DB thread issues
  DURING a session still read sim time — restore cannot un-issue them. Hence
  the relay's runtime opt-in (§session).
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
  publisher EXCLUDED** (iroh-gossip 0.35 parity: a sender never receives its
  own broadcast). Until 2026-10-09 the hub self-echoed — harmless (fe-sync's
  `SELF_ECHO` gate dropped our own request; our own response routed to no
  collector) but a sim/prod drift that inflated `gossip_deliveries` with
  non-crossing frames (DEC-C13 #6). The gate stays as real-path defense.
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
  the doc subscribers. `gossip_deliveries()` counts frames handed over — all
  cross-peer, so the sharded scenario's `≥ 4` (each query's request reached
  both non-requesting peers) proves the fan-out really crossed the plane.

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
- **Sync-event + DB-echo draining**: the sync thread's event sends are
  BLOCKING on a bounded(64) channel (`RowApplied` per retained inbound row),
  and every replica's DB thread answers each applied inbound row with an
  unsolicited `ReplicatedRowApplied`; nothing else reads either in a sim run,
  so every settle/query poll (`Run::drain_events`) drains both on all peers.
  Before DEC-C13 only sync events were drained: a replica retaining ≳129
  rows between store barriers parked its DB thread on the full result
  channel, the driver's next `RawQuery` send parked on the full command
  channel, and the run hung forever (the settle deadline is checked between
  polls, never inside a blocked send). Discarding DB results there is safe
  by the single-driver-thread invariant (every driver command is
  `send`+`wait_for` on the driver thread, so no wait is outstanding during a
  drain); a non-echo result is a stray and is logged. The harness side also
  `try_send`s the echo (fe-test-harness `src/AGENTS.md` §peer-model).
  Pinned by `mirror_replica_retaining_hundreds_of_rows_between_barriers_never_deadlocks`
  (320 mirror rows, no query barrier; its thread guard exits the process
  with code 101 after 300s instead of hanging CI — a hung run would hold
  `SCENARIO_RUN_LOCK` and stall every later scenario test).
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

## §session (F9/A20 — the long-lived driver + control bridge)

`session.rs::ScenarioSession` is THE driver. `run_scenario` is literally
`start` → `step(u32::MAX)` → `stop`, so the interactive surface and the
one-shot runner cannot drift (pinned by
`stepped_session_matches_one_shot_run_exactly`: uneven step chunks with a
status snapshot between every call reproduce the one-shot fingerprint).

- **Lifecycle.** `start(script, root_dir)` = validate → take
  `SCENARIO_RUN_LOCK` → install the HLC source → spawn peers, hierarchy,
  replica opens, manifest, membership barriers → merge the action list
  (owned `Action`s, cursor 0). `step(n)` executes the next `n` actions with
  part A's exact barriers (`execute` is the old loop body verbatim).
  `inject_fault(event)` applies a fault NOW behind the same `settle_writes`
  barrier a scripted fault gets (its `at_ms` is ignored; the record carries
  the real sim offset). `status()` is a snapshot. `stop()` = final
  `settle_stores` → fingerprint → shutdown → outcome.
- **Lock discipline.** The session OWNS the `MutexGuard<'static, ()>`; field
  order is drop order (`run` → `_hlc_guard` → `_run_lock`), so peers join
  and the HLC source is uninstalled BEFORE the lock releases — on `stop`,
  on error, and on abandonment (`Drop` closes replicas first). Owning a
  `MutexGuard` makes the session `!Send`: it lives and dies on the thread
  that started it (the bridge thread).
- **Concurrent start** is rejected by the bridge (`control.rs`) with a
  `Conflict` BEFORE parsing or touching the lock — the bridge thread itself
  holds the lock through its live session, so a second start would
  otherwise self-deadlock. Direct `ScenarioSession::start` callers (tests,
  `run_scenario`) instead *queue* on the lock — serialization, not error.
- **Bridge state** is a plain `Option<LiveSession>` owned by the one bridge
  thread (deviation from the design note's `Mutex<Option<…>>`: a `!Send`
  session cannot be shared anyway, and serial servicing IS the mutual
  exclusion). A driver failure in step/inject/stop tears the session down
  and reports `Failed` — a run that missed a barrier is no longer a valid
  deterministic run. Each session gets a `TempDir` under the bridge's work
  root, removed after the session drops.
- **Event pumping lives inside calls — no background pump.** Every
  step/inject/stop barrier steps the hub and drains sync events (part A's
  `drain_events` discipline); `status()` drains events ONLY (never steps
  the hub or clock), so a status call cannot perturb the run. Between calls
  a sync thread may block on its full bounded(64) event channel; the next
  call's first poll unblocks it. A background pump would race the driver's
  barriers (nondeterminism) — rejected.
- **Interactive faults == scripted faults at the same instant.** Pinned by
  `injected_outage_degrades_then_heals_like_the_scripted_one`: the A21
  outage script with its churn events replaced by no-op `set_latency`
  markers, driven step-wise with the outage injected live, reproduces the
  scripted fingerprint (degraded query, convergence, healed query). Use a
  marker event to move the clock to an exact instant before injecting.
- `inject_fault` rejects `query` events (queries are scripted actions;
  an ad-hoc query verb is an open item) and unknown peer names
  (`ScenarioScript::validate_event_peers`, shared with script validation).
- **Big interactive fleets** hit §honest-limits' 64-slot inbound burst
  drop sooner: a live `peer_online`/`heal` replays the whole doc at once.
  The exact store barrier turns that into a loud settle timeout → the
  session is torn down (`Failed`), never a silent pass.
- **Process-global hazard on a host binary.** A live session installs the
  SimClock as the PROCESS's HLC source and `init_hlc` resets the process
  HLC per peer spawn. That is why the bridge is wired only into
  fractalengine-relay behind the default-off `sim-control` feature AND a
  runtime opt-in, `FE_SIM_ALLOW=1` (DEC-C13: defense beyond the compile
  feature — a sim-control build without it logs a warning naming the risk
  and does not spawn the bridge, so `/api/v1/sim/*` stays 503), and never
  into the GUI binary. Lab relays must not serve production verses. The
  HLC itself is snapshot/restored around every session (§hlc-sim).
- **Teardown never blocks on a send.** `shutdown(run)` drains, sends
  `CloseVerseReplica` with `try_send`, drains again, then drops the peers,
  whose `TestPeer::shutdown_inner` joins while draining — an abandoned
  session (Drop mid-run) can hold sync threads parked on full event
  channels.

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
  control surface (`ScenarioSession` + `control.rs` bridge, DEC-C7) is
  §session.
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
