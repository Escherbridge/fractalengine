# fe-sync — module notes

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
