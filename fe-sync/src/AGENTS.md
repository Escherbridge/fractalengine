# fe-sync — module notes

## §blob-store-directory-initialization

`FsBlobStore` evaluates `create_dir_all` by its required postcondition: an error
is harmless when the target now resolves to a directory. This guards the
observed Windows error 183 case, whose `io::ErrorKind` was not `AlreadyExists`
despite the OS message, while still rejecting a file or missing target at the
blob-store path. The same helper protects root initialization and per-hash
shard creation.

## §congestion-control — explicit BBR (track p2p_unblock_now_20260711 FR-4)

iroh 0.35's default QUIC congestion controller is **CUBIC**
(`iroh-quinn-proto` `TransportConfig::default()` sets
`congestion_controller_factory: CubicConfig`). Per iroh#4286, iroh-blobs
throughput differs ~30x between BBR (~40% of link) and CUBIC (~1-1.5%), so
`endpoint.rs::bbr_transport_config()` explicitly selects BBR:

- iroh 0.35 *does* expose the knob: `Endpoint::builder().transport_config()`
  takes a `quinn::TransportConfig` (re-exported as
  `iroh::endpoint::TransportConfig`), applied to both client and server
  configs. The concrete `BbrConfig` is NOT re-exported by iroh (only the
  `Controller`/`ControllerFactory` traits are), so fe-sync depends directly
  on **`iroh-quinn-proto = "0.13"`** — the exact quinn fork+version iroh
  0.35 pins, so the trait objects unify. When `iroh_1_0_upgrade_20260711`
  bumps iroh, this dep must be bumped in lockstep (iroh 1.0 may also change
  the default; re-verify then).
- Passing a custom `TransportConfig` replaces iroh's builder default
  wholesale, which only set `keep_alive_interval(1s)` — so
  `bbr_transport_config()` re-applies that.
- Startup logging: `SyncEndpoint::new` logs
  `congestion_controller = "bbr"` at info on successful bind (quinn has no
  endpoint-level "active controller" getter; the config is authoritative,
  `Connection::congestion_state()` exists only per-connection).

## §sync-thread-blocking-io

The sync thread runs a **current-thread** tokio runtime; any synchronous
syscall inside the async block stalls every queued gossip/replica/blob task.
`handle_write_row_entry` therefore reads blob files via
`tokio::task::spawn_blocking` (track p2p_unblock_now_20260711 FR-3). Keep new
filesystem or other blocking calls in this file off the runtime the same way.

## §iroh-0.35 — migration state (P2P Mycelium Phase F)

fe-sync was ported from the pre-0.35 iroh APIs to **iroh / iroh-docs / iroh-gossip 0.35**.
Two subsystems restructured significantly upstream; here is what was ported vs. deferred.

### iroh-gossip 0.35 — ported (real API)

The old `iroh_gossip::{Host, Topic, TopicId}` surface was removed upstream. Current wiring:

- Instance: `iroh_gossip::net::Gossip`, spawned once per online sync thread via
  `Gossip::builder().spawn(endpoint).await` (`sync_thread.rs`, offline mode → `None`).
- Topic id: `iroh_gossip::proto::TopicId` (32 bytes). Derived from the verse topic string
  with `gossip_topic_id()` (blake3 → `[u8;32]` → `TopicId::from`).
- Subscription: `gossip.subscribe(topic_id, bootstrap) -> GossipTopic`. We hold the live
  `GossipTopic` handles in `gossip_topics: HashMap<String, GossipTopic>`; **dropping a handle
  is how you leave a topic** (there is no `unsubscribe(topic_id)` anymore).
- Broadcast: `GossipTopic::broadcast(Bytes).await` (now async — the tileset
  advertisement handler is therefore `async`). Unsigned transform broadcasts
  are intentionally absent.

Deferred from this pass: consuming **inbound** topic messages — the tileset
handlers broadcast out but nothing drains a `GossipTopic`'s event stream yet.
Outbound `broadcast` is best-effort (messages queue until a neighbor is
available). Inbound *connections* are now routed since the P2P stack landed
(see §iroh-0.35 below): the stack's `Router` accepts `GOSSIP_ALPN`.

### iroh-docs 0.35 — stack real, replicators mock-backed (A1)

The full P2P stack now spawns in the sync thread (`docs_engine.rs::DocsStack`):
`iroh_blobs::net_protocol::Blobs::persistent(<dir>/blobs)` +
`iroh_gossip::net::Gossip::builder().spawn(endpoint)` +
`iroh_docs::protocol::Docs::persistent(<dir>)` (which wires `Engine::spawn` +
the redb replica store `docs.redb` + persistent default-author storage) +
`iroh::protocol::Router` accepting all three ALPNs (`/iroh-bytes/4`,
`/iroh-gossip/0`, `/iroh-sync/1`). Persistent stores live under `FE_P2P_DIR`
(`p2p_data_dir()`, default `data/p2p`); the dir is created at spawn.

- The stack spawns only when the endpoint bound. On **bind failure** or
  **stack-spawn failure** the sync thread degrades to offline mode: the
  holder stays empty (`is_available() == false`), gossip is absent, and
  replicators use the in-memory mock — loudly (`error!` +
  `SyncEvent::RelayHealthChanged`), never a crash.
- `IrohDocsEngineHolder` is a real holder (`Option<Arc<DocsStack>>`):
  `is_available()` reflects the stack; `docs_client()` hands out the
  `MemClient` clone the replicator layer will write through.
- **One gossip instance per endpoint.** Gossip is owned by the stack; a
  second `Gossip` spawned outside it would never see inbound connections
  (the Router routes to the stack's instance only). Likewise two stacks in
  one process must not share a data dir — the redb file lock makes the
  second spawn fail (→ degrade).
- **Real Doc-backed replicators (F2 / A2):** the `VerseReplicator` /
  `PetalReplicator` traits are async (hand-boxed `ReplicatorFuture`, no
  async-trait dep), and `IrohDocsReplicator` rides the real 0.35 `Doc`
  client whenever the stack is online: `open_document` imports the
  `Capability::Write(NamespaceSecret)` (secret hex) or opens the namespace
  read-only without one, `write_row` → `Doc::set_bytes` (empty payload →
  `Doc::del`, the empty-entry tombstone), `subscribe` → a `LiveEvent` pump
  (`InsertRemote` + `ContentReady` → `RowChange`s carrying the payload
  bytes read from the stack's blobs store; `InsertLocal` skipped for loop
  prevention; empty entries emitted as tombstones), `close` → pump abort +
  `Doc::close`. **Mock fallback is offline-only** — `is_available() == false`
  (bind failure, stack-spawn failure — loud, never a crash). An
  `open_document` failure while the stack is ONLINE is NEVER a mock install
  (F20 finding 4): the failed replica stays registered in a loud
  non-replicating state — `SyncEvent::ReplicaOpenFailed`, an `error!` open
  banner that says NOT replicating, and `write_row`/`subscribe`/`snapshot`
  all erroring — so the node can never look healthy while publishing
  nothing. `start_sync`
  runs on every doc-backed open, with or without outbound peers: the
  dialed side must have its sync task running to serve entries.

- **Entry author identity (F20 finding 2).** `DocsStack::spawn` imports the
  endpoint's own `SecretKey` as the docs author and sets it default
  (`authors().import` + `set_default`), so `entry.author()` resolves to the
  endpoint `NodeId` did:key — the SAME identity the fe-identity keypair
  exposes (`to_iroh_seed` shares the seed), which is what the A3 gate on the
  DB thread resolves roles against. Both the live pump and the snapshot
  attribute `RowChange.author_id` from `entry.author()` (`author_did_key`),
  never from `InsertRemote.from` (the FORWARDING neighbor — a ≥3-member doc
  would otherwise judge the relaying node, not the author). The per-dir
  `client.authors().default()` author is unrelated to the app identity and
  must not be used for attribution. Loop prevention stays correct because
  our own writes are authored by that same endpoint identity.
- **Namespace ids (F20 finding 3).** `fe_database::derive_namespace_id`
  returns the Ed25519 verifying key of the namespace secret — the id the doc
  actually registers under (`NamespaceSecret::from_bytes(secret).id()`), so
  the secretless open (`client.open(stored_id)`) finds a previously imported
  doc on the same persisted store. Legacy keyed-BLAKE3 ids still in live DBs
  can never match; the secretless path falls back to scanning known docs for
  the one whose `verse/{verse_id}` manifest key exists
  (`find_doc_by_verse_manifest`).

`status.rs` also carries a `TODO(iroh-0.35)` for applying inbound peer `SyncEvent::NodeTransformed`
to the local world (currently logged, not applied) — it depends on the inbound gossip route above.

Runtime behavior: row bytes traverse the real network. The fe-test-harness
scenario `two_peer_replica_sync` proves A2 end-to-end (two in-process peers
over loopback, READ-BACK from the joining peer's durable store).

## §inbound-apply (F2 / A2+A4)

The sync thread's command loop is a `tokio::select!` over two streams
(`sync_thread.rs`): commands and **inbound replica rows**. crossbeam's
receiver has no async API, so a dedicated bridge thread forwards
`cmd_rx.recv()` into a tokio mpsc via `blocking_send` (ordering preserved;
the bridge parks while the sync thread is busy). Every open replica spawns a
**per-replica inbound pump** (`replicator.rs::pump_live_events` → forwarder
task) feeding one aggregated `mpsc<(verse_id, RowChange)>`; closing the
replica aborts its pump. `spawn_sync_thread` therefore takes two new
parameters: `db_cmd_tx: Option<Sender<DbCommand>>` (the inbound apply path;
`None` = rows logged and dropped, for tests without a DB thread) and
`p2p_dir: Option<PathBuf>` (per-thread data dir — the redb store takes an
exclusive file lock, so multi-peer processes must give each sync thread its
own dir; `None` resolves `FE_P2P_DIR`).

Inbound rows flow: pump → aggregated stream → `handle_inbound_row_change`,
which (a) skips own-author rows (E.8 — the mock fallback echoes local
writes; the real path never echoes because `InsertLocal` is skipped at the
source), (b) emits `SyncEvent::RowApplied`, (c) `try_send`s
`DbCommand::ApplyReplicatedRow` to the DB thread — drop-and-count
(`INBOUND_APPLY_DROPS` / `inbound_apply_drop_count()`), never a blocking
send. The DB thread is the single SurrealDB writer and the A4 enforcement
point (see fe-database `handlers/replicated_row.rs`), where the A3 role gate
also lives (landed F3, 2026-10-07: deny-by-default, Editor+, roles resolved
from the local tables at the verse scope — never wire-supplied).

Bootstrap peers (A9 prep): `FE_SYNC_BOOTSTRAP` holds **semicolon-separated**
`NodeAddr` JSON entries (semicolons because the JSON contains commas), parsed
with iroh's own types; bare `NodeId` hex carries no address and is skipped
loudly. `OpenVerseReplica.bootstrap_peers` carries the same entry form
per-verse and merges with the env set (deduped); `SyncEvent::Started.node_addr`
emits our own dialable `NodeAddr` JSON in exactly that form (F4 also logs it
raw — not Debug-escaped — on the `SyncStatus updated: started` banner so it
is copy-pasteable into `FE_SYNC_BOOTSTRAP`).

**Startup reconciliation (F4, 2026-10-07; F20 self-drain fix).** Every
`OpenVerseReplica` runs `seed_reconciliation` after `subscribe`:
`VerseReplicator::snapshot()`
(`get_many(Query::single_latest_per_key().include_empty())` — the doc's
current entries, tombstones included) replays through the SAME inbound apply
path as the live pump. This exists because the A3 role gate permanently
skips a denied row: the entry never re-fires as an `InsertRemote`, so a row
denied while a role or the verse manifest had not yet converged locally
would be stranded in the doc store forever; the snapshot gives it a second
chance on every open (relay startup scan, relay `VerseCreated`, GUI
re-navigation). Best-effort: a snapshot failure is a loud warn, never fatal.
Own-author rows are filtered downstream by the inbound handler like any
other row. Mock-backed replicas (offline) snapshot their in-memory rows, so
tests exercise the same seam.

**Self-drain capacity rule (F20 finding 1 — regression-tested).** The
snapshot is UNBOUNDED, so the reconciliation pass must apply entries
DIRECTLY through `handle_inbound_row_change` (which `try_send`s to the DB
channel with drop-and-count) — it must NEVER await a send into the
aggregated inbound stream, whose only drainer is the very command loop
running the pass. The pre-F20 inline-await shape deadlocked the loop forever
on any doc with more entries than the 256-capacity channel (any real IoT
verse — readings are per-row doc keys), starving all inbound applies,
`Shutdown`, and any blocking crossbeam senders. General rule: **a handler
awaited inline in the select loop must never await a send into a stream only
that select loop drains.** The regression test
(`reconciliation_snapshot_larger_than_inbound_capacity_does_not_deadlock`)
pins the exact shape: 300-entry doc, live convergence, re-open replay past
capacity, and `Shutdown` still processing.

## §pending-writes (F21/M2 — the verse-manifest open race)

`WriteRowEntry` for a verse whose replica is not open yet is **retained, not
warn-and-dropped**: `handle_write_row_entry` queues it in the command loop's
`PendingWrites` map, and `handle_open_verse_replica` republishes the verse's
queue **in FIFO order through the same write path a live write takes** after
a successful open (publish/conflict semantics are identical to the live
path — same key encoding, same set_bytes/del, same doc latest-per-entry
rules).

Why the seam (not the relay instance): `create_verse_handler` emits the
verse manifest `ReplicationEvent` before the host's open reaches the sync
thread — the relay's `open_replica_on_verse_created` Bevy system and the
GUI's navigation `open_replica` both send their `OpenVerseReplica` AFTER the
manifest `WriteRowEntry` is already in flight, so the first manifest row hit
the no-open-replica warn-and-drop and never entered the doc. The manifest is
emitted exactly once at creation and can never be re-emitted, and
`seed_reconciliation` cannot heal a row that never entered the doc — a fresh
relay's doc would permanently lack its verse manifest, so a fresh peer
joining never received it (defeating A3's bootstrap window, which exists
precisely so the manifest arrives that way). Fixing the seam covers every
host: relay, GUI, sim, harness.

Rules:

- **Bounded retention:** cap 1024 entries (`PENDING_WRITES_CAP`), drop-oldest
  with a warn — the queue keeps the newest version per key, so a flush still
  converges the doc to the latest row content.
- **Failed open never flushes** (F20 finding 4 interaction): an online
  open failure leaves the loudly non-replicating replica publishing NOTHING —
  retained writes stay queued for a later successful (re-)open
  (regression-tested: `failed_online_open_retains_pending_until_successful_reopen`).
- **Process-lifetime only:** sync-thread shutdown drops whatever is still
  queued with a warn (rows stay durable in the local DB; their publish is
  lost this session). Residual hole, known and accepted: a verse created but
  whose replica never opens in the creating process (and never opens again
  in a later process through any write) relies on a later session's manual
  write or the verse-invite path to publish its manifest — a DB-side re-emit
  on replica open is the deferred shape if that hole ever matters.
- Regression tests pin the exact failure shape:
  `write_before_open_retained_then_flushed_through_offline_mock` (offline
  seam, snapshot read-back) and
  `manifest_written_before_replica_open_converges_to_fresh_peer_and_reads_back`
  (REAL sync thread: emit-before-open retained → flushed after open → a
  fresh peer converges the manifest through the real loopback transport →
  after clean shutdown the manifest reads back from the creator's own
  persisted doc via a secretless reopen).

## §write-policy (auth_policy_pattern_20260710 §D1)

**F2 moved the gate; F3 landed it.** `write_policy.rs` no longer gates
`handle_write_row_entry`: those commands are the *outbound* path — a local,
already-admitted DB write being published to peers — so gating there only
blocked our own publishes (A3's "outbound local DB-thread writes are no
longer denied"). Peer admission now happens on the DB thread:
`DbCommand::ApplyReplicatedRow` → `handlers::replicated_row.rs`
(`admit_inbound_row`) resolves the author's role from the local `role`
table at the verse scope and requires Editor+ through fe-policy
(deny-by-default, never wire-supplied — see fe-database/src/AGENTS.md
§handlers and §rbac-policy for the landed gate, its bootstrap window, and
the `get_role` projection fix it surfaced).

`PolicyHandle` and its evaluation logic remain in `write_policy.rs`
(unit-tested) as the building block for any future fe-sync-side admission
work; no production path consults it today. `PolicyHandle` derives
`Resource` so the app side can insert a stricter policy for tests. The
causal-DAG membership resolver is not here; it remains blocked on
per-operation signing.

The §warn-on-send-failure sweep is complete for this crate: every
`SyncEvent` send site in `sync_thread.rs` goes through the `send_sync_event`
helper (warn on failure) — including the F1-era startup sends and the
blob/tileset stub handlers, which were the last bare `.ok()`s (F3, 2026-10-07).

`SyncCommand::UpdateNodeTransform` is a legacy compatibility command, not a
transport path: the sync thread logs and drops it, and the UI no longer sends
it. Local gimbal commits still update the in-memory view and local DB. A
networked transform must be represented as a signed canonical operation that
passes the same admission model before it can enter gossip or a replica.

## §relay-health (track `p2p_asset_streaming_20260718` FR-1 / decision D-77)

**Why:** iroh 0.35's default n0-hosted relay servers **EOL 2026-12-31**. Before
this hardening pass, relay failure was quiet: default relay binding with no
`relay_url` config wired, offline detection only checked bind failure — post-EOL,
gossip/replica traffic would have queued silently forever. This section documents
the config + health model that replaces that silence.

### `RelayConfig` (`relay_config.rs`)

Three variants, matching the D-77 hardening note verbatim: `Default` (iroh's
n0-hosted infra — the EOL-bound one), `Disabled` (direct/LAN-only), `Custom(Vec<String>)`
(operator relay URLs). `RelayConfig::parse` accepts the keywords
`"default"`/`"disabled"`/`"none"` or a comma-separated URL list, validating every
URL eagerly with iroh's own `iroh::RelayUrl` parser (`FromStr`) so a
misconfiguration fails at parse time — and again at `to_relay_mode()` time in
case a `Custom` value was hand-built bypassing `parse` — rather than on first
dial. `to_relay_mode()` converts to the `iroh::RelayMode` the endpoint builder
expects (`iroh::RelayMode::{Default,Disabled,Custom(RelayMap)}` — all
re-exported directly from the `iroh` crate; no new dependency was needed).

`RelayConfig::from_env()` reads the `FE_SYNC_RELAY` env var (falling back to
`Default` with a warning on a missing var or parse failure) and is what
`spawn_sync_thread` calls today. **This is a stopgap** — `spawn_sync_thread`'s
signature was deliberately left unchanged (env var instead of a parameter) so
this pass didn't have to touch its 3 external call sites
(`fractalengine/src/main.rs`, `fractalengine-relay/src/main.rs`,
`fe-test-harness/src/peer.rs` — all outside `fe-sync/src/`). FR-7 (application
settings surface, D-78) is the intended real home for this value; swap
`from_env()` for an `AppSettings`-sourced `RelayConfig` then.

### `RelayHealth` (`relay_config.rs`, surfaced via `SyncStatus::health`)

Five states: `Unknown` (default, pre-signal) → `Healthy` / `Disabled` (from
`on_bind_success`, which reads the `RelayConfig` to tell "no relay needed" apart
from "relay reachable") or `Unreachable` (from `on_bind_failure`, hard bind
failure). Runtime signals move the state with `on_error`/`on_success`:
`Healthy` degrades one step to `Degraded` before reaching `Unreachable` (two
consecutive failures, not one, to avoid flapping on a single transient error);
`Disabled` is a fixed point — a disabled relay cannot "fail" or "recover".
`is_problem()` (`Degraded`/`Unreachable`) is what a consumer should alarm on;
`Disabled` and `Unknown` are deliberately excluded.

Wired today: the endpoint bind result (`on_bind_success`/`on_bind_failure`) and
a gossip-spawn failure right after a successful bind (`on_error`) — both in
`sync_thread.rs`'s startup sequence, both emitting a new
`SyncEvent::RelayHealthChanged { health }` that `status.rs::drain_sync_events`
applies to `SyncStatus.health`, loud-logging (`warn!`) whenever the new health
`is_problem()`.

**TODO(ultrapilot) — continuous monitoring not wired.** `iroh::Endpoint::home_relay()`
returns a `Watcher<Option<RelayUrl>>` that would let the sync thread detect
relay loss *mid-session* (not just at startup), but wiring it means turning the
command loop's blocking `cmd_rx.recv()` into a `tokio::select!` against the
watcher stream — a bigger structural change than this pass's scope. Only the
startup bind result and the gossip-spawn outcome are tracked; a live watcher
loop is the natural next step.

### EOL warning

`endpoint.rs::warn_default_relay_eol_once()` logs one `WARN` per process (a
`std::sync::Once` guard) the first time a `SyncEndpoint` binds against
`RelayConfig::Default`, naming the 2026-12-31 date and pointing at this section.
Deliberately not unit-tested for the "fires exactly once" property — `Once` is
process-global static state, so asserting on it across `#[test]` functions in
the same test binary would be order-dependent; the pure decision of *whether*
to warn (`RelayConfig::is_default_infra`) is what's tested instead.

## §lifecycle-forwarding (`lifecycle.rs`, track `node_lifecycle_addressing_20260725` FR-6)

`LifecycleForwarder` wraps a `crossbeam::channel::Sender<LifecycleEvent>` (the
`LifecycleEventSender` alias). The DB thread emits create / promote /
delete-tombstone / reflow events on this seam (fe-database's
`spawn_db_thread_with_sync_and_lifecycle` takes the sender half; the concrete
channel is the same type — fe-database can't depend on fe-sync, so it declares
its own `LifecycleEventSender = Sender<LifecycleEvent>` and the binary bridges
the two halves). Forwarding is `try_send`: a full channel drops the event with a
`warn` rather than stalling the emitting system (N-5 — no blocking on the seam).
The op-log stays the durable source of truth, so a dropped in-process forward is
a lost notification, never lost data. Each op emits exactly one event (a stamp
delete additionally emits `PathReflow` for its owning path) — asserted in
fe-database's `runtime_lifecycle_tests`.

## §tombstone-honoring reconciliation (`reconciliation.rs` / `replicator.rs`, FR-1 / N-4)

Inbound reconciliation is no longer a byte-count no-op. `reconcile_petal` applies
each peer row to the durable store through `fe_database::merge::apply_replicated_node`,
which **refuses to resurrect a locally-tombstoned node**: a stale replica that
still holds a node we soft-deleted is skipped (`MergeApplied::SkippedTombstoned`),
and an incoming tombstone converges the local row to deleted. `RowChange.is_tombstone`
is derived from the row content (`row_is_tombstone` — a non-null `tombstone`
field), not hardcoded `false`; `IncomingEntryApplicator::should_apply` gives
tombstones dominance over concurrent live writes (never LWW, D-A7). The durable
non-resurrection proof lives in `fe-database` `merge::tests`; the fe-sync layer
tests the flag detection + `should_apply` dominance.

## §virtual-transport (M3/F8 — A19, decision D3)

`virtual_transport.rs` is the seam the simulation lab rides: a **virtual**
replica transport implements the same `VerseReplicator` contract as the real
iroh-docs path, so prod and sim cannot drift by construction. The user's D3
decision ("Full lab simulation") demands this — a parallel simulation-only
replication model is forbidden.

- **`VirtualReplica`** — `VerseReplicator` + the open-phase lifecycle
  (`open_document`/`is_doc_backed`/`start_sync`/`mark_open_failed`/
  `open_error`, the exact surface `handle_open_verse_replica` drives on the
  real path). The names are iroh vocabulary; the contract is
  transport-neutral. The sim lab implements it in fe-sim `transport.rs`.
- **`VirtualTransportFactory`** — one per simulated peer; builds the
  replica at each `OpenVerseReplica`.
- **`spawn_sync_thread_with_transport`** — the spawn seam. With a factory
  installed, the thread binds **no iroh endpoint** (no real network at all,
  no relay, no `DocsStack`; `bound_endpoint_count()` stays put — this is
  A19's "no real network" clause, pinned by the seam test), emits
  `Started { online: true, node_addr: None }` (the transport IS live) with
  relay health `Disabled` (the fixed point — a virtual transport cannot
  fail or recover a relay). `spawn_sync_thread` (the production shape every
  host binary calls) delegates with `None`.
- **The command loop is byte-identical.** `AnyReplicator` is the enum the
  open path drives: `Iroh(IrohDocsReplicator)` or
  `Virtual(Box<dyn VirtualReplica>)`, delegating `write_row`/`subscribe`/
  `snapshot`/`close` + the lifecycle methods. Pending-writes retention, the
  inbound pump, `seed_reconciliation`, fabric bookkeeping (`__shards`/
  `__peers`/manifest learning), and receive-side retention all run the same
  code. Only the transport under the trait object differs.
- **Gossip/compute plane (F9/A21 — implemented hub-side in fe-sim):**
  `VirtualTransportFactory::join_gossip_topic(topic_key, local_did,
  local_node)` returns a `VirtualGossipTopic` (broadcast + take-once inbound
  of `VirtualGossipMessage { from, direct, content }`). In virtual mode
  `subscribe_to_verse_gossip_topic` takes that branch instead of iroh-gossip:
  the sender lands in `gossip_senders` as `TopicSender::Virtual` (the enum
  every compute call site now holds — `TopicSender::Real` wraps the iroh
  `GossipSender`, so the real path is unchanged), and
  `pump_virtual_gossip_topic` forwards inbound frames as `GossipIncoming`
  exactly like the real pump — the command loop cannot tell the planes
  apart. The sync thread passes its own DID + iroh `NodeId` (one ed25519
  key), so the F23 sender-identity gate applies to sim frames verbatim. A
  factory without a gossip plane (the trait default `None`) leaves the verse
  honestly topic-less (`SubmitComputeTask` runs local-only). fe-sim's
  `SimNet` implements it with NO self-echo (iroh-gossip 0.35 never delivers
  a sender's own broadcast back to it — the hub matched that on 2026-10-09,
  DEC-C13; the `SELF_ECHO` gate remains as defense), scripted
  latency/partition/churn, and NO history (fe-sim `src/AGENTS.md`
  §gossip-plane).
- **Still not virtualized:** petal-level replication stays mock-backed
  (legacy path, no sim consumer), and blob fetching across peers (sim fleets
  exchange rows, not GLB assets).
- **HLC parity:** simulated peers stamp HLC from their `SimClock` via
  `fe_database::op_log::set_wall_clock_source` (see fe-database
  `src/AGENTS.md` §hlc) — every simulated peer reads time from the same
  accelerable clock, keeping monotonicity and run-to-run determinism.

## §sharding (M2/F6 — A13/A14)

The sharded hybrid timeseries fabric: `sharding.rs` (shard model + per-verse
fabric state) and `placement.rs` (the pure planner). D2's design decisions and
why:

- **Shard id = (petal, anchor node, time bucket).** A reading maps to exactly
  one shard by `(anchor, recorded_at_ms)` `div_euclid` the verse's bucket
  width (epoch-aligned; `div_euclid` keeps pre-1970 timestamps floor-aligned).
  Anchor, not metric: a sensor's readings stay in one shard's timeline — the
  query fan-out (F7) targets an anchor's series, and shard keys stay few.
- **The fabric is sync-plane state, NOT SurrealDB state.** `VerseFabric`
  (settings + peer declarations + shard ledger) lives in the sync thread's
  command loop, one entry per open verse. `__shards/*` and `__peers/*` doc
  rows are consumed at the inbound seam (`handle_inbound_row_change`
  `note_entry`, BEFORE the own-author filter — our own snapshot rows are how
  the fabric re-learns after a restart) and return early: they never reach
  the DB thread (which would `NotApplicable` them anyway). This is why the
  ledger has no DB-side convergence test — there is nothing to converge in a
  store.
- **Ledger/declarations ride the verse's own namespace** under
  `__shards/{petal}/{anchor}/{bucket}` and `__peers/{did}`, replicating like
  any row; a joining peer converges the whole placement picture from the
  `seed_reconciliation` snapshot through the same `note_*` seam the live
  pump uses. One writer per shard ledger key (the peer that first saw the
  shard plans and publishes); conflicts resolve by the doc's
  latest-per-entry semantics. **Ledger admission (F23):** `note_entry`
  enforces `__peers` self-declaration (the row key must name its own signed
  author — a forged `__peers/{other_did}` capacity row is dropped; see
  §transport-admission-control item 4). The `__shards` Editor+ gate was
  evaluated and NOT implemented (no role data reaches the sync plane) — the
  residual is recorded in §transport-admission-control item 5.
- **`note_verse_row` learns settings from BOTH directions of verse-doc
  traffic**: outbound writes parse the manifest blob before publishing
  (`handle_write_row_entry`), inbound rows/snapshot replay through
  `note_entry`. The DB handler's manifest re-emission on every settings
  change (fe-database §timeseries-settings) is what makes a settings change
  cross to peers at all.
- **Placement is pure and deterministic** (`placement.rs`: no I/O, no
  clocks): a no-coordinator fabric's placement decision must be replayable
  from identical inputs by any peer. Modes: `mirror` → all peers (capacity
  deliberately does not gate a mode whose point is "everyone holds
  everything"); `sharded` → exactly one host; `balanced` → R hosts, R a
  request that clamps to the reachable peers (1..N — the slider, never a
  hard requirement). Picking is power-of-choices: probe `PROBES=4`
  hash-derived candidates, keep the least-utilized (ties break by DID — pure
  function of the inputs). A candidate set ≤ PROBES is probed in full, so
  small fleets always cover their underloaded peers.
- **Seeders are the overflow tier, never the regular pool.** `pick_host`
  excludes seeders from the regular branch (a seeder with room is never
  consumed by regular placement — it stays in reserve) and spills to them
  only when every non-seeder is full; a pick within the seeder's own capacity
  is spillover, not over-capacity. A full regular fleet with no seeder room
  is a **loud last resort, never a drop**: the least-utilized peer hosts the
  shard anyway, flagged `overflowed` so the caller warns — an over-capacity
  host beats invisible data. The last resort is reachable ONLY for a
  homeless shard (`hosts.is_empty()`): a shard that already has one host
  drops its extra replica slots honestly (fewer than R) rather than breaking
  a capacity declaration.
- **Capacity is a planning hint, not an enforcement wall** (D2 #3, A14): the
  smallest peer never caps the fleet total — a small declaration moves
  placement to the peers that have room; only when NO peer anywhere can fit
  the shard does the last-resort branch fire.
- **Transfer routing per mode (A13)**: `mirror` → `Broadcast` (all peers);
  `sharded`/`balanced` → `Targeted(hosts)` — the route is the record the
  ledger row publishes and F7's targeted transport will consume. On the doc
  transport every subscriber physically receives the entry, so the route is
  enforced on the **receive side**: `retention_decision` keeps a reading only
  when the local peer hosts the shard (mirror keeps everything — pre-F6
  behavior preserved). An **unknown** shard (ledger not converged locally)
  retains — the ledger row and the first reading race through the doc, and
  the safe direction is keep: union semantics make over-retention
  idempotent, while a wrongly-dropped hosted row would be data loss.
- **The ledger row publishes BEFORE the reading** (`handle_write_row_entry`:
  ledger first, then the row) so a receiving peer learns the host set before
  the row lands; estimates (`row_count`/`size_bytes`) are written once at
  first sight and tracked locally afterwards — re-publishing per reading
  would double doc writes for metadata that is not authoritative. The host
  set never re-plans in F6 (membership churn re-planning is deferred).
- **Diagnostics**: `SyncCommand::GetShardLedger` → `SyncEvent::ShardLedger`
  (fabric dump JSON — settings, peers, shards). The DB loop never sees
  `__shards`/`__peers` rows; `SyncCommand::SetShardDeclaration` updates the
  local `__peers/{did}` declaration in every open verse and republishes it.
- End-to-end proof: harness scenario `two_peer_shard_fabric` (A13/A14 through
  the real loopback transport — mode switching via the manifest, ledger
  crossing, capacity exclusion, smallest-peer-never-caps, and the
  never-homeless last resort).

## §distributed-query (M2/F7 — A15/A16/A17)

`distributed_query.rs` is the fan-out query engine: `SyncCommand::SubmitComputeTask`
became a real transport, not a stub. One `DistributedQueryCall`
(`fe_runtime::distributed_query` — request + crossbeam `reply` sender) enters
the command loop; the loop runs the plan/collect/merge synchronously (no
inline awaits — the collector is a spawned task, so the select loop never
blocks on a fleet), and answers on the embedded reply channel with the merged
`DistributedQueryOutcome`. `SyncEvent::ComputeResultReady` carries the
diagnostics echo.

- **Spec is structured, never SQL** (`TsQueryKind`:
  `WindowAggregate`/`ReadingsInWindow`/`LatestPerAnchor`/`AllReadings`).
  Responders render their own partial SQL via the fe-query builders, so a
  request can only ever read one petal's readings through the sanctioned
  shapes — the spec is the authorization surface fe-api guards (see
  fe-api/AGENTS.md §distributed-query).
- **Planner** (`plan_distributed_query`): splits the spec into per-host
  partials against the verse's fabric. Window queries target only shards
  whose `[start, end)` bucket range overlaps the window; `LatestPerAnchor`/
  `AllReadings` target every shard of the petal (an anchor's series can live
  in any bucket). Targets are capped at `MAX_TARGET_SHARDS` (256) and the
  plan is honest: a capped plan marks itself truncated, it never silently
  narrows the query.
- **Transport**: requests/partial responses ride the verse's own compute
  gossip topic (the F4 seam), as `ComputeEnvelope` frames
  (`encode_envelope`/`decode_envelope`) under `GOSSIP_ENVELOPE_BUDGET`
  (448 KiB against iroh's 512 KiB frame cap) — an over-budget partial is
  trimmed row-wise and flagged `truncated` in the meta, never dropped
  silently (F23 adds a no-progress guard: a single row larger than the whole
  budget makes the halving loop a no-op, so `encode_envelope` now BAILS with
  an error — the responder warns and drops the answer — instead of
  re-serializing an identical envelope forever). Correlation is by
  `request_id` through `PendingQueries` (`register`/`deregister`/`route`, cap
  `PENDING_QUERY_CAP` = 64). **A duplicate id is REFUSED, not overwritten
  (F23 fix):** `register` returns `false` and the live collector's inbox is
  never replaced (pre-F23 `map.insert` clobbered it, and the clobbered
  collector's later `deregister` would evict the replacement — unreachable
  with ULID request ids, but the doc claim and the code now agree). The
  collector runs under bounded concurrency (`MAX_CONCURRENT_QUERIES` = 8)
  with a per-request deadline (`DEFAULT_QUERY_TIMEOUT_MS` 3 s,
  `MAX_QUERY_TIMEOUT_MS` 10 s): partials that arrive after the deadline are
  routed and dropped; a request with zero fabric/shards is an honest empty
  plan, not an error.
- **Responder** (`handle_gossip_incoming`): a host that receives a request
  renders a partial with its own `RESPONDER_EXEC_CAP_MS` budget, replying as
  a topic broadcast (the topic is exactly the replica members — everyone
  else's `route` ignores foreign request ids).
- **Responder shard filtering is NOT implemented (F23 honesty pass).** A
  responder runs the shard list it was asked for verbatim — it does not
  narrow to shards IT hosts. Correctness does not depend on it: receive-side
  retention (§sharding) means a non-host holds no rows for a shard it does
  not host, so its partial for that shard is empty, and the merge's
  per-shard attribution (`fold_attribution`/`covered_by_answered_hosts`)
  only ever credits a shard to a peer in that shard's ledger host set. A
  non-host answering an aggregate request is therefore a harmless empty
  contribution (wasted DB work, no wrong data); the coverage metadata can
  never be widened by it. The earlier doc phrasing ("a host answers only for
  hosted shards") overstated the code — this is the correction. Wiring true
  responder-side filtering would be a pure efficiency win (skip a DB round
  trip for shards we do not host) and is a reasonable follow-up, not a
  correctness fix.
- **Commutative merges** (`merge_partials`): aggregates fold
  count/sum/min/max per (anchor, window) — mean is carried as
  (sum, count), computed only at the end, so partials merge in ANY order and
  duplicate mirror answers are idempotent; raw readings union-dedupe by
  `reading_id`; `LatestPerAnchor` keeps the max `recorded_at_ms`. The merge
  is order-free by construction — the unit tests pin three-host merged ==
  union ground truth, commutativity, and mirror-duplicate safety.
- **Honesty metadata (A16)**: the outcome's meta carries
  `covered_shards`/`missing_shards`/`answered_hosts`. A shard is covered
  when ANY answered host returned its partial (`covered_by_answered_hosts`);
  `attribute_rows_to_shards`/`fold_attribution` fold the answered shards
  back onto the planned ones so a host's over-retained copy of a shard it
  no longer plans still counts the shard as covered only where its rows
  actually are. An offline host at R=1 ⇒ its exclusive shard is `missing`
  (the merge never fabricates its data); at R=2 the surviving mirror answers
  and the shard is `covered`. A shard answered-but-empty is covered (the
  host honestly holds no rows in that window — absence of data is not
  absence of the shard). **A FAILED host is not a covered-empty (F23):**
  `TsPartialRows.failed` (the executing host's DB errored — set by the
  `ExecuteTsPartial` error arm in fe-database, serde-defaulted on the wire
  for back-compat) excludes that response from the rows, the coverage
  attribution, and `answered_hosts`, so the formal host lands in
  `missing_hosts` and its shards in `missing_shards`. Pre-F23 the error arm
  replied `empty()`, which the merge counted as an authoritative
  covered-empty — a silent data-loss masquerade the metadata now refuses.
- **Send-side targeted delivery — evaluated, NOT adopted.** The orchestrator
  asked whether the fan-out's request/response correlation could be reused
  for per-host transfer routing on the doc transport (F6's receive-side
  retention being the current enforcement). It does not fit naturally: doc
  writes are one-shot `Doc::set_bytes` fan-ins over the whole topic with no
  reply channel to correlate, a "targeted" doc send would still physically
  broadcast (the doc protocol has no direct-address mode), and threading a
  parallel per-host request/response fabric alongside the doc seam would
  duplicate the retention decision in two places that can drift. Receive-side
  retention (§sharding) stands as the transfer-routing correctness guarantee.
  Recorded honestly here + in the F7 handoff.
- End-to-end proof: harness scenario `distributed_query` (three peers over
  the real loopback transport — exact-merge vs ground truth, raw dedupe,
  latest-per-anchor, all-covered, creator-copy attribution, carol-exclusive
  shard missing after departure, and the R=2 surviving mirror serving a
  row-in-result) + fe-api `tests/distributed_query_test.rs` for the surface
  guards (A17).

### §transport-admission-control (F23, 2026-10-08)

The M2 scrutiny round-1 review flagged the sync-plane seams as riding the
verse's own doc/gossip namespaces without any admission control of their own.
The mission-wide stance is deny-by-default RBAC; the API/data surfaces carry
the approved A13–A17 guards, but the transport/ledger seams were unguarded.
This pass closes the cheap, seam-fitting holes. **Posture before this
feature:** any node that knew a verse's ULID (the gossip topic id is a public
function of the ULID — no capability) could publish compute requests and
ledger/declaration rows; the only checks were structural (request shape,
petal containment). **Posture after:** the checks below. Every drop returns a
named `GossipDisposition::Dropped(&'static str)` from `drop_reason`, so each
shape is unit-testable without a gossip stack.

`handle_gossip_incoming` (`distributed_query.rs`):

1. **Verse binding.** A request's envelope `verse_id` must equal the verse of
   the topic the message ARRIVED on (`incoming.verse_id`, set by the pump
   from the topic it drained). The arrival topic is the authenticated
   binding; the envelope field is an untrusted claim. Mismatch →
   `drop_reason::VERSE_MISMATCH`.
2. **Sender identity.** On a DIRECT delivery (`GossipIncoming.direct`, from
   `DeliveryScope::is_direct()` — neighbor broadcast or 0 swarm hops), the
   claimed `from_did` must equal `did_key(incoming.from)`. The F20
   endpoint-identity == fe-DID alignment (the iroh endpoint key and the app
   DID share the ed25519 seed) makes the authenticated sender's DID
   derivable, so forged attribution is impossible on direct deliveries:
   a covered-empty cannot masquerade as a formal host's answer, and a request
   cannot impersonate a declared peer. Applies to BOTH responses and requests
   → `drop_reason::FORGED_SENDER`. **Residual (honest):** a RELAYED delivery's
   `from` is the forwarding neighbor, not the original broadcaster (iroh-gossip
   0.35 `GossipEvent` docs), so relayed deliveries cannot be wire-verified and
   fall back to claim-based admission. Relays are therefore the residual hole
   for attribution; it is not closable without per-message signing
   (deferred — the causal-DAG membership resolver is blocked on the same
   per-operation signing, see §write-policy).
3. **Requester membership.** A requesting peer must be a DECLARED fabric peer
   of the arrival verse (`__peers/{did}` in that verse's `VerseFabric`)
   before we execute anything → `drop_reason::UNDECLARED_REQUESTER`.
   Deny-by-default for strangers on a topic id that is derivable from the
   public verse ULID. Honest about convergence: a responder that has not yet
   converged the requester's own declaration refuses until it does (the
   declaration rides the doc and converges like any row). Claim-based for
   relayed requests (the claim is what we look up); the identity gate (2)
   still binds the claim on direct deliveries. **This one was implemented**
   (not just evaluated) — it fits the seam naturally: the fabric already
   holds the peer set, and every sanctioned requester is a declared peer.

`VerseFabric::note_entry` (`sharding.rs`, ledger admission):

4. **`__peers` is self-declaration only.** A `__peers` row's key
   (`record_id` = `__peers/{did}`) must name its own author (`change.author_id`,
   the doc entry's signed author — never the row bytes). A row declaring
   ANOTHER peer's capacity (e.g. publishing `__peers/{honest_did}` with
   `capacity_bytes: 1` to starve that peer out of future placement) is
   dropped with a warn. The honest path is unaffected: replica open,
   `SetShardDeclaration`, and the restart snapshot all write the local
   peer's OWN key under its own author identity.
5. **`__shards` Editor+ gate — evaluated, NOT implemented (residual
   recorded for orchestrator review).** The requested gate needs the author's
   ROLE resolved at the verse scope. Roles are **DB-thread state** (the local
   `role` table — see fe-database §handlers / §rbac-policy; `admit_inbound_row`
   resolves them from local tables, never the wire), and `__shards`/`__peers`
   rows are consumed at the sync seam and never reach the DB thread — so the
   sync plane has NO role information to gate on, and the manifest doc row
   carries only `created_by`/`default_access`, not per-peer roles. No honest
   seam-side gate is constructible today. The weaker shape available (require
   the author to be a declared peer) does not close the hole — a declared
   peer can still publish a false ledger row (claiming hosts that do not hold
   the shard, or omitting itself) — so implementing it would be a gate that
   looks like protection without being one; per the feature's instruction
   ("do not force a broken gate"), it was NOT implemented. **Residual risk:**
   a declared fabric peer can inject a `__shards` ledger row that misdirects
   placement/retention. Impact is bounded: retention is receive-side
   (over-retention is idempotent under union semantics, a wrongly-dropped
   hosted row is the real loss), and the merge's per-shard attribution means
   a false host set can at worst make a shard look `missing`/`covered` —
   metadata honesty, not silent data corruption. The durable fix is a
   role-bearing ledger (roles ride the doc, or a signed shard-claim
   envelope), which is the same per-operation-signing dependency as the
   relayed-attribution residual above.

### §gossip-bootstrap — the Join-drop race (fixed 2026-10-08)

`subscribe_to_verse_gossip_topic` (sync_thread.rs) must run **before** the
open sequence, and it must register every bootstrap peer with
`Endpoint::add_node_addr` before `gossip.subscribe(topic, bootstrap)`:

- **Why `add_node_addr` first:** the one-shot `subscribe` dials the bootstrap
  peers immediately; a bare `NodeId` with no address-book entry cannot be
  dialed and the join silently targets nobody. Registering the addresses
  first makes the join dial succeed on the first try (the docs `start_sync`
  dials in the background and races this otherwise).
- **Why subscribe-before-open:** the gossip actor is an independent task and
  **DROPS a Join that arrives for a topic this node has not subscribed to
  yet** — the join is one-shot with no retry, so a joiner whose Join lands
  in that window is silently neighbor-less on the topic for the whole
  session (its compute request/response path is dead; the doc topic never
  hit this because iroh-docs subscribes its topic immediately at doc open).
  Before the fix, the verse topic subscribe ran AFTER the whole open
  sequence (doc dial, `start_sync`, seed reconciliation — milliseconds of
  racing surface); a captured `iroh_gossip=debug` log proved the drop: alice
  received bob's Join at `+0.350s` while her own `Command(Join([]))` was
  only processed at `+0.353s` — bob never became a neighbor and ~1-in-3
  harness runs lost his partial. Subscribing first makes the window
  microseconds against the joiners' millisecond dials; the harness scenario
  sequences the host's open 500 ms before the joiners' opens to make it
  scheduling-proof.
- **Residual, known and accepted:** a Join can still be dropped if it
  arrives between the `OpenVerseReplica` command's arrival and the subscribe
  call (thread starvation). A self-healing join-retry monitor
  (`GossipTopic::joined()` + re-issue) is the follow-up shape if that window
  ever matters in production.
