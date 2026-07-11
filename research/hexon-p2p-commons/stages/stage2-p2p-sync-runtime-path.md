---
type: research-findings
stage: 2
date: 2026-07-11
---

# Stage 2: P2P / Sync / Replication / Runtime Data Path — Bottlenecks and Reality Check

## 0. Summary Position

FractalEngine's P2P claim rests on three pillars: iroh 0.35 (transport + gossip + docs),
a 7-thread crossbeam-bridged runtime, and a permission model keyed on iroh-docs
namespace secrets. Of these: **transport (QUIC endpoint) is real, gossip
(iroh-gossip) is real and wired for ephemeral transform broadcast, but the CRDT
replication layer that is supposed to carry authoritative Verse/Petal state
(iroh-docs) is 100% mocked** behind a trait that always reports itself
unavailable. The permission model has no distributed enforcement or revocation
propagation. Two separate networking stacks exist (iroh + libp2p) with no
integration between them. This document extends `research/p2p-mycelium/findings.md`
and `research/cross-track-alignment/report.md` with code-verified specifics.

---

## 1. fe-sync: what's actually wired vs mocked

### 1.1 Blobs — NOT wired
`fe-sync/src/sync_thread.rs::handle_fetch_blob` (lines 255-282) only checks the
local `BlobStoreHandle`. On a miss it computes the gossip topic hash for
logging and emits **nothing** — no iroh-blobs `Downloader`, no peer query, no
retry. The doc comment admits this: *"Would fetch blob from peers (stub — peer
discovery not yet implemented)."* Confidence: **HIGH (verified in code)**.

### 1.2 Docs (CRDT row replication) — MOCKED
`fe-sync/src/replicator.rs`:
- `IrohDocsEngineHolder::is_available()` (line 235) always returns `false` — it
  reads an `AtomicBool` that nothing in the codebase ever sets to `true`.
- `IrohDocsReplicator::write_row`/`subscribe`/`close` (lines 286-304) delegate
  unconditionally to an internal `MockVerseReplicator` (`inner` field), which
  is a `Mutex<HashMap<String, Vec<u8>>>` plus a `Vec<mpsc::Sender>` for
  fan-out — **process-local only, no network I/O, no persistence**.
- `IrohPetalReplicator` (petal-scoped variant, lines 344-414) is architecturally
  identical: same `MockVerseReplicator` inner field, same non-networked
  behavior.
- Memory's claim "IrohPetalReplicator wraps MockVerseReplicator" is **verified
  correct**, and generalizes to `IrohDocsReplicator` too.

**What "mock-backed" means for the resilience claim:** two peers on the same
verse today do not actually exchange rows via iroh-docs at all. Each peer's
"replica" is an isolated in-memory map that only observes its own local
writes. There is currently no code path that transmits a `RowChange` across
process boundaries for verse-level state. The entire CRDT-convergence/LWW
story (§9 of the prior research) is validated only against the mock, not
against real network conditions — set reconciliation, redb persistence, and
gossip-fed live sync from iroh-docs are all unexercised. Confidence: **HIGH**.

### 1.3 Gossip — REAL, but scoped narrowly
`fe-sync/src/sync_thread.rs` lines 79-95 construct a genuine
`iroh_gossip::net::Gossip` bound to the real iroh endpoint. Verse-scoped
topics are derived deterministically (`derive_gossip_topic`,
`gossip_topic_id` — blake3 hash of `"verse:{id}"`), and
`handle_update_node_transform` (lines 481-543) broadcasts real
`TransformUpdate` payloads over `topic.broadcast()`. This is genuinely wired
and tested (`fe-sync/src/sync_thread.rs` tests around line 933+).

**But**: gossip is used *only* for ephemeral, best-effort transform updates
(position/rotation/scale deltas for live drag interactions), not for
authoritative row state. Even received `NodeTransformed` events are logged and
discarded — `fe-sync/src/status.rs::drain_sync_events` line 88 has a TODO:
*"apply peer NodeTransformed to the local ECS/world"* — meaning **incoming
peer transform gossip currently never reaches the Bevy scene**. Confidence:
**HIGH**.

### 1.4 Peer presence — stub confirmed, worse than described
`fe-sync/src/verse_peers.rs`'s doc comment claims `VersePeers` "is now
populated from `SyncEvent::PeerConnected`/`PeerDisconnected` events... it is
now fully functional." This is **stale/aspirational documentation**: grep
across the whole `fe-sync` crate shows `SyncEvent::PeerConnected` and
`PeerDisconnected` are constructed **only inside unit tests**
(`messages.rs` lines 247-291). No code in `sync_thread.rs`'s command loop —
which is the only place with access to iroh connection events — ever
constructs or sends these variants. `status.rs::drain_sync_events` (lines
64-71) is ready to consume them and increments/decrements `peer_count`, but
nothing produces the input. **BLOCKER 1 from cross-track-alignment is not
resolved**; the `VersePeers` resource stays permanently empty in practice,
just as the original stub was. Confidence: **HIGH (verified via exhaustive
grep — zero non-test call sites)**.

---

## 2. Threading + channels: end-to-end trace and backpressure analysis

### 2.1 Topology (verified in `fractalengine/src/main.rs`)
- `CHANNEL_BUFFER = 256` (`fe-runtime/src/channels.rs:4`) applies to
  net/db/api command+event channels.
- Sync command/event channels are separately declared `bounded(256)`
  (`main.rs:98-99`).
- DB→sync replication bridge: `bounded::<ReplicationEvent>(256)`
  (`main.rs:69`).
- Scene-change and inbound-transform bridges: `bounded(256)` each, both
  bridging a `tokio::broadcast` to a `crossbeam::bounded` channel.

### 2.2 Peer-originated update path (as coded today)
Real path for an in-verse gossip transform update:
`iroh-gossip network → topic.recv() [NOT IMPLEMENTED — no receive loop exists
in sync_thread.rs] → ... → drain_sync_events (if it existed) → Bevy ECS`.

**Finding: there is no gossip receive loop.** `sync_thread.rs` subscribes to
topics and *broadcasts* (`topic.broadcast()`), but never calls
`topic.next()`/awaits inbound gossip events in the command `loop`. The
`SyncEvent::NodeTransformed` variant exists and is consumed by
`drain_sync_events`, implying a receive loop was planned, but it is absent
from the `tokio::select!`-free `loop { match cmd_rx.recv() { ... } }` in
`spawn_sync_thread` (lines 110-232) — that loop only reacts to local
`SyncCommand`s, never to the gossip stream. **A peer's transform broadcast is
therefore currently unreceivable by other peers in this codebase, despite the
send side being real.** Confidence: **HIGH (verified — no `GossipEvent`
handling anywhere in sync_thread.rs)**.

For **row-level updates** (CREATE/UPDATE via DB), the intended path is:
`fe-database write handler → replicate_row_with_petal() [main.rs:69 bridge]
→ ReplicationEvent (bounded 256, blocking send, fe-database/src/lib.rs:155)
→ bridge thread (main.rs:111-122) → SyncCommand::WriteRowEntry (blocking
send, main.rs:114) → sync thread → IrohDocsReplicator::write_row → mock
HashMap (never leaves process)`.

### 2.3 Backpressure mechanism — the critical finding
`fe-database/src/lib.rs::replicate_row_with_petal` (line 155) calls
`tx.send(ReplicationEvent{...})` — a **blocking** crossbeam send — directly
inside the DB thread's own write-handler code path (called from
`create_verse_handler`, etc. per `lib.rs:308,314`). The bridge thread that
drains this channel (`main.rs:111-122`) also uses a **blocking** `.send()`
into `SyncCommand::WriteRowEntry`.

This means: **DB thread write throughput is now coupled to sync-thread
consumption speed**, with two bounded(256) hops in series and zero
`try_send`/drop semantics on either hop. Contrast with the *other* two
bridges in the same file (`scene_change_tx_bevy`, `inbound_tx`,
lines 254 and 297) which correctly use `try_send` + log-and-drop on
`Full`. The replication path is the one hop in the whole system that can
convert a slow/stalled sync thread into a **frozen database** — every
`CREATE`/`UPDATE` blocks once the 256-slot buffer fills. Confidence:
**HIGH (verified in code — three call sites)**.

Mechanism for a stall: sync thread awaits `topic.broadcast(payload).await`
(an async iroh-gossip call) inside its single-threaded Tokio runtime, or is
blocked handling a slow blob read (`std::fs::read` synchronously inside
`handle_write_row_entry`, `sync_thread.rs:377` — a **sync filesystem call on
the async runtime's only thread**, which can itself stall all other queued
sync-thread work, including draining `cmd_rx`). Any one of these stalls
back-pressures the crossbeam channel, which back-pressures the bridge
thread's blocking send, which back-pressures `ReplicationEvent`'s channel,
which blocks the DB thread mid-transaction-handler.

### 2.4 Burst-of-10k scenario
With `bounded(256)` and blocking sends at both hops: a burst of 10,000
node updates (e.g., bulk import, or a terrain layer re-tessellation writing
many nodes) will fill the 256-slot `ReplicationEvent` channel within
~256 writes, after which **every subsequent DB CREATE/UPDATE call in the
handler blocks the DB thread** until the sync thread drains entries. Since
`write_row` on the mock replicator is fast (in-memory `HashMap::insert`),
in the *current* (mocked) system this self-heals quickly — but this
also means **the mocked backend is currently masking a real production
bottleneck**: once real iroh-docs writes (network I/O, disk-backed
redb, possibly awaiting peer acks under "live sync") replace the mock,
the same code path will block DB writes for the duration of network
round-trips, with no escape valve. Confidence: **MEDIUM (mechanism
verified in code; magnitude is inferred, not measured — no benchmark
harness exists for this path)**.

### 2.5 Render-thread coupling
`sync_thread.rs:377` does a **synchronous** `std::fs::read` inside an
async fn running on a `current_thread` Tokio runtime — this blocks that
runtime's only worker for the duration of the read, delaying gossip
broadcasts and command processing that share the same thread. This is a
minor but real latency source under any disk pressure (spinning disk,
network filesystem, or many small blobs). Confidence: **HIGH**.

---

## 3. fe-network (libp2p 0.56): parallel, disconnected stack

`fe-network/src/lib.rs` and `swarm.rs` show a **second, independent**
networking stack:
- Own identity: `libp2p::identity::Keypair::generate_ed25519()`
  (`lib.rs:29`) — **not** derived from `fe-identity::NodeKeypair` or the
  iroh `SecretKey`. Three unrelated identities can exist per node (iroh
  node key, libp2p peer key, `fe-identity` DID key) unless something
  external unifies them (nothing does, per grep).
- `FractalBehaviour` (`swarm.rs:7-10`) wires only Kademlia DHT
  (`kad::Behaviour<MemoryStore>`) — no gossipsub, no identify, no ping
  behavior beyond the manual `NetworkCommand::Ping`/`Pong` in `lib.rs`.
- The command loop (`lib.rs:46-60`) only handles `Ping`/`Shutdown` and logs
  swarm events at `debug!` — **no application data ever flows over
  libp2p**. It exists, builds, and discovers peers via Kademlia bootstrap
  (`discovery.rs`), but nothing consumes discovered peers.
- `fe-network` also contains `iroh_blobs.rs` and `iroh_docs.rs` modules —
  i.e. fe-network *also* has iroh-adjacent code, separate from fe-sync's
  iroh usage. This is a duplicated/parallel integration surface: iroh
  wiring exists in two crates (`fe-network` and `fe-sync`) that do not
  share state.

**Duplicated transports, unused DHT:** libp2p's Kademlia (structured
overlay DHT, designed for public global discovery) coexists with iroh's
relay+QUIC (direct-first, NAT-punching via n0's relay infra) with no
observed handoff. Given the P2P Mycelium decision to avoid a global DHT
(explicit in prior research: "No global DHT — we are NOT building a
public discovery service"), the Kademlia behavior in `fe-network` appears
to be **vestigial/pre-decision scaffolding** that was never removed after
the architecture pivoted to iroh-docs/gossip. Confidence: **MEDIUM**
(structurally verified; intent/history inferred from doc trail).

**NAT traversal story:** iroh's `Endpoint::builder().bind()`
(`fe-sync/src/endpoint.rs:19-24`) uses iroh's default relay set (n0's
public relay infrastructure) for direct-connection establishment and
hole-punching fallback — this is the *only* real NAT traversal path in
the system. `fractalengine-relay`/`docker/Dockerfile.relay` runs the
`fe-relay` binary, which is the **headless API+DB+sync host** (exposes
port 8765 HTTP only) — it is not documented or wired as an iroh relay
node (no iroh relay-server crate/feature present); it looks like an
always-on peer, not a relay-protocol hop. This means the project has no
self-hosted iroh relay — it depends entirely on n0's public relay
servers for peers behind symmetric NAT. Confidence: **MEDIUM** (absence
verified via Dockerfile + crate inspection; could not find an iroh
relay-server feature anywhere in the workspace).

---

## 4. Asset/blob fetch path (GLB/tile)

Real, verified chain:
1. Bevy requests `blob://{hash}.glb` → `BlobAssetReader::read`
   (`fe-runtime/src/bevy_blob_reader.rs:78-108`).
2. On local hit: `std::fs::read` synchronously inside the async `read`
   (blocking the Bevy asset-loading task pool thread it runs on — not the
   render/main thread directly, since Bevy's `AssetReader` trait methods
   run on IO task pool, but see caveat below).
3. On miss: fires `on_miss` callback → `SyncCommand::FetchBlob` sent with
   `verse_id: String::new()` (empty!) because, per the comment at
   `main.rs:129-130`, *"We don't know the verse_id at the asset-reader
   level... Phase F will route through VersePeers instead"* — this phase
   never happened.
4. Sync thread's `handle_fetch_blob` (as in §1.1) does nothing but log.
5. **The asset never arrives.** The Bevy load remains `NotFound`
   permanently; there is no retry/backoff/re-request mechanism observed.
6. Separately, `fe-renderer/src/loader.rs::load_to_bevy` (a different,
   older/parallel asset-loading path used by fe-hexon ingestion) falls
   back explicitly to `assets/placeholder.glb` on a local cache miss,
   with a comment: *"CROSS-CRATE: fe_network::iroh_blobs::fetch_asset —
   wired in Sprint 5B"* — **this integration point does not exist**;
   grepping `fe_network::iroh_blobs` shows the module exists
   (`fe-network/src/iroh_blobs.rs`) but nothing calls its fetch function
   from `loader.rs`. Two independent "TODO: wire real fetch" dead ends
   exist in two different crates for two different asset-loading paths.

Confidence: **HIGH** for all of the above (verified via direct code read).

**fe-hexon's actual P2P fetch layer** (`fe-hexon/src/p2p/fetch.rs`) is
more complete than fe-sync's: `FetchStrategy` has a semaphore-bounded
concurrent-download limiter, peer prioritization (LAN > LowLatency >
Remote), and manifest signature verification (`verify_fetched_manifest`).
But it operates on **hexon package manifests/crates**, a different
concept from the live Bevy asset (GLB/tile) hot-path described above —
it is not what backs `BlobAssetReader`'s on-miss path. So the P2P asset
story is split across two unconnected subsystems: one path (fe-sync,
live scene assets) is a total stub; the other (fe-hexon, package
distribution) has real fetch-strategy logic but (per §5) no permission
enforcement.

---

## 5. fe-api gateway + fe-hexon registry: federation seams

### 5.1 fe-api — real RBAC, HTTP-scoped only
`fe-api/src/assets.rs` shows genuine, tested RBAC: `require_role`/
`require_scope` gate every asset-serving endpoint, resolving scope via
the node's parent-chain (`resolve_node_scope`) before serving bytes. This
is a solid, JWT-claims-based (`ApiClaims`) authorization layer — but it
only covers the **HTTP API surface**, not the P2P sync/replication path.
Confidence: **HIGH**.

### 5.2 fe-hexon registry/P2P — no RBAC found
Exhaustive grep across `fe-hexon/src` for RBAC/role/permission/policy
terms returns **zero matches** outside of manifest *signature*
verification. Signature verification proves authorship/integrity
(the manifest was signed by the claimed `publisher_did`), not
*authorization* (whether that publisher/fetcher is allowed to
read/write in this context). This is the Phase 8.4 "RBAC not enforced in
fe-hexon" gap, confirmed structurally: **there is no policy-pattern
`evaluate(subject, action, resource)` function anywhere in the fe-hexon
or fe-auth crates** — grepping fe-auth for `evaluate`/`Policy`/
`deny_by_default` also returns nothing. The platform-vision's
"policy-pattern auth" primitive does not yet exist in code anywhere in
the workspace. Confidence: **HIGH**.

**Implication for "self-permissioned":** in practice, permission today
means one thing only: possession of the iroh-docs `namespace_secret`
(verse-level) grants unrestricted write to every table/record in that
namespace (`handle_write_row_entry`, `sync_thread.rs:345-397`, applies
`repl.write_row(table, record_id, &data)` with no per-table/record
check). There is no petal-level or role-level (Owner/Manager/Editor/
Viewer) enforcement on the replication write path — RBAC (`role` table,
`role_manager.rs`) is real and used by the **HTTP API**, but is not
consulted anywhere in `fe-sync`. A peer who has the namespace secret (via
an accepted invite) can write anything to any table in that verse via
the sync thread, regardless of their assigned `role`. Confidence:
**HIGH (verified — no rbac/role_manager import in fe-sync crate)**.

### 5.3 Revocation — local-only, does not propagate
`fe-auth/src/revocation.rs::revoke_session` writes an op-log entry and
updates the **local** `SessionCache` only. Line 22's comment: *"CROSS-CRATE:
send NetworkCommand::BroadcastRevocation to network thread — deferred
Sprint 5B."* This mirrors the `fetch_asset` dead reference exactly — a
second unimplemented "Sprint 5B" cross-crate hook. **Revocation on one
node does not inform any other peer.** A revoked peer retains full
`namespace_secret` write capability on every other peer's replica until
each peer independently and manually revokes them (there's no mechanism
even for that at the replica layer — `namespace_secret` itself isn't
rotated on revoke). Confidence: **HIGH**.

### 5.4 fe-hexon registry (HTTP) — separate from P2P fetch
`fe-hexon-registry` (per project memory) is a hosted HTTP registry
service mirroring the relay container pattern; `fe-hexon`'s `remote`
feature provides `RemoteRegistryClient`. This is a **centralized fallback
seam** — reasonable and honestly centralized per the research bias
warning ("some problems really are easier with one well-known peer") —
but its RBAC posture inherits the same gap noted above unless the
registry service itself enforces auth independently (not verified in
this pass; recommend as a Stage 3/5 follow-up to check
`fe-hexon-registry`'s own auth middleware).

---

## 6. Peer presence, reconnection, partition recovery, cold-start cost

- **Presence**: confirmed non-functional (§1.4). `SyncStatus.peer_count`
  can only ever be 0 in the current build; UI elements depending on
  peer-online indicators (Inspector Access tab, Profile widget per
  cross-track-alignment) have no real data source yet, despite the doc
  comment claiming otherwise.
- **Reconnection**: no reconnect/retry logic found in `sync_thread.rs`
  for the iroh endpoint itself; if `SyncEndpoint::new` fails at startup
  the thread runs permanently in offline mode for the process lifetime
  (no periodic re-bind attempt observed).
- **Partition recovery**: impossible to characterize meaningfully today
  because there is no real replica sync to partition — the mock
  replicator has no persistence and no reconciliation protocol. Once
  iroh-docs is wired, its range-based set reconciliation (per prior
  research) would handle this, but zero code exists yet to test against.
- **Cold-start cost for a new peer joining a large verse**: cannot be
  measured (no real sync path), but structurally: `OpenVerseReplica`
  creates a replicator and immediately calls `open_document()` (a no-op
  today) — there is no bulk-catchup/initial-sync step coded at all. When
  iroh-docs is wired, the honest expectation (per prior research and
  iroh-docs' design) is O(existing entries) for the initial range
  reconciliation handshake, likely dominated by blob-fetch bandwidth for
  all historical row-JSON blobs plus GLB/tile assets referenced by the
  verse — this could be substantial for a large, long-lived verse with
  no snapshot/checkpoint mechanism observed anywhere in the codebase.
  Confidence: **LOW (no measurement possible; reasoning from iroh-docs
  design docs only)**.

---

## 7. Scaling estimates at 10/100/1000 peers

Given the current code (mocked CRDT layer, no presence, no gossip
receive loop, blocking replication bridge):

- **10 peers**: Today's actual behavior — each peer is an island; no
  cross-peer state exchange happens at all via fe-sync (gossip send-only,
  docs mocked). "10 peers online" produces 10 independent local worlds
  that silently diverge with zero indication to users (no presence UI
  data, no conflict signal). This is not a performance problem yet — it's
  a correctness/functionality gap that performance testing would not even
  surface.
- **100 peers** (once iroh-docs is genuinely wired, projecting from
  architecture): gossip fan-out for transform updates is
  probabilistic pub/sub — proven to scale reasonably at this size per
  iroh-gossip's design (HyParView/Plumtree family), but the *missing
  receive loop* (§2.2) means 100 peers broadcasting is still 100 peers
  broadcasting into the void today. Once fixed, the DB-thread blocking
  chain (§2.3) becomes the binding constraint: 100 peers' worth of
  concurrent row writes funneled through one `bounded(256)` channel with
  blocking sends risks periodic DB-thread stalls under any write burst
  (e.g., 100 peers each importing a terrain layer simultaneously).
- **1000 peers**: iroh-gossip's own scaling ceiling (not evaluated here;
  Stage 3 external research territory) likely becomes relevant, but
  locally, the single-namespace-per-verse model means 1000 peers on one
  verse implies 1000-way fan-out through a *single* sync thread with a
  single-threaded Tokio runtime handling all gossip topics, all
  replicas, and all blob reads serially — this is a single-core
  bottleneck by construction (`Builder::new_current_thread()` at
  `sync_thread.rs:49`). No sharding, no per-verse thread pool. This is
  architecturally the most concrete scaling wall found: **the entire
  sync subsystem for a node runs on exactly one OS thread**, regardless
  of peer count or verse count. Confidence: **HIGH that this is a hard
  single-thread bottleneck by construction; MEDIUM on where exactly it
  saturates without a benchmark**.

---

## 8. Findings Summary

### Performance bottlenecks (mechanism included)
1. **Single-threaded sync runtime** — `sync_thread.rs:49`
   (`current_thread` Tokio runtime) serializes all gossip, replica, and
   blob I/O for a node across every verse/petal it participates in.
2. **Blocking replication bridge** — `fe-database/src/lib.rs:155` and
   `main.rs:114` both use blocking `.send()` on `bounded(256)` channels
   in series; a stalled sync thread freezes the DB thread's write path.
   Contrast with two sibling bridges in the same file that correctly use
   `try_send`+drop.
3. **Synchronous filesystem read on the async sync-thread runtime**
   (`sync_thread.rs:377`, `handle_write_row_entry`) — blocks gossip and
   command processing during any blob read.

### Real-vs-mocked gaps (resilience claim)
1. **iroh-docs CRDT replication is entirely mocked** — `IrohDocsReplicator`
   and `IrohPetalReplicator` both wrap `MockVerseReplicator`
   (in-memory-only, no network I/O). The engine-availability flag is
   hardcoded `false`.
2. **No gossip receive loop** — the sync thread only ever broadcasts;
   it never listens for inbound `GossipEvent`s, so even the one real
   piece (gossip send) is currently a one-way transmitter with no
   receiver, and applying incoming peer transforms to the Bevy scene is
   an explicit unimplemented TODO.
3. **Peer presence stub, worse than documented** — `VersePeers`'
   own doc comment claims it's "fully functional," but `PeerConnected`/
   `PeerDisconnected` are constructed only in unit tests; nothing in
   the runtime command loop ever emits them.
4. **Two unimplemented "Sprint 5B" cross-crate hooks** with identical
   shape: `fe_network::iroh_blobs::fetch_asset` (asset fetch) and
   `NetworkCommand::BroadcastRevocation` (revocation propagation) — both
   referenced by comment, neither wired.
5. **Two disconnected networking stacks** — `fe-network` (libp2p 0.56,
   Kademlia DHT, separate identity) carries zero application traffic;
   `fe-sync` (iroh 0.35) carries gossip only. No integration, no shared
   peer/identity model between them.

### Permission-model gaps
1. **No enforcement on the sync/replication write path** — RBAC
   (`role_manager`) exists and is used by the HTTP API (`fe-api`) but is
   never consulted in `fe-sync`; possession of `namespace_secret` implies
   unrestricted write to every table/record in the verse.
2. **fe-hexon has no RBAC/policy layer at all** — only manifest-signature
   (authenticity) checks exist; no `evaluate(subject, action, resource)`
   policy function exists anywhere in the workspace yet.
3. **Revocation is local-only** and does not propagate to peers or rotate
   the namespace secret — a revoked peer keeps full write capability
   against every peer that hasn't independently and manually revoked it.

### Candidate mitigations
1. **Unblock the replication bridge**: convert `fe-database/src/lib.rs`'s
   `replicate_row_with_petal` and the `main.rs` DB→sync bridge to
   `try_send` + bounded local retry queue (matching the pattern already
   used correctly for scene-change/inbound-transform bridges), so a
   stalled sync thread degrades to "replication lag" rather than
   "database freeze." Pair with a metrics counter on drops so lag is
   observable, not silent.
2. **Wire the real iroh-docs `Engine<D>` and a gossip receive loop**
   before any further resilience claims are made; until then, explicitly
   document (in user-facing terms) that multi-peer convergence is
   aspirational, not implemented, to avoid overselling "fully
   distributed" during this window. On the permission side, gate
   `handle_write_row_entry` behind a policy-pattern
   `evaluate(peer_did, Action::Write, Resource::Table(table))` call
   backed by the existing `role` table — this closes the
   Phase-8.4-flagged gap with a mechanism that already exists
   (role_manager) rather than a new subsystem, and gives revocation
   something real to disable (deny at evaluate-time) even before
   cross-peer revocation broadcast is built.

---

## Confidence Summary Table

| Claim | Confidence | Evidence type |
|---|---|---|
| iroh-docs replication is fully mocked | HIGH | Verified in code |
| Gossip send path is real, receive path absent | HIGH | Verified in code |
| VersePeers presence never populated at runtime | HIGH | Exhaustive grep, zero non-test producers |
| DB thread can freeze under sync backpressure | HIGH (mechanism) / MEDIUM (magnitude) | Verified code path; no load test exists |
| fe-network (libp2p) carries no application traffic | HIGH | Verified in code |
| No RBAC enforcement on sync write path | HIGH | Verified — no rbac import in fe-sync |
| fe-hexon has no policy/RBAC layer | HIGH | Exhaustive grep, zero matches |
| Revocation does not propagate to peers | HIGH | Verified — explicit deferred comment |
| Single-thread sync runtime is a hard scaling ceiling | HIGH (structural) / MEDIUM (saturation point) | Verified `current_thread` runtime; no benchmark |
| Cold-start sync cost for new peer in large verse | LOW | No real sync path exists to measure |
