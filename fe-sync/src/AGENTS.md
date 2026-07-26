# fe-sync — module notes

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
- Broadcast: `GossipTopic::broadcast(Bytes).await` (now async — the transform/tileset
  advertise handlers are therefore `async`).

Deferred: no `iroh::protocol::Router` is registered for `GOSSIP_ALPN`, so **inbound** gossip
connections are not yet routed. Outbound `broadcast` is best-effort (messages queue until a
neighbor is available). Wiring the Router + inbound peer handling is a follow-up.

### iroh-docs 0.35 — deferred (mock-backed)

The old single-arg `iroh_docs::Engine::new(endpoint)` / `iroh_docs::Document` surface is gone.
0.35 exposes `iroh_docs::engine::Engine<D: iroh_blobs::store::Store>::spawn(endpoint, gossip,
replica_store, bao_store, downloader, default_author_storage, local_pool)` and a docs RPC client
`iroh_docs::rpc::client::docs::Doc`. Standing that up requires the **full P2P stack** (a blob
store, a `Downloader`, a `LocalPoolHandle`, and the gossip instance) — out of scope for this pass.

Until then:

- `IrohDocsEngineHolder::is_available()` is always `false`.
- `IrohDocsReplicator` / `IrohPetalReplicator` are backed by the in-memory `MockVerseReplicator`.
- Real wiring points are marked `// TODO(iroh-0.35):` in `replicator.rs` and `sync_thread.rs`.

`status.rs` also carries a `TODO(iroh-0.35)` for applying inbound peer `SyncEvent::NodeTransformed`
to the local world (currently logged, not applied) — it depends on the inbound gossip route above.

Runtime behavior is therefore unchanged from the pre-checkpoint WIP intent: fully mock-backed,
network paths are no-ops when offline (which is every current test path).

## §write-policy (auth_policy_pattern_20260710 §D1)

`write_policy.rs` gates `handle_write_row_entry` (sync_thread.rs): no row is
applied to a verse replica without a `Policy::evaluate` decision. Today the
sync thread has **no role data plumbed** (roles live in the DB thread), so the
default `PolicyHandle::permissive_migration()` wraps the strict
`RoleLevelPolicy` in `fe_policy::PermissiveMigrationPolicy` — every would-be
denial is logged at `warn` and allowed. Enforcement flips by swapping in
`PolicyHandle::strict()` once role plumbing lands (roles must reach the sync
thread before real iroh-docs replication ships — decisions §D1/§D5).
`PolicyHandle` derives `Resource` so the app side can insert/override it;
`spawn_sync_thread` constructs the default internally to avoid churning its
signature (3 call sites). The causal-DAG membership resolver from the §D1
amendment is NOT here — blocked on per-op ed25519 signing (hexon_delta_format).

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
