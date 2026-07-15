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
