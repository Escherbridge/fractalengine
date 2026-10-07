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
  `Doc::close`. Mock fallback is now **offline-only** (bind failure, stack
  spawn failure, unusable capability — loud, never a crash). `start_sync`
  runs on every doc-backed open, with or without outbound peers: the
  dialed side must have its sync task running to serve entries.

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
point (see fe-database `handlers/replicated_row.rs`); the role gate is F3's
A3 seam.

Bootstrap peers (A9 prep): `FE_SYNC_BOOTSTRAP` holds **semicolon-separated**
`NodeAddr` JSON entries (semicolons because the JSON contains commas), parsed
with iroh's own types; bare `NodeId` hex carries no address and is skipped
loudly. `OpenVerseReplica.bootstrap_peers` carries the same entry form
per-verse and merges with the env set (deduped); `SyncEvent::Started.node_addr`
emits our own dialable `NodeAddr` JSON in exactly that form.

## §write-policy (auth_policy_pattern_20260710 §D1)

**F2 moved the gate.** `write_policy.rs` no longer gates
`handle_write_row_entry`: those commands are the *outbound* path — a local,
already-admitted DB write being published to peers — so gating there only
blocked our own publishes (A3's "outbound local DB-thread writes are no
longer denied"). `PolicyHandle` and its evaluation logic remain (unit-tested
in `write_policy.rs`) and are the intended building block for the **inbound**
admission gate: peer admission happens on the DB thread
(`DbCommand::ApplyReplicatedRow` → fe-policy deny-by-default, Editor+,
roles resolved from the `role` table at verse scope, never wire-supplied) —
F3's A3 seam, marked `TODO(F3/A3)` in fe-database
`handlers/replicated_row.rs`.

`PolicyHandle` derives `Resource` so the app side can insert a stricter policy
for tests or future admission work. The causal-DAG membership resolver is not
here; it remains blocked on per-operation signing.

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
