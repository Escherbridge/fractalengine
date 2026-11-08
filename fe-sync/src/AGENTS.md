# fe-sync — module notes

## §blob-store (default path, corrected OF-1/S1b 2026-10-14)

`FsBlobStore::default_path()` is **per-install**, co-located with that
install's own DB: `data/blobs` (CWD-relative; the GUI binary's
`set_current_dir(packaged_root)` lands it beside `data/fractalengine.db`),
overridable by `FE_BLOB_DIR` — same override-env-first precedence as the
relay's `FE_DB_PATH`/`FE_P2P_DIR`. It was previously **per-user**
(`{data_local_dir}/fractalengine/blobs`), which every install on one machine
shared; asset GC's "my DB doesn't name this hash" check (fe-database
src/AGENTS.md §handlers asset_gc) is only correct when the store it operates
on is exclusive to that DB, so a shared store let install A's GC delete a
blob install B's DB still referenced. The old expression survives only as
`legacy_default_path()`, read once by
`fe_database::handlers::blob_migration::migrate_legacy_blob_store` at startup
to copy forward (never move or delete) anything a DB still names. Both
`fe_sync::FsBlobStore` and `fe_hexon::registry::FsBlobStore::default_path()`
(the independently-maintained duplicate, fe-hexon src/AGENTS.md §blob-store)
carry identical `FE_BLOB_DIR`/`data/blobs` logic — covered by
`fe-test-harness/tests/blob_store_cross_read.rs`'s
`default_path_agrees_between_fe_sync_and_fe_hexon` so the two never
re-diverge.

**Relay co-location (W2b F5, 2026-10-14):** unlike the GUI binary's hardcoded
`DB_PATH`, the relay's DB path is operator-configurable (`FE_DB_PATH`), so
`FE_DB_PATH` diverging from `data/` without `FE_BLOB_DIR` set would silently
un-co-locate the blob store. `fractalengine-relay/src/main.rs::main` now
derives `FE_BLOB_DIR` from `FE_DB_PATH`'s parent (`derive_blob_dir_from_db_path`,
a pure fn, table-tested) and `set_var`s it before any thread starts or any
`default_path()` call — both the fe-sync one above and the fe-hexon duplicate
read `FE_BLOB_DIR` once, so this must win the race, never follow it.

## §blob-store-directory-initialization

`FsBlobStore` evaluates `create_dir_all` by its required postcondition: an error
is harmless when the target now resolves to a directory. This guards the
observed Windows error 183 case, whose `io::ErrorKind` was not `AlreadyExists`
despite the OS message, while still rejecting a file or missing target at the
blob-store path. The same helper protects root initialization and per-hash
shard creation.

## §congestion-control — explicit BBR (track p2p_unblock_now_20260711 FR-4)

QUIC's default congestion controller is **CUBIC** — on iroh 0.35
(`iroh-quinn-proto` `TransportConfig::default()`) and still on iroh 1.x
(noq-proto 1.3 `TransportConfig::default()` sets `CubicConfig`). Per
iroh#4286, iroh-blobs throughput differs ~30x between BBR (~40% of link) and
CUBIC (~1-1.5%), so `endpoint.rs::bbr_transport_config()` explicitly selects
BBR:

- iroh 1.x exposes the knob as `iroh::endpoint::QuicTransportConfig::builder()
  .congestion_controller_factory(..).build()`, handed to
  `Endpoint::builder(..).transport_config(..)`. The concrete controller is NOT
  re-exported by iroh (only the `Controller`/`ControllerFactory` traits), so
  fe-sync depends directly on **`noq-proto = "1.3"`** — the quinn fork iroh
  1.3 itself depends on (`noq`/`noq-proto ^1.3.0`), so the trait objects
  unify. **Lockstep rule:** every iroh bump must re-check that the
  workspace `noq-proto` resolves to the same version iroh uses (a mismatch is
  a type error at `congestion_controller_factory`, caught by the
  `bbr_transport_config_constructs` test).
- noq 1.0 replaced quinn's BBR (v1) with **BBRv3** (`noq_proto::congestion::
  Bbr3Config`); that is what fe-sync selects now. The startup log reads
  `congestion_controller = "bbr3"`.
- `QuicTransportConfig::builder()` starts from iroh's own multipath-tuned
  defaults (keep-alive, path keep-alive/idle timeouts, max paths), so the
  0.35-era hack that re-applied `keep_alive_interval(1s)` is gone — only the
  congestion controller is overridden.
- Startup logging: `SyncEndpoint::new` logs the controller at info on a
  successful bind (there is no endpoint-level "active controller" getter;
  the config is authoritative).

## §sync-thread-blocking-io

The sync thread runs a **current-thread** tokio runtime; any synchronous
syscall inside the async block stalls every queued gossip/replica/blob task.
`handle_write_row_entry` therefore reads blob files via
`tokio::task::spawn_blocking` (track p2p_unblock_now_20260711 FR-3). Keep new
filesystem or other blocking calls in this file off the runtime the same way.

## §iroh-1.x — the P2P stack on iroh 1.3 (ported from 0.35, 2026-10-11)

fe-sync runs on **iroh 1.3 / iroh-docs 0.101 / iroh-gossip 0.101 /
iroh-blobs 0.103.1** (ported from the 0.35 stack; earlier still from the
pre-0.35 APIs). The port was faithful — same behavior and guarantees — and
this section records the API seams it moved through. 0.35 and 1.x peers are
**wire-incompatible** (relay protocol, QUIC multipath, noq, gossip ALPN
`/iroh-gossip/0` → `/iroh-gossip/1`): every peer and relay node of a swarm
upgrades together (flag day). Within v1, every minor is wire-compatible.

**Verification status at port time (2026-10-11):** `cargo check --workspace
--tests` and `cargo +1.99.0 clippy --workspace --all-targets -D warnings`
are clean; **no test had been run yet** (one serial sweep was deferred to
the end of the port). The relay-assisted connection against a self-hosted
iroh-relay 1.x (track acceptance criterion) is still unverified. Track:
`conductor/tracks/iroh_1_0_upgrade_20260711/plan.md`.

API map (0.35 → 1.x) for the seams fe-sync uses:

- Identity/addressing: `NodeId` → `EndpointId`, `NodeAddr {node_id,
  relay_url, direct_addresses}` → `EndpointAddr {id, addrs:
  {Relay(url) | Ip(addr)}}`, `endpoint.node_id()` → `endpoint.id()`,
  `endpoint.node_addr().await` → `endpoint.addr()` (sync snapshot;
  `SyncEndpoint::node_addr` waits — bounded by `iroh::NET_REPORT_TIMEOUT` —
  for `online()` first when a relay is configured, never under `Disabled`,
  where `online()` would pend forever).
- Endpoint construction: `Endpoint::builder(presets::Minimal)` (crypto
  provider only — deliberately NOT `presets::N0`, which would add pkarr/DNS
  publishing to n0's `iroh.link` that 0.35 fe-sync never did) +
  `secret_key` + `relay_mode` + `transport_config` (§congestion-control) +
  `address_lookup(MemoryLookup)`.
- Address book: `endpoint.add_node_addr(addr)` was removed upstream;
  `SyncEndpoint` owns one `iroh::address_lookup::MemoryLookup` and exposes
  `add_peer_addr(EndpointAddr)` (infallible). iroh-docs adds its own
  lookup for `start_sync` peers; the lookups compose (§gossip-bootstrap).
- Bootstrap JSON: `FE_SYNC_BOOTSTRAP` / `OpenVerseReplica.bootstrap_peers`
  / `SyncEvent::Started.node_addr` now carry `EndpointAddr` JSON.
  `parse_bootstrap_peer` still accepts the legacy 0.35 `NodeAddr` JSON shape
  (converted to the equivalent `EndpointAddr`) so existing config strings
  keep parsing; a bare id (hex or base32) is still rejected loudly
  (undialable).

### iroh-gossip 0.101 — ported (real API)

- Instance: `iroh_gossip::Gossip`, built once per online stack via
  `Gossip::builder().max_message_size(..).spawn(endpoint)` — synchronous and
  infallible on 0.101. Offline mode → `None`.
- Topic id: `iroh_gossip::TopicId` (32 bytes). Derived from the verse topic
  string with `gossip_topic_id()` (blake3 → `[u8;32]` → `TopicId::from`).
- Subscription: `gossip.subscribe(topic_id, bootstrap).await ->
  GossipTopic` — **async on 0.101** (it hands the Join request to the gossip
  actor), so `subscribe_to_verse_gossip_topic` is async and is awaited in the
  open sequence BEFORE the doc open (§gossip-bootstrap). We hold the split
  halves: the `api::GossipSender` in `gossip_senders` (via `TopicSender`),
  the `api::GossipReceiver` in a pump task; **dropping both halves is how you
  leave a topic** (there is no `unsubscribe(topic_id)`).
- Events: the 0.35 nesting `Event::Gossip(GossipEvent::Received(m))` is
  flattened to `api::Event::{Received(Message), NeighborUp, NeighborDown,
  Lagged}`. `Message {content, scope: DeliveryScope, delivered_from}` and
  `DeliveryScope::is_direct()` (`Neighbors | Swarm(Round(0))`) are
  unchanged, so the F23 gate input is byte-for-byte the same
  (§transport-admission-control). A broadcaster still never receives its own
  broadcast (plumtree `broadcast` emits no local event).
- Broadcast: `GossipSender::broadcast(Bytes).await` (best-effort; messages
  queue until a neighbor is available). Unsigned transform broadcasts are
  intentionally absent.

**Inbound consumption is live for the compute plane (F7/F9) AND the tileset
plane (PAS-P3 S3).** The distributed-query fan-out
(`distributed_query.rs::handle_gossip_incoming`, real and virtual transports
alike) drains the verse's gossip topic for `ComputeEnvelope` request/response
frames (tagged `type`). Tileset advertisement rides the SAME topic, tagged
`kind` instead — `decode_tileset_ad` is tried FIRST in the `gossip_inbound_rx`
select branch; a hit is handled by `handle_inbound_tileset_ad` (records the
provider, emits `SyncEvent::PeerTilesetAdvertisement`) and never reaches
`handle_gossip_incoming`; a miss (any other frame shape — a compute envelope,
garbage, or a pre-S3 build's frame) falls through unchanged. `SyncEvent::
NodeTransformed` is an unsupported legacy unsigned variant with no current
emitter. Inbound *connections*
are routed by the stack's `Router`, which accepts `iroh_gossip::ALPN`
(`/iroh-gossip/1`).

### iroh-docs 0.101 — stack and replicators both real (A1/A2); mock is offline-only

Current as of F2/A2 (2026-10-07), F23's transport-admission hardening
(2026-10-08) and the iroh 1.x port (2026-10-11) — see §inbound-apply,
§sharding, §distributed-query, and §transport-admission-control below for
what the sync thread does with this stack once it's up.

The full P2P stack spawns in the sync thread (`docs_engine.rs::DocsStack`):
the legacy-store migration (§store-migration) →
`iroh_blobs::store::fs::FsStore::load(<dir>/blobs)` +
`iroh_blobs::BlobsProtocol::new(&store, None)` + `Gossip::builder()...
.spawn(endpoint)` + `iroh_docs::protocol::Docs::persistent(<dir>)
.spawn(endpoint, blobs_store, gossip)` (the redb replica store `docs.redb` +
persistent default-author storage) + `iroh::protocol::Router` accepting all
three ALPNs (`/iroh-bytes/4`, `/iroh-gossip/1`, `/iroh-sync/1`). Persistent
stores live under `FE_P2P_DIR` (`p2p_data_dir()`, default `data/p2p`); the
dir is created at spawn. The in-process docs client is
`iroh_docs::api::DocsApi` (`Docs` derefs to it; 0.35's quic-rpc `MemClient`
/ `Doc<FlumeConnector>` are gone — `DocHandle = iroh_docs::api::Doc`). Blob
reads go through `iroh_blobs::api::Store` (`status` first, then
`get_bytes`); `iroh-io` and `quic-rpc` are no longer dependencies.
`FsStore` runs its own multi-thread tokio runtime (its drop uses
`block_in_place` on that runtime's own worker, which upstream intends to be
safe under the sync thread's current-thread runtime and inside
`#[tokio::test]` — **not yet exercised by a test run at port time**; the
orchestrator sweep's stack/harness tests are the proof); the Router's
shutdown calls each handler's `shutdown`, which shuts the blob store and the
docs engine down.

- The stack spawns only when the endpoint bound. On **bind failure**,
  **store-migration failure** or **stack-spawn failure** the sync thread
  degrades to offline mode: the
  holder stays empty (`is_available() == false`), gossip is absent, and
  replicators use the in-memory mock — loudly (`error!` +
  `SyncEvent::RelayHealthChanged`), never a crash.
- `IrohDocsEngineHolder` is a real holder (`Option<Arc<DocsStack>>`):
  `is_available()` reflects the stack; `docs_client()` hands out the
  `DocsApi` clone the replicator layer writes through.
- **One gossip instance per endpoint.** Gossip is owned by the stack; a
  second `Gossip` spawned outside it would never see inbound connections
  (the Router routes to the stack's instance only). Likewise two stacks in
  one process must not share a data dir — the redb file lock makes the
  second spawn fail (→ degrade).
- **Real Doc-backed replicators (F2 / A2):** the `VerseReplicator` /
  `PetalReplicator` traits are async (hand-boxed `ReplicatorFuture`, no
  async-trait dep), and `IrohDocsReplicator` rides the real
  `iroh_docs::api::Doc` whenever the stack is online: `open_document` imports the
  `Capability::Write(NamespaceSecret)` (secret hex) or opens the namespace
  read-only without one, `write_row` → `Doc::set_bytes` (empty payload →
  `Doc::del`, the empty-entry tombstone), `subscribe` → a `LiveEvent` pump
  (`InsertRemote` + `ContentReady` → `RowChange`s carrying the payload
  bytes read from the stack's blob store; `InsertLocal` skipped for loop
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
  (`DocsApi::author_import` + `author_set_default`), so `entry.author()`
  resolves to the endpoint `EndpointId` did:key — the SAME identity the fe-identity keypair
  exposes (`to_iroh_seed` shares the seed), which is what the A3 gate on the
  DB thread resolves roles against. Both the live pump and the snapshot
  attribute `RowChange.author_id` from `entry.author()` (`author_did_key`),
  never from `InsertRemote.from` (the FORWARDING neighbor — a ≥3-member doc
  would otherwise judge the relaying node, not the author). Any other
  per-dir default author is unrelated to the app identity and must not be
  used for attribution. Loop prevention stays correct because
  our own writes are authored by that same endpoint identity.
- **Namespace ids (F20 finding 3).** `fe_database::derive_namespace_id`
  returns the Ed25519 verifying key of the namespace secret — the id the doc
  actually registers under (`NamespaceSecret::from_bytes(secret).id()`), so
  the secretless open (`DocsApi::open(stored_id)`) finds a previously imported
  doc on the same persisted store. Legacy keyed-BLAKE3 ids still in live DBs
  can never match; the secretless path falls back to scanning known docs for
  the one whose `verse/{verse_id}` manifest key exists
  (`find_doc_by_verse_manifest`). iroh-docs 0.101's `Doc` has **no
  close-on-drop** (0.35's rpc `Doc` closed itself when its last clone
  dropped), so the scan explicitly `close()`s every probed doc that is not
  the match — the engine refcounts open handles, and an unclosed probe
  would leak an open replica per known doc on every legacy reopen. Any new
  code that opens a `Doc` must close it the same way.

`status.rs` deliberately ignores the legacy unsigned `SyncEvent::NodeTransformed`
variant. It has no current emitter. It must not bypass signed row admission by
writing directly into the local world; see section write-policy for the current
network write boundary.

Runtime behavior: row bytes traverse the real network. The fe-test-harness
scenario `two_peer_replica_sync` proves A2 end-to-end (two in-process peers
over loopback, READ-BACK from the joining peer's durable store).

## §store-migration — 0.35 data dirs on the 1.x stack (`p2p_store_migration.rs`)

**Why:** iroh-docs 0.35 / iroh-blobs 0.35 (redb 2.4) wrote `docs.redb` and
`blobs/blobs.db` in redb **file format v2**. iroh-docs 0.101 / iroh-blobs
0.103 open them with redb 4, which refuses v2 files ("upgrade required") —
an existing `data/p2p` would otherwise fail `DocsStack::spawn` and degrade
the node to offline/mock forever.

**What runs:** `DocsStack::spawn` calls `migrate_legacy_p2p_store(dir)` on
the blocking pool before any store opens:

1. Marker `<dir>/.fe-store-format` == `iroh1` → no-op.
2. No legacy redb file present → write the marker (fresh dir). Stores the
   1.x stack creates are therefore always marked and never re-examined.
3. Otherwise: **copy** the whole dir to `<dir>.v035.bak-<unix_ts>` (written
   as `<…>.partial` and renamed only once complete, so an interrupted copy
   never masquerades as the rollback; never
   moved or deleted — it is the rollback), open each legacy file with
   `redb_v2` (the workspace's `redb = "2.6.4"` alias — the last redb that
   can upgrade v2→v3) and call `Database::upgrade()`, then write the marker.
   Any error is returned loudly and the marker is not written, so the next
   start retries.

iroh-docs 0.101's default `redb-v2-migration` feature then rewrites the
tuple tables (`records-1`, `latest-by-author-1`, `records-by-key-1`) on
first open — the 0.35 schema is identical to the one that feature targets.
iroh-blobs 0.103 needs only the file-format step (same table type names,
same postcard encodings, same `data/{hash}.data|.obao4` layout).
**Never build iroh-docs with `default-features = false` without re-adding
`fs-store` + `redb-v2-migration`.**

**Failure → offline/mock**, exactly like a stack-spawn failure (`error!` +
`SyncEvent::RelayHealthChanged`). Rollback: stop the node, replace `<dir>`
with the backup copy, run a 0.35 build. Last resort: move the dir away and
start fresh — rows written locally stay in SurrealDB, but doc-only rows
(peers' entries not yet admitted by the A3 gate) only come back by
re-syncing from 1.x peers.

**Proof:** `p2p_store_migration::tests::legacy_v035_fixture_migrates_and_reads_back`
copies the 0.35-written fixture `fe-sync/tests/fixtures/p2p_store_v035/`
(written by a real iroh 0.35 / iroh-docs 0.35 / iroh-blobs 0.35 build — the
standalone, non-member generator crate beside it, `p2p_store_v035_gen/`,
mirrors the 0.35 `DocsStack` + `IrohDocsReplicator` write path:
`SecretKey [7;32]`, `NamespaceSecret [23;32]`, a verse manifest row, a small
row, a 40 KiB row that lands in `blobs/data/`, and a `Doc::del` tombstone),
migrates it, spawns the 1.x `DocsStack` on it, reopens the doc secretlessly
and asserts every `expected.json` row (len, blake3, tombstone flag), plus
that a second migration is a no-op.

**The fixture is generated, never committed** (RUNBOOK §3 R2, 2026-10-13).
`fe-sync/tests/fixtures/p2p_store_v035/` is gitignored (~7 MB of redb + blob
data). The generator is pinned: its own `[workspace]` with a committed
`Cargo.lock`, always run `--locked`. It cannot join the root workspace,
because iroh 0.35 and the 1.x stack cannot coexist in one dependency graph.
Seeds are fixed but entry timestamps are not, so two runs are not
byte-identical. That is fine: `expected.json` records content hashes, and any
fresh run satisfies the test.

- **Regenerate locally** (the output dir must not exist):
  `cd fe-sync/tests/fixtures/p2p_store_v035_gen && rm -rf ../p2p_store_v035 && cargo run --locked -- ../p2p_store_v035`.
  Its `target/` stays inside the generator dir and is gitignored.
- **`FE_REQUIRE_STORE_FIXTURE` contract.** Unset, or any value but `1`: the
  test skips with an `eprintln!` when `expected.json` is missing or the test
  endpoint cannot bind, so a dev box without the fixture stays green. Set to
  `1`: both skips become panics. A missing fixture or a skipped read-back is
  then red, never skip-as-green. CI sets it. No fixture-path override exists;
  the generator writes the default path directly.
- **CI** (`.github/workflows/build-artifacts.yml`, `linux` job, after the
  scoped test sweep — D-66 keeps fe-sync out of that sweep). The job runs the
  generator in the dev profile. It then runs the 3 `p2p_store_migration` lib
  tests with `FE_REQUIRE_STORE_FIXTURE=1`. Two choices in that step matter:
  - It passes the sweep's `-p` set plus `-p fe-sync`. Cargo unifies features
    over the selected packages, so `-p fe-sync` alone would resolve shared
    crates (tokio, bevy, serde, …) differently and rebuild most of the
    release graph. With the sweep's set, every dependency is reused. `--lib`
    plus the name filter means only the migration tests run.
  - The generator's `target/` is cached with plain `actions/cache`, keyed on
    rustc + the generator lock. A second `Swatinem/rust-cache` would break
    the main cache: its post step prunes `~/.cargo/registry/cache` down to
    its own workspace's crates, and the main cache would then be saved
    without its crates.
  - The step tees the test output and greps for
    `legacy_v035_fixture_migrates_and_reads_back ... ok`. A name filter that
    matches zero tests exits 0, so a renamed or cfg'd-out test would
    otherwise go green; the grep makes "the fixture test ran and passed" the
    pass condition.

## §namespace-id-capability (SYNC-NSID-CAP, 2026-10-11)

**Question** (RUNBOOK §9, flagged by the M4 re-review): is `verse.namespace_id`, readable
through fe-api `/query`, a read capability for the verse's iroh replica? **Verdict: YES, by
design, and it is LIVE today — not latent.**

- `fe_database::derive_namespace_id` (`fe-database/src/lib.rs:1444`) returns the namespace
  secret's Ed25519 verifying key — the iroh-docs `NamespaceId` the doc registers under
  (§iroh-1.x "Namespace ids").
- In iroh-docs (0.35 and 0.101 alike) a read capability *is* a `NamespaceId`: `sync.rs` defines
  `Capability::Read(NamespaceId)` beside `Capability::Write(NamespaceSecret)`. fe-sync itself
  opens a replica read-only from nothing but the id (`replicator.rs:554-555`,
  `parse_namespace_id` → `DocsApi::open(ns)`).
- Inbound docs sync is not peer-authorized: iroh-docs `engine/state.rs::accept_request`
  rejects only `NotFound` (namespace not syncing; unchanged in 0.101) — any `EndpointId` that names a namespace this
  node is syncing is served. fe-sync runs `start_sync` on every doc-backed open, and
  `sync_thread.rs:535/573` spawns the real `DocsStack` (Router accepting `iroh_docs::ALPN`) and
  `IrohDocsEngineHolder::online` whenever the endpoint binds. (The claim that the docs engine
  never serves was stale: that described the pre-A1 holder.)
- So id + a serving node's address = the full replica (every row the verse replicates). The
  write half stays safe: writing needs the `NamespaceSecret`, and inbound rows still pass the
  DB-thread admission gate (§inbound-apply, §write-policy).

**Consequence.** Treat `namespace_id` as a secret wherever it leaves the process. fe-api
`/query` (and share/export, which reach `verse` through the same `prepare_scoped_sql`) now
refuses the column and hides it from every `verse` row (fe-api/AGENTS.md §query-guard). No
other fe-api route returns it. The id still travels where it must — the local UI's own verse
tree and replica open — and anyone a verse is shared with necessarily holds it.

**Not done here (needs a decision, no network change in this pass):** peer-level admission on
the docs ALPN (an iroh-docs accept callback that consults verse membership), and rotating the
namespace of a verse whose id may already have leaked. Both are network-plane changes owned by
the next fe-sync security pass.

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
`DbCommand::ApplyReplicatedRow` to the DB thread — never a blocking send.
A full channel keeps the row (QA-0138; counted in `INBOUND_APPLY_DROPS` /
`inbound_apply_drop_count()`): content rows go to the fabric's `KeptRows`
(one live entry per table/record/author — a newer replica entry supersedes
the kept one), role rows to their own QA-0132 queue, and later rows of the
verse queue behind kept ones so nothing overtakes them. `resend_unsent_rows`
drains them at the top of every loop turn (kept work wakes the loop) and
prefetches the blob of each resent scene-asset row. Before QA-0138 a 2 Hz
stream behind a busy DB thread lost rows here until the next replica open. The DB thread is the single SurrealDB writer and the A4 enforcement
point (see fe-database `handlers/replicated_row.rs`), where the A3 role gate
also lives (landed F3, 2026-10-07: deny-by-default, Editor+, roles resolved
from the local tables at the verse scope — never wire-supplied).

Bootstrap peers (A9 prep): `FE_SYNC_BOOTSTRAP` holds **semicolon-separated**
`EndpointAddr` JSON entries (semicolons because the JSON contains commas),
parsed with iroh's own types — the legacy 0.35 `NodeAddr` JSON shape is still
accepted (§iroh-1.x); a bare `EndpointId` carries no address and is skipped
loudly. `OpenVerseReplica.bootstrap_peers` carries the same entry form
per-verse and merges with the env set (deduped); `SyncEvent::Started.node_addr`
emits our own dialable `EndpointAddr` JSON in exactly that form (F4 also logs it
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

## §event-outbox — non-blocking `SyncEvent` emission (`event_outbox.rs`, PAS-P5 S5 / W1 residual)

The sync thread runs on a current_thread runtime. The command loop and every
task it spawns share one OS thread, so a blocking crossbeam `send` into the
`bounded(256)` event channel (`fractalengine/src/main.rs`) from any of them
stalled the WHOLE thread once more than 256 events piled up between Bevy
frames. It could not serve commands, inbound rows or even `Shutdown`.
**Every emit now goes through `EventOutbox`** (`EventSink::emit`):

- `try_send` first. On `Full`, the event joins a loop-local FIFO backlog,
  and once a backlog exists every later event queues behind it, so order
  is kept.
- **State-bearing events are lossless** (`RowApplied`, `BlobReady`,
  `NodeTransformed`, tileset meta/chunk/complete/resumed, `ReplicaOpenFailed`,
  `ShardLedger`, `Started`/`Stopped`, and everything not listed below). They
  wait in the backlog for as long as it takes, with a warning at 256 queued
  and at every 4x past that.
- **Advisory events coalesce.** A queued `RelayHealthChanged` is replaced in
  place by a newer one, and an identical queued `PeerTilesetAdvertisement`
  (same peer, same payload) absorbs the repeat. At most
  `MAX_ADVISORY_BACKLOG` (1024) advisory events are held; past that they are
  dropped. Every coalesce or drop is counted (`dropped_advisory`) and warned,
  at the first and then every 1000th.
- **The loop drains it (the self-drain rule).** The loop calls `flush()` at
  the top of every iteration. A `backlog_ready()` select branch wakes it when
  an emit queued something (`Notify`), then retries every 5 ms while the
  channel stays full. Nothing ever awaits or blocks on a send the loop must
  service itself. On exit, `flush_before_exit` gives the main thread
  `SHUTDOWN_FLUSH_TIMEOUT` (5 s) to take the rest, then drops it with a warning.
- Production code cannot emit through a raw channel: the `EventSink` impl
  for `SyncEventSender` is `#[cfg(test)]` only. `distributed_query`
  keeps its own `try_send` drop-and-count (`EventOutbox::sender`).
- Tests: `a_burst_from_spawned_tasks_never_blocks_the_loop_and_loses_nothing`
  and `advisory_events_coalesce_while_state_events_queue` (unit), plus
  `a_command_burst_past_the_event_capacity_never_stalls_the_loop` (sync_thread.rs).
  The last one runs 600 commands through a `bounded(1)` command channel
  while the main thread drains nothing. Every command is accepted, and all
  600 replies and `Stopped` then arrive in order.

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
  on replica open is the deferred shape if that hole ever matters. **Role
  statements already take that shape** (role_propagation AC-42): after every
  successful open, `handle_open_verse_replica` sends the DB
  `DbCommand::ReemitLocalRoleStatements { verse_id }` (`try_send`, never
  awaited), and the DB re-publishes this node's own live statements through
  the normal outbound bridge (fe-database AGENTS.md §role-propagation).
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

## §relay-health (track `p2p_asset_streaming_20260718` FR-1 / decision D-77; SYNC-RELAYWATCH 2026-10-11)

**Why:** relay failure used to be quiet — default relay binding with no
relay config wired, offline detection only checked bind failure, so a dead
relay meant gossip/replica traffic queued silently forever. The original
trigger was iroh 0.35's n0 relays EOLing 2026-12-31; the iroh 1.x port
(§iroh-1.x) moved fe-sync onto the v1 relay fleet
(`{use1,usw1,euc1,aps1}-1.relay.n0.iroh.link`, supported for the life of
iroh v1). Those public relays are **shared and rate-limited for relayed
traffic** (iroh ≥1.1 logs a warning when a client is throttled), so
production deployments should still configure their own relay (`Custom`;
self-host the `iroh-relay` 1.x binary, feature `server`). This section
documents the config + health model that replaces the silence.

### `RelayConfig` (`relay_config.rs`)

Three variants, matching the D-77 hardening note verbatim: `Default` (iroh's
n0-hosted public v1 relays — shared and rate-limited), `Disabled` (direct/LAN-only), `Custom(Vec<String>)`
(operator relay URLs). `RelayConfig::parse` accepts the keywords
`"default"`/`"disabled"`/`"none"` or a comma-separated URL list, validating every
URL eagerly with iroh's own `iroh::RelayUrl` parser (`FromStr`) so a
misconfiguration fails at parse time — and again at `to_relay_mode()` time in
case a `Custom` value was hand-built bypassing `parse` — rather than on first
dial. `to_relay_mode()` converts to the `iroh::RelayMode` the endpoint builder
expects (`iroh::RelayMode::{Default,Disabled,Custom(RelayMap)}`, the custom
map built with `RelayMode::custom(urls)`; iroh 1.x refuses to bind an empty
custom map, so an empty URL list is `RelayConfigError::Empty`). iroh's
`Staging` mode is deliberately not exposed. Name clash to remember: iroh 1.x
exports its own `iroh::RelayConfig` (the per-relay entry) — keep iroh paths
fully qualified next to `crate::relay_config::RelayConfig`.

`RelayConfig::from_env()` is what `spawn_sync_thread` calls; it is
`resolve(env, settings)` over two sources, with this precedence:

1. `FE_SYNC_RELAY` (env) — the ops override, always wins when set;
2. the application-settings spec (`AppSettings.relay`, persisted in fe-ui's
   `settings.ron`), registered by the GUI binary with
   `set_settings_relay_spec(raw)` **before** it spawns the sync thread
   (a process-wide `OnceLock`; first call wins);
3. otherwise `Default`.

A source that fails to parse warns and yields `Default` — it does **not** fall
through to the next source, so a typo never silently picks a different relay
set. The settings value is read once at endpoint build, so a change in the
Settings section applies after restart. Hosts that never register a spec
(`fractalengine-relay`, the test harness, examples) behave exactly as before
(env or `Default`). `spawn_sync_thread`'s signature is unchanged on purpose:
the OnceLock seam keeps the source transport-agnostic for the iroh 1.x port.

### `RelayHealth` (`relay_config.rs`, surfaced via `SyncStatus::health`)

Five states: `Unknown` (default, pre-signal), `Healthy`, `Degraded`,
`Unreachable`, `Disabled`. `on_bind_success` reads the `RelayConfig`:
`Disabled` → `Disabled` (no relay needed — not a failure), anything else →
**`Unknown`** — on iroh 1.x a bind contacts no relay, so a successful bind
proves nothing about reachability (on 0.35 this was `Healthy`). A hard bind
failure is `on_bind_failure` → `Unreachable`. Runtime signals move the
state with `on_error`/`on_success`: `Healthy` degrades one step to
`Degraded` before reaching `Unreachable` (two consecutive failures, not
one, to avoid flapping on a single transient error); `Disabled` is a fixed
point — a disabled relay cannot "fail" or "recover". `is_problem()`
(`Degraded`/`Unreachable`) is what a consumer should alarm on; `Disabled`
and `Unknown` are deliberately excluded.

Every transition emits `SyncEvent::RelayHealthChanged { health }`, which
`status.rs::drain_sync_events` applies to `SyncStatus.health`, loud-logging
(`warn!`) whenever the new health `is_problem()`. Signals wired:

1. the endpoint bind result (`on_bind_success`/`on_bind_failure`);
2. a P2P-stack spawn / store-migration failure right after a successful
   bind → `Degraded` (the 0.35 one-step degradation; from the post-bind
   `Unknown` this is set explicitly, since `Unknown.on_error()` would claim
   `Unreachable` for what is a local store problem). The watcher below is
   NOT started when the stack failed, so this signal is never overwritten
   by a later `Healthy` while replication is down;
3. **the continuous home-relay watcher (SYNC-RELAYWATCH).** When a relay is
   configured (not `Disabled`, not virtual, not offline, stack spawned), the sync thread
   takes `SyncEndpoint::home_relay_status().stream()` (iroh 1.x
   `Endpoint::home_relay_status()` — a `Watcher<Vec<RelayStatus>>`, one
   entry per home relay) as a `tokio::select!` branch beside commands,
   inbound rows and gossip. `stream()` (not `stream_updates_only`) yields
   the current value first, so a relay that connected while `node_addr()`
   waited on `online()` promotes `Unknown → Healthy` immediately. Each
   snapshot is summarized (`RelayStatusSummary::from_status` — upstream
   `RelayStatus::new` is crate-private, so the pure mapping
   `RelayHealth::on_relay_status` is table-tested against the summary):
   any connected relay → `on_success`; else an auth denial
   (`auth_denied_reason()`, iroh ≥1.2) → `Unreachable` directly + `error!`
   (logged on the descent only; repeat snapshots while already
   `Unreachable` log at debug)
   (it never self-heals); else any `last_error()` → `on_error` (two
   strikes); an empty snapshot (no home relay chosen yet) or no error yet →
   unchanged. `RelayHealthChanged` is emitted only when the value changes.
   The watcher only disconnects when the last `Endpoint` clone drops; a
   `None` from the stream retires the branch (no busy loop). `last_error()`
   is logged, never matched on (upstream: an opaque `AnyError`). Tests:
   `relay_config::tests::on_relay_status_transition_table` (the mapping) and
   `sync_thread::tests::relay_watch_drives_health_into_sync_status` (the
   emit-on-change seam `fold_relay_health` → `drain_sync_events` →
   `SyncStatus.health`, Unknown → Healthy → Degraded → Healthy).

### Default-relay warning

`endpoint.rs::warn_default_relay_once()` logs one `WARN` per process (a
`std::sync::Once` guard) the first time a `SyncEndpoint` binds against
`RelayConfig::Default`: the public n0 relays are rate-limited, configure
`FE_SYNC_RELAY` / the Settings relay for production. (On 0.35 this was the
2026-12-31 EOL warning.) Deliberately not unit-tested for the "fires exactly
once" property — `Once` is process-global static state, so asserting on it
across `#[test]` functions in the same test binary would be order-dependent;
the pure decision of *whether* to warn (`RelayConfig::is_default_infra`) is
what's tested instead.

### Self-hosting iroh-relay 1.x (IROH-1X relay-assisted AC, W6-8)

Production `Custom` relays should run the `iroh-relay` binary (the
`iroh-relay` crate's `server` feature), self-hosted rather than the default
n0 fleet. `--dev` (the flag `fe-sync/tests/relay_assisted_test.rs` runs the
container with) is **test-only**: it binds plain HTTP, defaults
`access = everyone`, and has QUIC address discovery off. It is never the
production invocation — production needs TLS/ACME (the binary's own `--tls`
flags, or a reverse proxy terminating TLS in front of it) and an access
control mode narrower than "everyone", normally `access.shared_token` in its
TOML config:

```toml
http_bind_addr = "0.0.0.0:3340"
access.shared_token = ["<a real token>"]
```

The tests pin `n0computer/iroh-relay:v1.3.0` by digest (A13) and manage the
container's whole lifecycle themselves (start, `/healthz` poll, an
exact-name `rm -f` in a `Drop` guard that also runs on panic) through
`docker` or `podman` (`FE_CONTAINER_RUNTIME` override, else the first
runtime whose `<rt> info` succeeds); with neither available they skip,
unless `FE_REQUIRE_RELAY_CONTAINER=1` turns the skip into a failure — the
CI step's contract, same shape as `skip_unless_required` in
`p2p_store_migration.rs` §store-migration.

**Relay access token (A14).** `relay_config::RelayAccessToken` is a
standalone newtype — never a field of `RelayConfig` itself — read from
`FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN` via fe-identity's `EnvBackend`
(`RelayAccessToken::from_env`), and attached as `Authorization: Bearer` only
to `RelayConfig::Custom` relays reached over `https://` or loopback `http://`
(`RelayConfig::to_relay_mode_with_token`; a non-loopback plaintext URL is
refused with `RelayConfigError::InsecureTokenTransport` before the token is
ever sent — RL-2). `Default` and `Disabled` never carry it: there is no
operator relay to attach it to, and the n0 fleet must never see an
operator's token. `SyncEndpoint::new` delegates to
`SyncEndpoint::with_relay_token(secret, relay_config,
RelayAccessToken::from_env())`, so production (`sync_thread.rs:527`'s only
call site) picks up the token with **no edit to that file**. The token's
`Debug` redacts to `RelayAccessToken(***)`, and it has no `Display` or
`Serialize` impl, so the existing `?relay_config`-style logs
(`endpoint.rs`, `sync_thread.rs:459`) stay safe without an edit either —
the token simply never lives on the type those logs format (RL-1).

The token-gated test leg runs `iroh-relay --config-path` against a 3-line
TOML whose `access.shared_token` entry is a decoy; the real, per-run random
token is passed as `IROH_RELAY_ACCESS_TOKEN` through the child container
process's own **environment** (`docker`/`podman run -e
IROH_RELAY_ACCESS_TOKEN`, no `=value` on argv) — `iroh-relay` itself treats
that env var as an override that replaces its config's token list
entirely. The token is never written to a file, a log, or this process's
own environment.

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
  apart. The sync thread passes its own DID + iroh `EndpointId` (one ed25519
  key), so the F23 sender-identity gate applies to sim frames verbatim. A
  factory without a gossip plane (the trait default `None`) leaves the verse
  honestly topic-less (`SubmitComputeTask` runs local-only). fe-sim's
  `SimNet` implements it with NO self-echo (iroh-gossip — 0.35 and 0.101
  alike — never delivers a sender's own broadcast back to it — the hub matched that on 2026-10-09,
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
  latest-per-entry semantics. **Ledger admission (F23 + RP-5):** `note_entry`
  enforces `__peers` self-declaration (the row key must name its own signed
  author — a forged `__peers/{other_did}` capacity row is dropped; see
  §transport-admission-control item 4). The `__shards` Editor+ gate — once
  evaluated and NOT implemented because no role data reached the sync plane
  — is now CLOSED by RP-5's role view (§role-view): a `__shards` row needs
  Editor+ at the shard's own petal scope (`VerseFabric::role_at`, folding in
  `default_access`); a row that cannot be judged yet (role not converged, or
  the petal unknown locally) is quarantined, not dropped — see §role-view
  "Quarantine" and §transport-admission-control item 5 (now CLOSED).
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

## §role-view (RP-5, role_propagation_20261013)

The sync plane's OWN materialized role view, independent of — and running
alongside — the DB plane's `role_grant`/`role_statement_pending` tables
(fe-database `src/AGENTS.md` §role-propagation). `VerseFabric` owns it
(`sharding.rs`); it exists because the gates this crate already needs
(`__shards` admission, the compute requester, a manifest's settings) had no
role data to resolve against before this track (§transport-admission-
control item 5's old residual). Both planes call the exact same pure
function, `fe_policy::role_grants`'s `materialize`/`validate_statement`/
`EffectiveRoles::{role_at, authority, admissible}` (plan §2.2) — so a
`fabric_and_db_materialize_identical_effective_roles_from_the_same_statements`-
style parity holds by construction, not by coincidence.

- **Owner, pinned once.** `VerseFabric::learn_owner` takes a verse
  manifest's `created_by` the first time a TRUSTED one is seen and never
  relearns it. Trusted (OF-6d, W5s 2026-10-25) = an inbound manifest whose
  signed author IS its `created_by` (self-signed), or our own outbound,
  locally-admitted write (`note_local_verse_row` — the creator, or an
  invite-pinned owner, fe-database T10). A manifest claiming someone else as
  creator pins nothing and adopts no settings; its claimed creator is queued
  on `reread_queue`, so the §2.5 re-read replays THAT DID's own `verse/`
  entry (a later Manager edit shadowing the owner's entry in the snapshot
  cannot stall the pin). **Local correction (R1, 2026-10-25):** our own
  locally-admitted manifest is the local authority on the owner, so
  `repin_owner_from_local` REPLACES a different pin an inbound self-signed
  manifest set first — `seed_reconciliation` replays the replica BEFORE the
  pending local writes flush, and any member holding the namespace secret
  can author a self-signed `verse/` entry naming itself, so
  first-self-signed-wins alone handed a fresh joiner's sync-plane owner to
  whoever wrote first. The correcting manifest's settings are adopted
  outright (a false owner's `default_access: viewer` must not outlive it),
  and `rematerialize` drops every statement the false owner made. The
  same authority holds across a restart: a `verse`/`petal` entry signed by
  OUR OWN key replayed from the replica is `RowOrigin::OwnReplay`
  (`note_own_replayed_*`, routed by `handle_inbound_row_change`) — it
  corrects a false pin / maps an unmapped petal, but being possibly STALE
  its settings and petal MOVES pass the ordinary gates; and because the
  snapshot is latest-per-key (a later forged `verse/` entry hides ours),
  `reassert_own_rows` replays our own `verse/`+`petal/` entries by author
  right after `seed_reconciliation`. Tests:
  `this_nodes_own_manifest_corrects_an_owner_pinned_by_a_forged_self_signed_manifest`,
  `a_replayed_own_manifest_corrects_a_forged_owner_pin_without_overriding_newer_settings`.
  Residual (D-23 class): a node that never writes a manifest of its own
  (a relay that opened without an invite) still pins the first SELF-signed
  manifest it sees — the same immutability rule the DB plane's `created_by`
  field keeps (T9). Everything downstream
  (`role_at`, `admissible`) is `EffectiveRoles::default()` (nobody has any
  role) until the owner is known, which is the correct pre-manifest answer
  with no special-casing needed.
- **Admitted statements, not a pending table.** `role_statements: HashMap<
  (scope, peer_did, grantor_did), RoleStatement>` holds only statements
  `EffectiveRoles::admissible` currently accepts — one per key, the latest
  by `fe_policy::latest_per_grantor`'s tie-break. A statement that fails
  `admissible` is **discarded on arrival, never stored** — unlike the DB
  plane's `role_statement_pending` table, the fabric has nothing of its own
  to persist: the replica (iroh-docs, or the mock in tests) already holds
  the row durably, so re-fetching it later is enough (see "Re-read on
  empowerment" below). This is why the fabric needs no flood budget of its
  own either — the replica bounds what it can ever hold.
- **`rematerialize` runs the demote cascade to a fixpoint.** Every
  admission (an insert) or empowerment (the owner learned, or a statement
  admitted) calls `rematerialize`: recompute `fe_policy::materialize` over
  the current statement set, diff against the previous view (queuing every
  RISEN did into `reread_queue`), then drop any now-inadmissible statement
  and recompute again — looping until a pass removes nothing. This is what
  makes concurrent revocation converge (plan §0's refinement #1): revoking
  a Manager cascades to invalidate every statement THEY made that depended
  on their own authority, in one call, regardless of insertion order.
- **`role_at(peer, scope)` folds in `default_access`.** The one function
  every gate in this crate calls (mirroring the DB plane's A3 gate and
  local grant authority, same `fe_policy` function underneath): the
  explicit materialized role, or `default_access` (parsed from the
  manifest's `default_access` field, restricted to `{viewer, none}` —
  anything else is treated as `none`, never silently mapped through
  `RoleLevel::from`) when nothing explicit applies.
- **Gated `note_verse_row`.** A verse manifest's `ts_*`/`default_access`
  settings are adopted only once an owner is pinned (OF-6d — an unpinned
  fabric adopts nothing inbound; only our own locally-admitted first
  manifest is adopted outright), and then only when its author is the
  owner or Manager+ AT THE VERSE SCOPE. A petal/fractal Manager's or an Editor's manifest edit is
  ignored: verse settings sit above their subtree (R7).
- **Petal -> fractal map.** `note_petal_row` learns it from `petal` rows.
  A `__shards`/`__holders` doc key carries only the petal id, but
  `fe_policy::build_scope` needs the fractal segment too (it panics on a
  petal id with no fractal) — `VerseFabric::petal_scope` is the lookup that
  builds the full scope string on demand, returning `None` when the petal
  hasn't converged locally yet. **Gated (OF-9, R1 2026-10-25)** — the map is
  the spine of every `role_at` scope chain, so an ungated row could
  re-parent a victim petal under an attacker's fractal (their grant there
  suddenly enclosing it) or strip legitimate Managers of it. An inbound row
  is admitted exactly as the DB plane's A3 `confine_petal` admits it:
  scope-safe ids (no `#`, mirrored `is_scope_safe_id`), the signed author
  Editor+ (the fe-policy write floor) at the petal's scope under the row's
  fractal, AND — when the row MOVES an already-mapped petal — Editor+ at its
  current scope too. Editor+, not Manager+: the DB plane lets an Editor
  create a petal, and a stricter sync gate would quarantine every
  Editor-created petal's `__shards`/`__holders` rows forever. A failing row
  is quarantined (`QuarantineKind::Petal`), re-judged on every role change,
  and `petal/` is in the re-read-on-empowerment prefix set. Our own outbound
  petal write is locally admitted (`note_local_petal_row`). A move re-verdicts
  admitted holders. Test:
  `a_petal_row_without_authority_over_its_scopes_never_bends_the_scope_chain`.
  **Provenance + contest (R6, OF-10a/b).** Each mapping remembers its
  `PetalMapping` provenance — the signed author whose Editor+ admitted it
  (`None` for our own write / own replay of an unmapped petal: locally
  authoritative, never re-verdicted or contested) — plus a short history
  (cap 4) of what a move replaced.
  - *No first-writer-wins (OF-10a).* Before R6 the first authorized
    mapping squatted: a Manager of fractal `fa` who knew a victim petal's
    ULID mapped it under `fa` first, and the genuine row (author Editor+ at
    `fv` only) then failed the move gate forever. Now a conflicting row
    whose author is Editor+ at its own target but NOT at the incumbent's
    scope is quarantined only if the incumbent dominates (local, or its
    author is ALSO Editor+ at the challenger's scope); otherwise the petal
    becomes **contested** — removed from the map, `petal_scope` = `None`,
    so every gate on it fails closed and neither side's grant encloses it.
    It settles (`settle_contested_petal`) when a claim's author holds
    Editor+ at EVERY claimed scope (latest wins — the owner or a verse
    Editor re-asserting the row), when lapsed claimants (revoked, re-judged
    on every `rematerialize`) leave one fractal standing, or on a local
    write. This also catches the snapshot-shadowing variant: a later
    attacker row hides the genuine one from a fresh joiner's latest-per-key
    snapshot, but the §2.5 re-read replays the genuine author's own
    `petal/` entry, which then contests. Residual (fail-closed
    availability only): an insider with Editor+ in any fractal who knows a
    petal's ULID can contest it on peers whose incumbent is inbound, until
    an author with authority over both scopes re-asserts it.
  - *Re-verdict on role change (OF-10b).* `rematerialize` (every owner
    pin/re-pin, admission, revocation, demote cascade) runs
    `reverdict_petal_mappings` before `reverdict_holders`: an inbound
    mapping whose author lost Editor+ at its scope — or, for a move, at
    the scope it replaced — is quarantined and the previous layer restored
    (itself re-verdicted), else the petal is unmapped. So a false owner's
    mappings die with the re-pin, and the map converges with what a fresh
    joiner would judge under the current view. A newer same-fractal row
    from a current Editor+ refreshes provenance (pushed as a layer), so a
    petal outlives its creator's demotion while anyone authorized still
    asserts it. Tests:
    `a_first_arriving_squat_cannot_hold_a_petal_against_its_real_home`,
    `a_petal_mapping_admitted_under_a_stale_role_view_is_withdrawn_on_re_pin_or_demotion`.
  - *OF-11 hardening (R6b).* `OwnReplay` is now gated whenever the petal is
    mapped OR contested — not just mapped — so a stale replayed own row can
    no longer auto-settle a contest through the unconditional
    locally-authoritative path; a contested own replay instead enters the
    normal claim path under the local DID (test:
    `a_stale_own_replay_cannot_settle_a_contested_petal`). The contest's
    claim cap refuses the incoming claim instead of evicting an existing one,
    and the refusal is remembered in `refused_contest_claims` so a flood can
    never settle past proof it hasn't earned (test:
    `a_claim_flood_cannot_evict_a_contested_fractal_from_the_settlement_proof`).
    R6b's mapping-history cap also refused moves outright; OF-13 below
    replaced that.
  - *OF-13 hardening (polish lane W1-S5, 2026-10-31).* Three fail-closed
    fidelity defects in the R6b machinery, fixed with the review's preferred
    shapes:
    - *Position-independent lapse check (OF-13a/c).* Each `PetalMapping`
      stores `from_fractal`, the scope a MOVE left (`None` for a first
      mapping, a different-author refresh, a settled contest winner, or a
      local write; a same-author refresh inherits it).
      `top_mapping_lapsed` requires the author's Editor+ at `fractal_id` and
      at `from_fractal`. Before this it read the fractal of whatever layer
      sat below on the stack. Cap eviction could change that layer, so a
      fully authorized move could be judged against an older scope and
      rolled back on the next role change. Because the check no longer
      depends on the stack, `push_mapping_layer` no longer protects layers
      and has no refusal path. It always applies the move, then evicts the
      oldest INBOUND layer below the incumbent. It never evicts the
      locally-authoritative base (only index 0 can have `author: None`),
      the new top, or the incumbent the top replaced. Before this, the
      evict scan could remove that incumbent, so a lapse rollback restored
      an older mapping. Rollback still pops one layer at a time and
      re-verdicts each exposed layer against its own `from_fractal`. The
      depth is bounded: if the layer a move replaced was evicted under the
      cap, rollback falls through to the next surviving layer (ultimately
      the local base) or unmaps the petal. Every layer it lands on still
      passes its own authority check. Tests:
      `lapse_rollback_restores_replaced_mapping_not_older_layer`,
      `authorized_move_applies_when_all_history_layers_protected`.
    - *Refused claims are pruned when their author lapses (OF-13b).*
      `refused_contest_claims` maps each contested petal to `author ->
      fractal`. It keeps one entry per author: iroh-docs keeps one entry per
      `(key, author)`, so an author's earlier refused row is already
      superseded. That bounds the set by the number of distinct authorized
      authors. Before this it was a plain set of fractal ids, which grew with
      every distinct id. `settle_contested_petal` drops entries whose author
      no longer holds Editor+ at the claimed fractal, the same lapse
      judgment its partition applies to admitted claims, so a since-revoked
      refused author stops blocking settlement. The map is cleared wherever
      `contested_petals` is removed, including the locally-authoritative
      branch of `admit_petal_entry`, which before this left it stale. Only
      the author can replace their own entry, so the OF-11b guarantee (a
      victim's refused fractal stays owed) holds. Test:
      `revoked_author_refused_fractal_stops_blocking_settlement`.
    - *Re-verdict is author-indexed (R6 efficiency note).* `rematerialize`
      collects every DID that fell in any cascade pass (`ChangedDids::fallen`
      — a superset of the net fall). `reverdict_petal_mappings` then
      re-checks only petals whose current top's author is in that set. This
      is sound: `role_at` reads only `effective`, plus `default_access`
      (capped at Viewer, so it can never satisfy Editor). A top that was
      valid can therefore only lapse when its author's role falls. Every
      top is valid when it becomes top (admitted, settled, local, or
      exposed by a pop and checked by the same loop). The no-owner branch
      passes `None` and re-checks every petal. Contested petals are always
      re-judged, because a RISE can settle them.
    - *OF-12b not applied (accepted residual stands).* The carried idea
      was to require verse-level authority to un-map a petal that has
      admitted holders. It re-opens the OF-10a squat: a squatter who maps a
      petal under its own fractal can admit its own `__holders` rows there
      (it is Editor+ at that scope), and the genuine row then cannot contest
      it. Restricting the check to verse-scoped holders only moves the
      problem, because the owner's holders freeze a squat until the owner
      re-asserts the row. This needs a design decision, not a leaf fix. The
      availability residual above (fail-closed, no bypass) is unchanged.
- **Quarantine (the `__shards` gate's retry seam).** A gated row that
  cannot be judged yet — the author's role hasn't converged, or the row's
  petal is unknown — is held in a capped, drop-oldest FIFO (1024/verse,
  `VerseFabric::quarantine_len`/`quarantine_evicted` for diagnostics), not
  denied. It is re-evaluated on every `rematerialize` (any role-view
  change) and on every new `petal` row. The cap is an accelerator, not the
  safety net: an evicted row is still durably in the replica, and the
  re-read on empowerment below recovers it regardless of eviction.
- **Re-read on empowerment — the durable, flood-proof retry.** When
  `rematerialize`'s diff finds a DID whose `role_at` rose at ANY scope, that
  DID is pushed onto `reread_queue` (deduplicated). The sync thread's
  command loop drains it at the top of every iteration
  (`drain_reread_queues`, `sync_thread.rs`) — never inline inside the
  handler that raised the DID's role, so a slow drain never delays that
  handler's own return. For each queued DID it calls
  `VerseReplicator::entries_by_author(did, ["role/", "__shards/",
  "__holders/", "verse/", "petal/"])` and feeds every entry straight through
  `handle_inbound_row_change`, the SAME path a live row takes. This is a
  provided trait method (default: filters `snapshot()` — correct but
  O(whole replica); `IrohDocsReplicator` overrides it with
  `Query::author(id).key_prefix(table/)` per table, `AnyReplicator`
  forwards to whichever arm is active, and `fe-sim`'s `VirtualReplica`
  inherits the default unmodified — none of them needed editing beyond
  `AnyReplicator`). Because the read goes straight to the local docs store
  and is never routed through the aggregated inbound channel the command
  loop itself drains, it cannot deadlock the §inbound-apply self-drain
  rule.
- **Snapshot reads `role/` from ALL authors (step 3).** `single_latest_per_key`
  picks the single PHYSICALLY newest entry at a key across every author who
  wrote it — so an attacker who writes a forged, later entry to the SAME
  key as the owner's genuine statement (always structurally denied by
  `validate_statement`'s grantor-mismatch check once read) would shadow the
  genuine entry from that query entirely, and a fresh peer whose only
  source is this snapshot would never see it: a convergence DoS. `snapshot()`
  therefore drops whatever `single_latest_per_key` returned for the `role`
  table and replaces it with a SEPARATE `Query::key_prefix("role/")` pass
  (no `single_latest_per_key` aggregation — one entry per `(key, author)`),
  so the owner's genuine entry and the attacker's forgery are both visible,
  independently. `MockVerseReplicator` mirrors this with a second,
  per-`(record_id, author)` store for the `role` table only (`role_entries`,
  seeded by `write_row` for its own author and by the test-only
  `seed_role_entry` for a simulated foreign author) — every other table
  stays the single-author-per-key shape it always was.
- **Not this crate's job.** `fe-sync` never resolves WHICH statements are
  structurally valid beyond calling `fe_policy::validate_statement` — the
  rules themselves (what `owner`/unknown-role strings mean, the subtree
  authority walk) live in `fe-policy` (`fe-policy/src/AGENTS.md`
  §role-grants) and are shared verbatim with the DB plane.

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
- **Responder filtering is RE-ENABLED (SYNC-RESPONDER, RP-7
  2026-10-24) — provisional, revertible on its own.** The §2.6 filter: an
  aggregate request answers only the shards where
  `local ∈ hosts(shard) ∪ holders(shard)`, or the ledger holds NO entry for
  that shard at all here — an absent entry can never prove non-hosting
  (replication lag is unknowable), so it is answered, not silently skipped.
  **Hosted-only filtering is documented as permanently unsafe** for exactly
  this reason. A raw/latest request answers iff the responder hosts or
  holds ANY shard of the requested petal, or the petal is unknown here
  (unchanged from the pre-filter `PETAL_UNKNOWN` gate). A request the
  filter leaves nothing to answer drops with `drop_reason::NOT_HOSTED`
  (`handle_gossip_incoming`, named and unit-testable without a gossip
  stack).

  **Why re-enabling is safe now.** This crate reverted the equivalent
  `hosted_subset` filter on 2026-10-11 (ledgered then as "a pure efficiency
  win, not a correctness fix" — that was false) because roles were
  store-local and never replicated: a non-owner host resolved a non-owner
  author to the manifest's `viewer` default and DENIED that author's
  readings at the A3 inbound gate (`admit_inbound_row`, fe-database
  `src/AGENTS.md` §handlers `replicated_row`), so the author's own durable
  creator copy was the ONLY copy of those rows and the author was not a
  ledger host for them. A hosted-shard filter then made the author drop the
  request as `NOT_HOSTED`, the denying host answered an honest-looking
  covered-empty, and the merged result was silently incomplete (the harness
  `distributed_query` scenario caught it; provisioning Editor roles in the
  harness would have hidden the production default instead of fixing it, so
  the filter was reverted). RP-6 (§2.6 settlement) closes that hole from the
  OTHER side: a non-host author now DECLARES itself a holder
  (`__holders/{shard}/{did}`, admitted Editor+ at the shard's petal scope)
  the first time it writes into a shard it does not host, and the collector
  explicitly waits `HOLDER_WAIT_MS` for every admitted holder before
  settling — closing the settlement-order hole that made the 2026-10-11
  revert necessary. `hosts ∪ holders(shard)` is therefore exactly the
  expected-answerer set the collector already waits for: the filter answers
  for precisely the peers whose silence the settlement logic already
  notices, never for a peer the collector would otherwise have to guess
  about. The merge still does the right thing with a holder's rows
  (`merge_aggregates`: a formal host's NON-EMPTY answer wins, else any
  non-empty answer; `fold_attribution` credits row-bearing shards as
  covered) — RP-7 only stops OTHER peers from answering pointlessly.

  **Revert trigger (critic tick C7, RUNBOOK §3 sequencing).** This filter
  shipped as its OWN commit and is reverted alone — restoring the old
  no-filter pinning test — if any RP-6 settlement test or the harness
  `distributed_query` scenario (RP-8) is red in the final sweep; it never
  ships on a red RP-6. RP-9's critic pass must explicitly endorse keeping
  it. Pinned by
  `a_declared_holder_serves_its_creator_copy_and_a_non_holder_stays_silent`
  (requester → declared holder + denying host over an in-process wire, host
  delivered FIRST — the order the pre-RP-6 settlement and the old filter
  both failed on — plus a third peer that is neither host nor holder and
  must stay silent; both the aggregate and the raw leg) and
  `the_responder_answers_an_unknown_ledger_shard` (the safe direction: a
  shard this responder's own ledger has no entry for is answered, not
  skipped).
- **Merge filter fails closed (W5s, OF-7a/b/c, 2026-10-25).** An
  unexpected answerer the requester's view cannot judge (`Unverified` — no
  owner pinned, petal unknown) contributes NO rows and is reported in
  `meta.unverified_answerers` (pre-fix its rows were merged). A formal
  host's NON-EMPTY aggregate rows win the shard even when it reports the
  shard deferred — the deferred flag only withholds the covered marker
  (`merge_aggregates`), and fe-database scopes that flag to the query's
  verse by the queue's verified delivery key (`ExecuteTsPartial.verse_id`,
  `InboundAdmissions::deferred_rows_in`; the payload `reading_id` must equal
  the doc key). Admitted holders are re-verdicted on every `rematerialize`
  (a revoked holder stops being an expected answerer; re-promotion
  restores it via the §2.5 re-read) and a holder's own `__holders`
  tombstone withdraws it.
- **Diagnostics echo is non-blocking.** The collector runs as a task on the
  sync thread's current-thread runtime (§sync-thread-blocking-io), so its
  `ComputeResultReady` echo is a `try_send` (drop-and-warn when the event
  channel is full) — the merged outcome has already gone out on the
  embedded reply channel, which is the authoritative answer. A blocking
  send here parked the WHOLE sync runtime behind a lagging event consumer:
  the harness `distributed_query` leg-1 retry loop never drains sync events,
  so the 65th echo froze alice's gossip/replica/command loop and the next
  query surfaced as an opaque "no outcome within the deadline" instead of
  the leg's real assertion. Pinned by
  `a_full_event_channel_drops_the_echo_instead_of_stalling_later_queries`.
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
   docs: `delivered_from` "is not the same as the original author"; the
   `DeliveryScope::is_direct()` semantics are byte-for-byte identical in 0.35
   and 0.101), so relayed deliveries cannot be wire-verified and fall back to
   claim-based admission. "Relayed" here means a gossip SWARM hop (plumtree
   forwarding), NOT an iroh relay SERVER: a connection routed through an iroh
   relay server is still an end-to-end-authenticated QUIC/TLS connection to
   that `EndpointId`, so relay-server routing does not weaken the
   direct-delivery identity check. Relays are therefore the residual hole
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
6. **Requester role (RP-5, §role-view).** A declared requester still needs
   Viewer+ at the REQUESTED PETAL's scope: `VerseFabric::role_at(from_did,
   petal_scope)`, `default_access` folded in as the ⊥ fallback. An
   unconverged petal map (no scope string buildable yet) falls back to
   `default_access` directly — there is nothing to quarantine and retry on
   a live gossip request. Below Viewer → `drop_reason::INSUFFICIENT_ROLE`.
   This is the item 5 residual's mirror image on the read side: item 5
   gates WHO may WRITE a `__shards` row, this gates WHO may READ a petal's
   readings through the compute plane.

`VerseFabric::note_entry` (`sharding.rs`, ledger admission):

4. **`__peers` is self-declaration only.** A `__peers` row's key
   (`record_id` = `__peers/{did}`) must name its own author (`change.author_id`,
   the doc entry's signed author — never the row bytes). A row declaring
   ANOTHER peer's capacity (e.g. publishing `__peers/{honest_did}` with
   `capacity_bytes: 1` to starve that peer out of future placement) is
   dropped with a warn. The honest path is unaffected: replica open,
   `SetShardDeclaration`, and the restart snapshot all write the local
   peer's OWN key under its own author identity.
5. **`__shards` Editor+ gate — CLOSED (RP-5, role_propagation_20261013).**
   Previously evaluated and NOT implemented: the gate needed the author's
   role resolved at the shard's scope, but roles were DB-thread-only state
   (the local `role` table) and `__shards`/`__peers` rows are consumed at
   the sync seam and never reach the DB thread, so the sync plane had no
   role information to gate on. RP-5 closes this by giving the sync plane
   its OWN role view (§role-view): `VerseFabric` now runs the shared
   `fe_policy::role_grants` materializer over `role` doc rows (both
   directions) independently of, and in addition to, the DB plane's own
   admission of the same rows. `admit_shard_entry` (`sharding.rs`) requires
   Editor+ at the shard's own petal scope (`role_at`, `default_access`
   folded in as the ⊥ fallback) before a `__shards` row is applied; a row
   whose author's role hasn't converged yet, or whose petal isn't known
   locally yet, is **quarantined** (capped at 1024/verse, drop-oldest with
   a counter) rather than denied — the replica still holds it, and the
   §role-view re-read on empowerment (or the next replica open's
   reconciliation replay, for a petal-map wait) recovers it. A declared
   peer below Editor at the shard's petal can no longer inject a ledger row
   that misdirects placement/retention; the old residual risk this item
   recorded is gone with it.

### §gossip-bootstrap — the Join-drop race (fixed 2026-10-08)

`subscribe_to_verse_gossip_topic` (sync_thread.rs) must run **before** the
open sequence, and it must register every bootstrap peer in the endpoint's
address book (`SyncEndpoint::add_peer_addr` → iroh 1.x `MemoryLookup`, the
replacement for 0.35's `Endpoint::add_node_addr`) before
`gossip.subscribe(topic, bootstrap).await` (async on iroh-gossip 0.101 — the
open handler awaits it, so the ordering is unchanged):

- **Why register addresses first:** the one-shot `subscribe` dials the bootstrap
  peers immediately; a bare `EndpointId` with no address-book entry cannot be
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

## §tileset-transfer (PAS-P3 S2, 2026-10-13 — `blob_transport.rs`, `tileset_transfer.rs`)

A tileset travels as one HashSeq `[index, bundle_0 … bundle_n]`: child 0 is
the opaque index blob. That is fe-format `TilesetBundleIndex` JSON for a
legacy unsealed tileset, or since PAS-P5 S5 the signed `HeaderDoc` JSON for
a sealed one (`fe_terrain::tiles::bundle_for_seeding` picks the shape;
fe-sync passes either through unparsed). Children
1.. are the self-contained `.hexon` bundle archives fe-terrain's bundler
writes. **fe-sync never parses fe-format types**: it verifies bytes against
HashSeq children only.

**Why a trait on iroh 1.3 (D-77 drift note).** Decision E2 said "build on
0.35 behind a transport trait"; the stack is now iroh 1.3 / iroh-blobs
0.103.1, and the trait stays for three reasons: tests run on the
production-compiled `InMemoryNetwork` (no sockets), PAS-P5's HTTP
partial-fetch impl plugs in behind the same seam, and iroh-blobs is pre-1.0
(its API churns; only `IrohBlobTransport` absorbs that). The trait lives in
fe-runtime (`fe-runtime/src/AGENTS.md` §blob-transport); this crate holds
the iroh impl and the service.

**`IrohBlobTransport`** (over the `DocsStack` fs store + endpoint; `from_stack`).
`add` keeps a `TempTag` until `set_tag` covers the blob (the named tag is a
`HashAndFormat::hash_seq`, so it protects root + children; the fs store has
no GC by default, but everything we keep is tagged anyway). Fetches dial the
provider with `Endpoint::connect(EndpointAddr, iroh_blobs::ALPN)` — the
`ProviderRef` must parse as `EndpointAddr` JSON (never a URL), and a full
`EndpointAddr` carries its own addresses, so no `add_peer_addr` round-trip
through `SyncEndpoint` is needed — then `Remote::fetch(conn,
HashAndFormat::raw(h))`, which skips locally complete content. A HashSeq
root-only fetch is the root fetched as a raw blob (the same bytes the
`GetRequest::builder().root(..)` form would ask for). One connection per
provider is cached and evicted on failure. Every await is bounded:
`CONNECT_TIMEOUT` 15 s, `FETCH_TIMEOUT` 60 s, `LOCAL_STORE_TIMEOUT` 30 s.

**`TilesetTransfer`** (pure orchestration over `Arc<dyn BlobTransport>` and
a download root):
- `publish` → HashSeq + tag `fe/tileset/<id>` + catalog entry `{id, root,
  total_chunks, bytes}`; `catalog_from_tags` rebuilds the catalog at start
  (tags persist in the fs store) and MERGES into it — startup seeding can
  publish while the rebuild runs, and a replace would drop that entry from
  every later advertisement; `unpublish` drops the tag.
- `request_meta(providers, id, root)` resolves the root from the first
  provider whose children hash back to it, tags `fe/download/<id>`, writes the
  ledger, and fetches + verifies the index. The same root reuses the ledger
  (providers merged); a new root restarts the download; an unreadable ledger
  is discarded.
- `fetch_chunk` / `download` store each bundle only when
  `blake3(bytes) == chunk_hashes[seq]` and it fits `MAX_CHUNK_BYTES`, with an
  atomic tmp + rename write. `download` runs `MAX_CONCURRENT_CHUNK_FETCHES` (4)
  at once, tries providers in candidate order, and reports per-chunk
  `TransferEvent`s plus a `DownloadReport`. **`download` refuses
  (`TransferError::AwaitingConfirmation`) until `confirm` has run** — see
  §partial-fetch-p2p.
- `pending_downloads` / `resume_all` re-verify files and fetch only missing
  seqs; already complete downloads are listed, not re-run, and only a ledger
  a full `download` marked (`full-download`) is resumed — a streamed-bundle
  cache is never grown into a full download (§tileset-security). `resume_all`
  additionally skips any ledger that is not `confirmed` (§partial-fetch-p2p).
  `cancel` deletes the ledger, the files and the download tag. A completed
  download stays on disk until `cancel` (the host discards it after install;
  S7 keeps it as a streaming cache).

On-disk layout, under `<FE_P2P_DIR>/downloads/<id>/`: `download.json`
(`{schema, tileset_id, root, provider_candidates, chunk_hashes, index_hash,
confirmed}`, hashes hex; `confirmed` added PAS-P5 S7a,
`#[serde(default)]` so a ledger from before this slice reads `false`),
`index.json` (child 0) and `<seq>.bundle`. **Invariant: "received"
is derived, never stored.** It is recomputed from files whose BLAKE3 verifies,
so the ledger can never claim a chunk the disk does not hold. A ledger read
back from disk must re-prove `hash_seq_root([index] + chunks) == root`, or it
is refused, so local corruption cannot redirect verification.

**Untrusted input.** `tileset_id` arrives from advertisements and becomes a
path component and a tag suffix, so `TilesetTransfer::download_dir` is the
single choke point. Every public method validates the id through it before
any join or tag write: the S1 charset `[A-Za-z0-9_.-]`, non-empty, not `.`/`..`,
at most 128 bytes, and no Windows device stem (`CON`, `NUL`, `COM1`, …). A
refused id is `TransferError::UnsafeTilesetId` and nothing is touched. The
guard itself is the shared `fe_runtime::safe_id::is_safe_path_id`
(fe-terrain's install path uses the same function). Size caps are enforced
before a blob is downloaded — see §tileset-security.
All filesystem work runs on `spawn_blocking` (§sync-thread-blocking-io / NFR-3).

### Sync-thread wiring (PAS-P3 S3, `sync_thread.rs`)

**One `TilesetTransfer` per online stack.** Built right after `DocsStack`
spawns (`Arc<dyn BlobTransport> = IrohBlobTransport::from_stack(&stack)`,
`TilesetTransfer::for_p2p_dir(transport, &p2p_dir)`); `None` in offline or
virtual-transport mode. Every tileset command handler takes `&Option<Arc<
TilesetTransfer>>` and answers the offline case honestly (a
`TilesetMetaReceived`/`ChunkFailed` whose reason says `"P2P offline"`) rather
than hanging or silently dropping — this is what keeps the command loop
byte-identical between the real and virtual transports (§virtual-transport).
At startup a spawned task (`report_and_resume_tileset_downloads`) rebuilds
the catalog from persisted `fe/tileset/` tags (`catalog_from_tags`), reports
each FULL download `pending_downloads` finds as
`SyncEvent::TilesetDownloadResumed` (received/total BEFORE anything is
re-fetched; a streamed-bundle cache is not a download and is not reported),
hands every already-complete full download back as
`TilesetDownloadComplete` (`completed_from` — a restart between completion
and install would otherwise strand it: `resume_all` skips complete ones),
then runs `resume_all` to finish the rest. The host discards a download
with `CancelTilesetDownload` once installed, so a complete download still on
disk at start means "not installed yet". A `download` that fails as a
whole (e.g. `DownloadTooLarge`) answers with the empty-root
`TilesetMetaReceived`, which fe-ui shows as Failed.

**The select! loop never awaits a transfer inline.** Every handler that
calls into `TilesetTransfer` (`publish_files`, `unpublish`, `request_meta`,
`download`, `fetch_chunk`, `cancel` — all bounded by `CONNECT_TIMEOUT`/
`FETCH_TIMEOUT`/`LOCAL_STORE_TIMEOUT`, up to 60s) runs inside a `tokio::spawn`
the match arm fires and returns from immediately; only the gossip broadcast
helpers (`handle_advertise_tilesets`, `broadcast_ads_on`) stay awaited
inline, like every other lightweight gossip send in this file. Per-bundle
progress (`TransferEvent::{ChunkStored,ChunkFailed,Complete}`) is forwarded
to `SyncEvent`s by `pump_transfer_progress`, spawned alongside the operation
over a fresh one-shot `UnboundedChannel` — not a single long-lived channel
routed through the main `select!`, since nothing in the invariant requires
that and a per-operation channel is simpler to reason about.

**`TilesetProviderMap`** (`tileset_id -> known providers`, authenticated
ones sorted first, replaces the dead Phase-F.5 `TilesetDownloadTracker`
HashMap) is what `RequestTilesetMeta`/`DownloadTileset` resolve a root +
candidate providers from — session-only, never persisted (the durable state
is `TilesetTransfer`'s own ledger/catalog). An explicit provider override
(`RequestTilesetMeta.peer_id`, `DownloadTileset.provider`) is tried first,
then every advertised provider. **`RequestChunk.provider` is accepted on the
wire but not yet consulted** (`handle_request_chunk`'s doc comment) — a
chunk fetch always rides the ledger a prior meta resolution already built.
This is not a gap for S7 (whose streamed source always resolves meta
before requesting a chunk); it only matters for a provider-only chunk
request with no prior meta resolution, which no caller does today.

**Advertisements, both directions:**
- *Outbound* (`handle_advertise_tilesets`) is always built from the
  published catalog, never from `AdvertiseTilesets.advertisements_json`
  (kept on the wire for back-compat; fe-ui still sends `""`). `PublishTileset`
  additionally broadcasts its one new ad on **every currently-joined verse
  topic** (`broadcast_ads_to_all_verses`) — publish has no single verse to
  target.
- *Inbound*: `decode_tileset_ad` is tried before `handle_gossip_incoming`
  (§iroh-1.x). Admission mirrors the §transport-admission-control item 2
  pattern: `tileset_ad_is_authenticated(direct, claimed_provider_addr, from)`
  is true only when the delivery was direct AND the ad's own
  `provider_addr` parses to the authenticated sender's `EndpointId`.
  Unauthenticated ads still record as separate hint entries — they can never
  rewrite what an authenticated ad recorded (§tileset-security), and they
  never skip the per-bundle BLAKE3 check `TilesetTransfer` runs regardless
  (Risk 1 in the campaign plan — the real fix is D-76 D2 in PAS-P5).
- The dead three-field `TilesetAdvertisement` struct is gone; `TilesetAd`
  (tagged `kind: "tileset_ad"`, distinct from `ComputeEnvelope`'s `type` tag
  so the two decoders can never collide on one topic) is the only wire
  shape fe-sync emits or parses. fe-ui's richer fe-terrain-shaped DTO
  (region_name/bounds/zoom) still reads a `PeerTilesetAdvertisement` payload
  that only carries `tileset_id`/`root_hash`/`total_chunks`/
  `approx_size_bytes` today — it degrades gracefully (its `.get(..)
  .unwrap_or(default)` reads), and S4 aligns the two shapes.

**Bounded buffers.** `drain_sync_events` caps `TilesetEventBuffer` and
`StreamedTilesetEventBuffer` at `MAX_BUFFERED_TILESET_EVENTS` (4096) and
drops (with a warning) past it. The headless relay has no consumer for
either, so it also clears `TilesetEventBuffer` every frame
(`discard_tileset_events`); before the final review it grew by one event
per advertisement for the life of the process.

**PAS-P1V:** `status.rs::drain_sync_events`'s `RelayHealthChanged` arm now
logs one unconditional `info!` before its existing problem/no-problem
branch, so the headless relay-health script has a reliable info-level line
to grep in every mode (`Disabled` included), not only the non-problem ones.

### §partial-fetch-p2p — the two-phase download gate (PAS-P5 S7a, 2026-10-14)

`SyncCommand::DownloadTileset` resolves the root and child 0 **only**, emits
`TilesetMetaReceived`, and stops — **no chunk is ever requested** from this
command alone (`handle_download_tileset`). A chunk fetch starts only once
`SyncCommand::ConfirmTilesetDownload { tileset_id, expected_chunk_hashes }`
arrives (`handle_confirm_tileset_download`), because fe-sync itself cannot
tell a sealed header from a forged one (format-agnostic by design, §top of
file); the host (`fractalengine::terrain_bridge`, which does parse
`fe_format`) is the one place that can.

- **`TilesetTransfer::confirm(tileset_id, expected_chunk_hashes)`** refuses
  (`TransferError::BadIndex`) unless `expected_chunk_hashes` is byte-for-byte
  the SAME hashes `request_meta` already resolved from the HashSeq, in the
  same order — this is what catches a signed chunk index naming fewer (or
  more) bundles than the HashSeq itself carries (a seeder padding in an
  unsigned extra chunk, or a tampered index): the confirm is refused outright
  and the sync thread cancels the download, so the untracked chunk is never
  reachable through `download`/`resume_all` at all, not merely unverified.
  On success it is idempotent and persists `Ledger.confirmed = true`
  (`LEDGER_FILE`'s `confirmed` field). **It also refuses
  (`TransferError::NotRequested`) a download that no
  `request_download_meta` asked for in this process.** `handle_download_tileset`
  is the only production caller of `request_download_meta`, and a plain
  `request_meta` (the streamed path) never qualifies. Final review
  (2026-10-14): before this, a confirm for a streamed cache's id ran
  `download`, wrote `FULL_DOWNLOAD_MARKER`, and turned the cache into a full
  download that fe-ui auto-installed. The in-memory mark is bounded by
  `MAX_REQUESTED_DOWNLOADS` and cleared by `confirm` or `cancel`. Test:
  `a_streamed_cache_is_never_confirmed_into_a_full_download`.
- **Start-up discards unconfirmed full downloads.** A ledger with
  `FULL_DOWNLOAD_MARKER` that is not `confirmed` predates the gate (since
  the gate shipped, the marker is only written after a confirm), and nothing
  will ever confirm it. `report_and_resume_tileset_downloads` therefore
  cancels it. It is never reported as `TilesetDownloadResumed`, and
  `completed_from` never hands it back for install. Test:
  `an_unconfirmed_full_download_is_discarded_at_startup_never_installed`.
- **`download`/`resume_all` check `ledger.confirmed` before anything else** —
  before `ensure_index`, before the `FULL_DOWNLOAD_MARKER` is even written.
  An unconfirmed download therefore never becomes a `full_download` `pending_downloads`
  would hand back as `completed_from`, however many individual chunks a
  caller fetches one at a time through `fetch_chunk` (the streamed,
  not-installed path `RequestChunk` drives — deliberately NOT gated here;
  see below). This is why a restart never re-trusts an unverified header:
  confirmation lives on disk, not only in the (lost) in-memory state of the
  process that received the original `TilesetMetaReceived`.
- **`start_download`** (`request_meta` + `confirm` + `download` in one call)
  auto-confirms with whatever the HashSeq itself resolved — it is a
  test/internal convenience with NO production caller; the real network path
  always drives the three steps separately through `SyncCommand`s so a host
  can verify in between.
- **The streamed (not-installed) single-chunk path is intentionally
  ungated.** `RequestTilesetMeta`/`RequestChunk` (fe-terrain's
  `StreamedTileSource`, via `fractalengine::terrain_bridge::drive_streamed_tilesets`)
  call `fetch_chunk` directly and never go through `confirm`. This is safe
  by construction (and `confirm` refuses a never-requested id, above):
  `fetch_chunk` alone never writes
  `FULL_DOWNLOAD_MARKER`, so no amount of one-at-a-time streaming can ever
  make `pending_downloads` report a `full_download` (and so
  `completed_from`/auto-install never sees it) regardless of whether
  `confirm` ever ran. Each streamed bundle is still checked against the
  (possibly signed) index by `fe-terrain`'s own
  `CompositeTileSource::streamed_bundle_check` before it is decoded —
  a narrower, per-tile guard than the install-flow's two-phase gate, matching
  the narrower thing it protects.
- **Publisher-DID pin provenance (critic tick 4).** `TilesetAd.publisher_did`
  (wire-optional, `#[serde(default)]`) is carried into
  `TilesetProvider.publisher_did` by `TilesetProviderMap::record` and
  resurfaces on `TilesetMetaReceived.ad_publisher_did` — **always the SAME
  entry `root_for` chose its root from**, never a different entry's claim
  (`RootChoice` carries root + authenticated + publisher_did together,
  test: `provider_map_prefers_authenticated_roots_and_ignores_relayed_claims_about_them`).
  fe-sync never decides whether to trust it; the host's TOFU pin is "locally
  known publisher for this `hexon_id`, else `ad_publisher_did`" — see
  `fractalengine/src/AGENTS.md` §tileset-p2p for the verification side.
- **`CatalogEntry.publisher_did`** is a best-effort hint read by
  `publisher_did_hint` — pure `serde_json::Value` duck-typing over the index
  blob (a sealed `HeaderDoc`'s `manifest` field is itself JSON text naming
  `publisher_did`; a legacy unsealed index has neither shape and yields
  `""`). This is NOT a `fe_format` dependency: fe-sync parses a generic JSON
  tree, never a typed `HeaderDoc`/`HexonManifest`.

### §tileset-security (PAS-P3P4 S8b adversarial review, 2026-10-14)

What a hostile verse member (or anyone relaying on a verse topic) can send,
and what stops it. Each guard has a named regression test.

- **Provider-map poisoning** (mandatory finding #1). `TilesetProviderMap`
  entries are keyed `(verse, provider, root)`. A relayed ad can only ADD a
  hint entry; it is refused outright when it names a provider that vouched
  for itself directly, so it can never replace an authenticated root.
  `root_for` answers the newest authenticated root, else the newest hint
  (logged as such in `resolve_providers_and_root`); `candidates` returns
  only providers that advertised THAT root. Test:
  `a_relayed_ad_cannot_replace_an_authenticated_root` (a real download
  fetches A's bytes).
- **Verse binding.** Every entry remembers the verse topic it arrived on;
  `CloseVerseReplica` calls `forget_verse`, so a verse this node left stops
  feeding it providers.
- **Ad storms.** At most `MAX_TRACKED_TILESETS` (1024) ids and
  `MAX_PROVIDERS_PER_TILESET` (16) entries per id; a full map refuses new
  hint-only ids and evicts the stalest hint-only id for an authenticated
  one. An ad with an unsafe id or an undialable provider is dropped before
  it costs map space or a UI event. Test: `tileset_ads_are_verse_bound_validated_and_capped`.
- **Dialing.** A wire-supplied `ProviderRef` is capped at 4 KiB and
  `sanitize_provider_addr` keeps only its endpoint id, IP addresses and
  relay URLs already in this node's relay map (`with_allowed_relays`, fed
  from `RelayConfig`). This constrains the supplied address, not every route the
  shared endpoint can retain or discover. The historical global SSRF-immunity
  conclusion is unestablished; see section relay-routing-contract (DQA022). The pure
  test `a_provider_address_keeps_only_trusted_relays` checks supplied-address filtering
  only, not actual endpoint-wide network egress.
- **Size caps before download.** `BlobTransport::fetch`/`fetch_hash_seq`
  take the caller's cap. `IrohBlobTransport` first asks the provider for
  the blob's BAO-verified size (`get_verified_size`: the last chunk's proof
  binds the size to the hash) and refuses an over-cap blob before any of
  its content enters the fs store. `TransferLimits` (per service; tests use
  small ones) sets the bundle cap (64 MiB), the index cap (16 MiB) and the
  total-per-download cap (`MAX_DOWNLOAD_BYTES`, 2 GiB, since an install caps
  at 1 GiB decoded); `MAX_HASH_SEQ_CHILDREN` is 1 + 65 536. An over-cap
  download stops spawning fetches and returns `DownloadTooLarge` with no
  `Complete`. Tests: `an_oversized_bundle_is_refused_and_never_persisted`,
  `a_download_over_its_total_byte_cap_stops_without_completing`, and the
  loopback over-cap assert in `iroh_transport_round_trips_a_hash_seq_on_loopback`.
- **Ledger tampering.** `download.json` is size-capped (8 MiB), its bundle
  count is capped, and it must still hash to its root. A tampered ledger
  is skipped, so the providers it names are never dialed. `resume_all`
  logs and skips a failing download instead of aborting the others. Test:
  `a_tampered_ledger_is_never_resumed`.
- **Streamed-cache disk budget (PAS-P5 S5, critic AC-21a).** A streamed
  cache (ledger, no `full-download` marker) records its last use in
  `downloads/<id>/last-used` (unix ms), stamped on `request_meta` and every
  `fetch_chunk`. After each `fetch_chunk` and at sync-thread start
  (`report_and_resume_tileset_downloads`), `trim_streamed_caches` evicts the
  least-recently-used caches (`cancel`: files, ledger, tag) until all of
  them fit `TransferLimits::max_streamed_cache_bytes` (2 GiB default).
  Three things are never evicted: a full download, whether in progress or
  complete and awaiting install (the marker); a cache with a meta/chunk
  request running (an in-memory busy set); and the cache whose bundle was
  just stored. So one oversized cache can exceed the budget rather than
  delete the bytes `ChunkReceived` points at. Installed tilesets live in the
  hexon store, never under `downloads/`. Test:
  `streamed_caches_past_the_disk_budget_evict_least_recently_used_first`.
- **Streamed caches stay caches.** `fetch_chunk` (the streamed path, which
  a peer-replicated petal can trigger) never writes the `full-download`
  marker, so a restart never turns it into a full download and auto-install.
  Test: `a_streamed_bundle_cache_is_not_resumed_as_a_full_download`.

**Risk 2 — blob-ALPN request filter: CLOSED (RP-5 step 6, critic tick 4
AC-21(e)), partially.** The revisit trigger ("when RP replicates roles...")
has fired: `docs_engine.rs::spawn_blob_access_filter` now builds
`BlobsProtocol::new(&store, Some(event_sender))` with `get:
RequestMode::Intercept` — and since W5s (OF-6c) `get_many: Intercept`
(every requested hash must be admitted), `observe: Intercept` (a bitfield
is an existence oracle) and `push: Disabled` pinned explicitly. The
filter keys on the REQUESTED HASH, not the connection: `BlobAccessGate`
tracks every tileset HashSeq this node publishes or fetches — root AND
children — as a `RootBinding`, plus a per-verse access snapshot
(`VerseFabric::access_snapshot(verse_id)` — owner, `default_access`, and
every DID an admitted role statement resolves to Viewer+ AT THE VERSE
SCOPE; a petal-scoped grant never opens verse-bound tilesets, OF-6e).
**Binding model (R1 redesign, 2026-10-25 — replaces the W5s OF-6a/b
patches after OF-8 found two HIGHs in them).** One invariant: *a binding
is set only by an AUTHORITATIVE local event and gossip never mutates it.*
- A `RootBinding` is `published` (this node's `fe/tileset/` content — own
  publish or catalog rebuild: a Viewer+ of ANY open verse, the publish path
  advertising to every joined verse) and/or `learned` — the fetch context
  of this node's FIRST own fetch decision (`bind_fetched_root`, called by
  `RequestTilesetMeta`/`DownloadTileset` BEFORE any byte lands): every
  verse the tileset id (any root) or the root (any id) was advertised on
  then (`TilesetProviderMap::verses_for`); a Viewer+ of EVERY one is
  admitted, a closed verse denies. **Fixed once set** — a later fetch
  context never re-binds it (no remap, OF-6a) and never narrows it, and an
  inbound ad has no entry point into the gate at all (the W5s
  `note_advertised_root` is gone: its post-fetch narrowing plus a persisted
  64-verse overflow was an attacker-triggerable, durable refuse-everyone
  state, OF-8b). Fetch-context integrity: provider-map eviction never drops
  the last entry of a verse while another verse holds several (a hint flood
  on the attacker's verse cannot shrink the victim's binding to the
  attacker's verse alone). Test:
  `a_root_bound_to_its_first_fetch_context_is_never_rebound_or_narrowed`,
  `an_attacker_cannot_shrink_a_fetched_roots_bound_verse_set`.
- **Children: claimants AND-ed (OF-8a).** Children are reported by
  `TilesetTransfer`'s `ContentObserver` (publish, meta resolution before
  any child is fetched, catalog rebuild, the start-up
  `report_downloaded_content` tag scan) and set once per root
  (content-addressed). A hash's claimants are its own binding (if a root)
  plus every root listing it; it is admitted iff some claimant is an own
  PUBLISHED root that admits on the publish path, or EVERY claimant admits
  on its learned path. A fetched HashSeq's child list is attacker-shaped
  content: under the W5s `.any(..)` a fetched attacker tileset listing a
  victim's bundle hashes lent the attacker's (self-controlled) membership
  to the victim's bytes. Now such a listing only adds a claimant that must
  ALSO admit — in either learn order. Test:
  `a_fetched_hash_seq_listing_a_victims_children_never_lends_them_its_viewers`.
- **Child-range requests (R1).** A `GetRequest` whose ranges reach past
  offset 0 makes iroh-blobs stream every child the root blob lists. The
  filter computes the hashes such a request would stream
  (`get_request_hashes`): only a tracked root with recorded children is
  answerable (root + every child must admit); a child-range request on any
  other hash is refused — otherwise an UNTRACKED HashSeq this node stores
  (a replicated row's content blob listing a protected bundle's hash)
  launders tracked bytes past the per-hash gate. No workspace client sends
  one (`IrohBlobTransport` and iroh-docs fetch raw). Test:
  `a_child_range_request_through_an_untracked_hash_seq_cannot_launder_a_tracked_bundle`.
- **Persistence.** The map persists to `<p2p_dir>/blob-access-gate.json`
  (`format: 3`) and loads before the router accepts a request; a format-1
  (W5s) file's `learned` sets — which may carry gossip narrowing — are
  DROPPED on load (fail-closed; the next own fetch re-learns: the
  reversibility OF-8b asked for). A format-2 (R1) file's in-data
  `"\u{0}unvouched-provider"` marker is migrated to the `never_seed` flag
  (R6). Tests:
  `a_pre_r1_state_file_drops_its_gossip_narrowed_bindings_and_relearns_on_the_next_fetch`,
  `an_unvouched_root_stays_unseeded_even_if_a_verse_snapshot_collides_with_the_old_sentinel`.
  A closed verse only drops its snapshot — its roots stay tracked and DENY
  rather than fall back to "untracked → admit".
- **Provenance rule (R1).** A relayed hint can name a THIRD-PARTY provider
  that serves this node on the strength of this node's membership in some
  other verse; re-serving those bytes to the hint's verse would launder
  them one hop past that provider's own policy. So when any provider a
  fetch may use never VOUCHED for the tileset/root itself (no
  authenticated entry from that endpoint — `vouched_providers`), the root
  is bound `never_seed` (`bind_fetched_root(.., vouched: false)`): fetched
  for local use but never secondary-seeded; the flag is fixed with the
  first fetch decision like `learned`. (R1 marked this with a sentinel
  verse id inside `learned`; OF-10c replaced it with the flag because a
  sentinel in data is one colliding `set_verse_access` away from failing
  open.)
  Test: `a_root_fetched_through_an_unvouched_provider_is_never_secondary_seeded`.
- **Residuals (fail-closed availability only, recorded):** (1) a fetch
  context may include verses an attacker advertised the tileset/root on
  BEFORE this node's first fetch (only verses this node has open — ads
  arrive only on joined topics), or an attacker hint may add an unvouched
  provider; either narrows or disables secondary seeding of that root on
  this node (the publisher still serves), never widens it. (2) A fetched
  attacker HashSeq listing victim children narrows the victim's secondary
  seeding the same way — and needs this node's own fetch decision of the
  attacker's tileset; two legitimately fetched tilesets from different
  verses sharing a byte-identical bundle narrow that bundle likewise (the
  AND is the price of order-independence).
A raw `GetRequest`
for a hash NOT in that reverse map carries **no tileset context** and is
let through unconditionally — this is how `FetchBlob`/
`two_peer_blob_exchange` and CDL-SPEC10 A6's device-payload fetch, and
ordinary iroh-docs entry-content fetches, stay unaffected exactly as this
risk note originally required. A request for a tracked hash is admitted
only per the claimant rule above, for the requesting `EndpointId` (mapped
to a did:key the same way the A3/§role-view gates do, via a
`ClientConnected`-notify-populated `connection_id -> did` table, since the
request event itself carries no identity). `ConnectMode` stays at `Notify` (never `Intercept`) —
this is a hash-keyed filter, not a membership-gated connection accept.
**"Partially" — the residual this leaves, recorded honestly (critic tick 8
C20, ledger row BLOB-HASH-CAPABILITY):** a hash never recorded as a
tileset root — an ordinary row/asset blob, or a `two_peer_blob_exchange`
hash — stays open over this ALPN to anyone who knows its 256-bit hash,
exactly as before. Closing THAT needs a membership-keyed filter for every
hash this ALPN serves, or namespace rotation — out of this item's scope;
it is a separate, still-open trigger.


## distributed-catalog-restart

A fresh fabric must restore owner plus Petal-to-Fractal ancestry before a
production distributed query is admitted. An unresolved catalog now returns
an explicit not-ready outcome, rather than a zero-target empty success. A
known Petal with no ledger can still legitimately yield an empty plan. This
checks knowledge, not caller authorization; existing API scope and sync role
admission remain in force.

Replica open requests the bounded DB-authorized local catalog re-emit before
its existing role-statement replay. The repair restores Verse/Fractal/Petal
rows from stored Manager-fenced readback, including runs created before Petal
publication was fixed. The normal own-author bookkeeping seam maps the Petal
and reevaluates quarantined ledger rows through existing role admission; it
never bypasses that admission or re-ingests local readings. Viewer-only nodes
cannot sign a reconstructed catalog and rely on admitted replica convergence.

The existing reconciliation journey models own persisted ledger plus missing
Petal, not-ready state, and recovery after the own catalog row arrives without
changing reading counts. Existing runtime/backpressure and durable DB journeys
cover the command order and honest refusal. Source presence is not executed
restart, fleet or timeout proof. Archived pass13 shows a real online same-key
stack replaying 40 entries but an empty distributed plan after restart; local
history/readings/blob persistence passed separately.

## blob-store-root-permissions

`FsBlobStore::new` creates or opens the root and already attempts to set Unix permissions to `0700`. Metadata and permission-write failures retain their existing best-effort behavior. This cleanup changes no permission handling, Windows ACLs, or directory initialization. The unsupported unsigned `NodeTransformed` status branch remains diagnostics only; signed row and canonical admission paths are unchanged.

## Terminal maintenance forwarders

Sync shutdown closes the async command receiver, requests the crossbeam forwarder to stop, and joins it before reporting completion. Sender clones cannot retain an orphan receiver. The distributed-query bridge also exposes an owned, stop-aware variant; bounded send retries observe shutdown. Legacy convenience callers retain their previous detached lifetime.


## Join tickets

`join_ticket::NativeJoinTicket` parses the native iroh-docs ticket and checks
canonical encoding, exact namespace and selected capability, the issuer's bound
EndpointId/DID, at most eight peers, and bounded usable IP/relay addresses. Viewer
requires READ, while Editor/Manager require WRITE; unsupported custom transports
refuse. The wrapper's Debug never exposes its capability bytes. Export uses the
actual open native document and refuses offline/mock/expired requests.

Join imports capability with `import_namespace` without starting synchronization,
then enters the ordinary gossip/open pipeline. Actual document subscription is
installed before start_sync. Its correlated open result confirms only pipeline
installation; readiness separately requires a live document pump and successful
SyncFinished from the exact invited issuer on that new replica generation. A
closed/failed pump cannot confirm readiness. The bounded per-generation peer set
contains at most256 successful identities and clears for each new subscription.
Host readiness must also confirm actual stored ancestry and effective role.

Capability import is a persistent effect in another store. Expired/time-out or
failed connections therefore report an unconfirmed or partially admitted outcome;
they never claim that no data changed or automatically retry. Final application
join success is not emitted by this native transport adapter alone. The existing
fresh-peer manifest journey now exports/imports an actual cold READ ticket and
requires Viewer native writes to refuse, but remains unexecuted until root sweep.

A live-event stream error clears all earlier successful peer synchronization
proofs while leaving the normal event reader available for recovery. Another
successful SyncFinished is required; an unknown missed NeighborDown or failure
cannot preserve readiness. The existing real-transport journey drives a bounded
continuing error stream through the actual pump and checks this invalidation.

Canonical native ticket parsing is now owned by fe-database::join_ticket, a pure
admission module. The sync module re-exports that one policy and owns only live
export, capability import, subscriptions, dial and readiness effects. This avoids
DB-to-sync waits and duplicate ticket parsing rules; no database command trusts
a caller-supplied native-validation boolean.


Joined native pipelines retain a local generation/Verse/namespace lease. Native readiness must
match that lease, as must the local-only CloseJoinedReplica command. Ordinary Open/Close
invalidates it. Exact conditional close enters the ordinary close branch, including gossip,
provider and blob-access cleanup. A stale dialog or superseded handoff cannot close a newer
pipeline with the same Verse ID, and no new transport or wire protocol is introduced.


Join preparation refuses a target already present in the replica map before importing a
capability or replacing its pipeline. The GUI advises leaving that Verse before accepting the
new invitation. A refused request installs no native generation lease and cannot make its later
conditional cleanup close the existing navigation pipeline; ordinary Open behavior is unchanged.


## scene-assets

Scene asset byte transfer is separate from iroh-docs row-content transfer and from
tileset publishing. Native UI uses FetchSceneAsset with an owned request ID. Jobs
coalesce equal hash/Verse/open-generation requests, fan out SceneAssetFetchSettled
for at most 256 exact owners, and also emit BlobReady only after verified shared-store
persistence. Hash-only failure events are advisory and cannot settle a newer owner.
Compatibility FetchBlob requests enter these same independently admitted verified
scene jobs, never a presence-only readiness path or untrusted provider route.

The private scene_asset runtime protocol and DQA020 adoption are described in the
DB handlers directory. These contracts have no public REST/MCP route and no local
object-history claim. Their source implementation and tests still require independent
review and the end-of-campaign integrated sweep before acceptance.


### Scene byte lifecycle and authorization

scene_assets owns four active jobs and 64 waiting jobs, a 120-second outer work
deadline, bounded private DB requests, and at most 16 providers captured from a
successfully opened native Verse. Requests never supply arbitrary provider URLs.
IrohBlobTransport filters explicitly supplied provider relay URLs and pins EndpointId
through the authenticated connection. The shared native endpoint can also use retained
or discovered routes; its actual relay egress is not globally allowlisted (DQA022;
see section relay-routing-contract). Fresh provider-size admission runs even for a
native cache hit; provider BAO size, content digest, declared size/MIME and the texture
guard are checked before local persistence. Missing glTF external resources remain
a separate dependency gap: transferring its root file does not certify those files.

Source bytes are read/verified from the shared canonical store on the blocking
pool, imported into the native blobs store, and retained by a named one-child
HashSeq tag. Temporary holds are owned by an RAII guard. Scene metadata is released
through a bounded internal completion channel only after those bytes are retained,
with exact open-generation validation. Non-scene metadata keeps its existing row
publisher. An unsuccessful source job leaves durable metadata for later recovery,
logs why it was withheld, and never claims delivery. Native named tags remain
retained until a separately authorized garbage-collection policy removes them.

Recipient jobs re-admit current stored metadata/Viewer authority before and after
the shared FsBlobStore write. The write and streaming readback verification are
awaited even after cancellation; shutdown joins all jobs before stopping the native
stack, so terminal reset cannot outlive a late shared-store write. Close/reopen
cancels the old job generation, suppresses its success, and cannot settle new request
owners. Already received bytes are not erased by cancellation or role loss (DQA015).
No current role guarantee is claimed for an already admitted in-flight remote stream.
Large existing LOCAL copies remain available independently from the 256 MiB network
cap; missing larger copies return TransferLimitExceeded instead of entering a job.

### Durable scene classification

Scene content does NOT inherit tileset Published's any-open-Verse permission.
scene_asset_gate persists a separate deny-by-default hash class for BOTH raw child
and retention root before either is added to native storage. Native get/get-many/
observe requests keep existing tileset gates AND require fresh private DB Serve
admission bound to the authenticated connection DID and stored current asset scope.
A closed Verse has no current authorizer lease. Receiving a copy never manufactures
local upload/adoption proof for secondary seeding.

The classification registry and activation marker live beside the native P2P store
and are never cleared with DB tables or on Close. Entries are never evicted; 16
contexts/hash and 8 MiB total refuse further publication instead of dropping old
protection. An activated missing, unreadable or malformed registry denies UNKNOWN
native content too and refuses new jobs, so restart/reset cannot turn protected
raw bytes back into the legacy untracked-blob allow path. This state requires a
separate safe storage recovery; deleting classification while retaining native
bytes is not a supported repair. Ordinary row-entry raw content remains unchanged
when the registry is healthy.

### Authored verification limits

The existing DB journey exercises actual scope/role/metadata/adoption transactions.
The existing real two-stack blob-filter journey additionally drives production
SceneAssetJobs and IrohBlobTransport over loopback into separate FsBlobStores,
using an explicitly controlled typed DB-admission fixture. It authors persistence,
coalesced owner finality, provider revocation despite a native cache hit, old/new
generation separation, raw/root restart denial and lost-registry refusal. This is
not an executed test and is not a native two-application database/identity proof.
Parent owns that live paired verification, the final single test sweep, CI and
release acceptance. Object history, models and field-definition replication remain
separate and are not made convergent by blob delivery.

SceneAssetEventBuffer is the sole native projection handoff from drain_sync_events.
The GUI host registers it; native UI consumes it after that drain system. It caps
1024 events and sets overflowed on loss. A consumer must reconcile pending owners
through fresh current reads on overflow, never infer a completed request. Headless
hosts need not allocate the buffer. The existing undrained-buffer journey authors
its cap, exact request retention and overflow signal alongside tileset buffers.

The shared FsBlobStore validates existing hash-named files instead of trusting their
presence, repairs corrupt entries with verified caller bytes, and stages each writer
in its own create-new same-directory file. Atomic replacement and exact canonical
readback handle rename races; RAII cleanup removes only that writer's staging file.
The existing idempotent-add journey authors corrupt-cache repair and simultaneous
same-hash writes with no remaining temporary files. This narrows false readiness
from the previous shared temporary filename and existence-only deduplication.

Classification blocks the affected hashes while its durable write is pending. The
raw/root class file and activation marker are synced before the native add; a write
failure fails closed for subsequent requests/jobs. Pending counts and class capture
share one lock order so a request cannot observe new unpublished classification
contexts between its pending check and snapshot capture.

The node publisher marks scene-asset ENVELOPES with _fe_scene_asset:1 only after its
normal local provenance/authority preparation. It is a routing hint, never authority;
Publish/Serve repeat actual current DB admission. The receiver strips the hint along
with local-only provenance. Unknown marker versions are withheld with a diagnostic.
Ordinary API/device metadata, including GLB data used only as a sensor payload, keeps
its original publisher. A later placed-node scene envelope has a distinct row hash,
so an in-flight pre-placement metadata write cannot coalesce away byte publication.
Old peers can ignore the hint but do not gain upgraded byte-ready sequencing.

JobLease retains the four-slot resource through async work and clones it into
uncancellable blocking IO, so rapid Close/reopen cannot accumulate unbounded disk
workers behind cancelled async jobs. Runtime teardown drains finite blocking tasks
before the sync OS-thread join. Generic metadata classification reads remain one
small sequential loop operation, using the same blocking-pool file reader.

Scene asset review corrections bind every Publish/re-admission to the exact envelope
asset ID and Petal, while byte storage remains content-addressed. FetchBlob no longer
emits readiness for mere canonical-path presence; compatibility and owned requests
share bounded asynchronous admission/verification/readback jobs. Current read state
also verifies local file size/digest before LocalBlob.

Native providers include successful authenticated SyncFinished peers of the exact
open subscription, in addition to explicit native bootstrap endpoints. Jobs sample
this live bounded roster; NeighborDown/errors/Close/subscription replacement invalidate
old identities and generations. This follows pinned iroh-docs LiveEvent::SyncFinished
and iroh 1.3 Endpoint::connect's retained recent remote routes, without arbitrary UI
addresses. Existing native journeys author Owner-open-empty/Editor-join discovery,
reverse byte delivery, retained namespace reopen and stale-roster rejection. The role
fixture remains controlled; production DB authority is covered separately. No new
execution, peer-history, model/schema convergence or tagged-release claim is added.

### relay-routing-contract - supplied URL filtering and native routing (DQA022)

DQA022 selects the existing shared-native-endpoint routing contract for this scene
asset slice. ProviderRef parsing is bounded EndpointAddr JSON; sanitize_provider_addr
filters explicitly supplied relay URLs against with_allowed_relays. This is an input
URL filter. It does not establish an endpoint-wide network-egress allowlist: iroh 1.3
merges supplied addresses with retained paths and address lookup, probes known paths,
and can reuse another active relay route. Filtering a remote_info snapshot or checking
only after connection does not enforce a stronger boundary.

Scene discovery accepts authenticated successful synchronization peers associated with
the exact live opened Verse subscription, under bounded roster/generation ownership.
The blob connection pins their EndpointId. Serving still requires fresh actual stored
Viewer reach for the authenticated requester and actual local Editor plus legitimate
local upload/adoption proof; fetching repeats actual local Viewer and exact stored
metadata/reference admission. Byte-size/hash/MIME guards, queue/time/IO bounds and
Close/reset cancellation remain independent of native route choice. This contract
adds no arbitrary UI provider URL route and does not treat a peer-supplied Boolean
or mere possession of bytes as authority.

The older tileset-security/test-comment statement that supplied filtering guaranteed
global SSRF immunity was broader than its implementation or pure sanitizer test.
It is corrected here as an unestablished residual claim, not reported as a repaired
endpoint-wide control. The existing named test checks the supplied address projection
only. An authenticated peer/another native protocol may have populated routes outside
that supplied filter, so callers and operators must not infer a global egress promise.

Strict endpoint-wide relay egress is an OPEN policy/dependency choice for separate
work. A complete implementation would need a supported SDK or reviewed lower-level
sender/relay-actor policy covering retained/discovered paths, probes, active fallback
and cached connections before any disallowed dial. DQA022 does not select a direct-only
endpoint (which would regress relay-dependent WAN access), a vendor fork, or silently
broaden an enforced allowlist. Those mechanisms need their own scoped implementation
and independent security review. No network execution or stronger policy acceptance
is established by this documentation correction.

Scene transport initialization gives RelayMap::urls an explicit Vec<iroh::RelayUrl>
result before passing it to with_allowed_relays. The pinned SDK's urls method is
generic over FromIterator, while the adapter accepts IntoIterator; leaving both
sides generic can send trait inference through Bevy's recursive reference-to-Res
blanket impl. Keep this collection concrete; increasing recursion_limit masks the
missing inference boundary and does not describe the intended container.

## §join-readiness — simultaneous-dial recovery (QA-0101, 2026-11-07)

`successful_peers` (join readiness + the scene provider roster) is fed by two
signals in `replicator.rs::pump_live_events`: `SyncFinished(Ok)`, and
`InsertRemote.from` (content delivered by a peer proves a live session).
When two peers dial each other at once, iroh-docs 0.101 fails the Connect leg
with `Failed to close connection1` and then DROPS the Accept leg's successful
`SyncFinished` ("sync state finish called but not in running state"), so the
event stream alone can leave a synced peer unconfirmed forever (the viewer's
false "Join timed out" and "No native provider" in pass 77). A close failure is
therefore not evidence against the peer (it is not removed). `schedule_resync`
is a fallback only: one jittered re-dial per peer per 60 s, skipped if the peer
was confirmed during the delay. Never re-dial unthrottled: `Doc::start_sync`
re-joins gossip, the remote dials back on NeighborUp and recreates the race —
an earlier per-failure retry produced ~14 failed syncs/s per peer.

## §peer-book — relay-less restart addresses (QA-0086, 2026-11-07)

`peer_book.rs` keeps `<p2p_dir>/peer-book/<blake3(verse)>.json` (LRU since QA-0117, see §peer-book-lru): up to 16
`EndpointAddr`s of peers this node synced with. While a replica is open a task
refreshes it every 10 s from the roster via `Endpoint::remote_info`; on
`OpenVerseReplica`/`JoinVerseReplica` the book is merged into the bootstrap set
(explicit/env entries first). With `FE_SYNC_RELAY=disabled` there is no address
lookup, so this is what lets a restarted peer re-dial a peer that kept its port.
Since QA-0145 every node re-binds its recorded ports (§stable-port), so both sides restarting
at once also re-dial from their books; `FE_SYNC_BOOTSTRAP` is needed only when a recorded port was taken.

## §tileset-readvertise (QA-0101, 2026-11-07)

Tileset ads are ephemeral gossip. `pump_gossip_topic` forwards `NeighborUp` as an
empty `GossipIncoming` frame and the select loop re-advertises this node's
catalog on that verse (at most once per verse per 5 s), so a peer that joins
after the last Refresh/seed toggle still learns the provider for petal terrain.

## §peer-book-lru (QA-0117, 2026-11-07)

The peer book (`peer_book.rs`) is an LRU of `{addr, last_seen}` entries (the pre-QA-0117
bare address list still loads, counted as seen now). Entries older than 7 days drop on load;
a full book evicts its least recently synced entry that is not in the current synced set.
`merge` adds remembered peers only up to 16 in total, so explicit bootstrap/invite peers
always win and `scene_assets::opened_with_roster` never sees more than its 16-provider cap.
The recorder reads `SceneProviderRoster::peers()`, which holds only peers with a successful
`SyncFinished` (`confirm_peer`); forwarded `InsertRemote` authors never enter it.

QA-0136 (critic, 2026-11-07): `peers()` used to fail once more than 16 peers had synced,
which stopped the recorder and failed every scene fetch in a verse with 17+ collaborators.
It now returns every confirmed peer (bounded at 256), and `scene_assets::provider_window`
asks the explicit providers first, then a window of roster peers rotated per call, 16 in
all. Prefetch retries and later petal-entry fetches sample different windows, so every
collaborator is eventually asked.

## §scene-asset-prefetch (QA-0118, 2026-11-07)

A peer's scene-asset metadata row (`asset` table, `_fe_scene_asset` marker) starts an
unowned fetch of its blob when it arrives (`scene_asset_prefetch_hash` in the inbound
branch) and, for rows already in the replica, right after the open
(`ReplicaOpenReport::scene_asset_hashes` from `seed_reconciliation`). A failed prefetch
retries after 1, 2, 4, 8, 16 s without holding an IO slot while it waits, one log line per
attempt. Held bytes are skipped. A petal no longer has to be opened for its bytes to arrive.

QA-0133 (critic, 2026-11-07): only an ADMITTED row prefetches. `handle_inbound_row_change`
returns an `InboundVerdict`; the inbound branch and `seed_reconciliation` prefetch only a
`Forwarded` row whose author this node's role view holds at Editor+ at the row's petal (the
verse scope while the petal is unmapped), judged in `seed_reconciliation` after the whole
snapshot replayed. At most `PREFETCH_CAP` (8) prefetches run at once, so unowned work never
takes the `MAX_JOBS` owned-fetch budget; the rest wait in a bounded backlog (256) that
`pump_prefetches` drains from the loop's kept-work tick, and a verse close drops its share.

Reopened QA-0118 (fix:2, 2026-11-07): a failed fetch is never terminal while its verse stays
open. Every in-job failure (prefetch after its 1-16 s retries, or an owned fetch) records the
blob in `SceneAssetJobs::unavailable` (cap 1024), which `pump_prefetches` re-arms after 15,
30, 60, 60 ... s; `rearm(verse)` makes them due now on a gossip NeighborUp and after a
role re-read (a provider may now admit this node). A success, held bytes or the verse close
clears the entry. Rows that arrived before their author's grant skipped the arrival
prefetch; `drain_reread_queues` reads each empowered author's `asset/` entries and prefetches
them. Run 1's refusals were the provider's Serve admission demanding upload proof (QA-0137,
fe-database `handlers/AGENTS.md` scene-asset-transfer) and, under load, Serve admissions
timing out behind the saturated DB thread (QA-0138); the refusal log now names the gate
(`gate="tileset"|"scene-asset"|"child-range"`).

## §publication-lease (QA-0120, 2026-11-07)

A write for a verse whose replica is closed (a revoke made from the verse list) used to wait
in `PendingWrites` until the next visit. The sync thread now remembers each verse's open
parameters (from every host `OpenVerseReplica` and the host's `RememberVerseReplica`) and
opens a publication lease: it queues its own `OpenVerseReplica` (FIFO behind host commands),
which flushes the retained writes, and closes the replica inline 120 s after the last write.
A host open during a lease takes ownership. `PublicationLeases` is the pure state machine.

## §rejudge-on-demotion (QA-0121, 2026-11-07)

`VerseFabric` queues every DID whose role fell. The loop reads that DID's `node/` keys,
then every author's entry for those keys from the snapshot, and sends ONE
`DbCommand::RejudgeReplicatedNodes` batch per verse (`send_rejudge_batch`; fe-database
`src/AGENTS.md` §write-time-role).

Critic fixes, 2026-11-07:

- **No lost demotion (QA-0132).** The queues are peeked (`pending_rejudges`/
  `pending_rereads`) and a DID is acknowledged (`ack_*`) only after its work was accepted.
  A closed replica keeps the DID until the reopen; a failed replica read or a full DB
  channel keeps it for the loop's kept-work tick (`KEPT_WORK_RETRY`, 1 s, armed only while
  such work exists). A `role` row a full DB channel refused is kept on the fabric
  (`keep_unsent_role_row`, cap 4096) and resent in order BEFORE any re-judge batch, so the
  DB plane never judges with a stale role view.
- **No grant-cycling bypass (QA-0131).** The empowerment re-read replays only
  `role/__shards/__holders/verse/petal`; a risen author's scene rows go to the DB as a
  `restore_only` re-judge, which admits a version only if its author held Editor when it was
  written. Replaying `node/` through the current role would admit rows written while the
  author was revoked.

## §stable-port — a restarted node keeps its address (QA-0145, 2026-11-07)

`SyncEndpoint::new_with_stable_ports` (the sync thread's only bind) re-binds the UDP ports recorded in
`<p2p_dir>/endpoint-ports.json` (`{"v4":…,"v6":…}`) and records whatever it bound. A taken port falls
back to fresh ports with a warn; tests and tools keep `SyncEndpoint::new` (fresh ports).
Why: iroh 1.3 keeps a peer's selected path per endpoint id and sends a new connection's handshake ONLY
to that path (`remote_state.rs::handle_msg_send_datagram`). When a peer restarts on a new port while
the old connection is still open (30 s idle timeout), the dead path stays selected: closing a
connection never re-selects while another one exists. Run 3's owner could not dial the restarted
editor for minutes ("Failed to connect Timeout during connect" at iroh-blobs' 1 s pool timeout),
so every content download from it failed. With the same port the cached path stays live.

## §content-refetch — a failed docs download is not terminal (QA-0145, 2026-11-07)

iroh-docs downloads an entry's content once (from the forwarder, or later after a neighbor's
`ContentReady`) and never retries. The pump therefore keeps a re-fetch schedule per pending hash
(`replicator.rs` `PendingRows`/`ContentRetry`): after 2 s, then 4, 8, 16, 30, 30 … s while the
replica is open, it fetches the blob itself through `IrohBlobTransport` (15 s dial, BAO-verified,
16 MiB cap) from the forwarder and the entry author, at most 4 at once and 4096 hashes per replica.
A success emits the waiting rows exactly like `ContentReady`. Invariant: per (author, table, record)
slot only the newest entry may still land: a newer `InsertRemote` drops the slot's older waiting
entry, so a late fetch can never apply stale content over a newer row.

**Bounded by author, never refuse-on-full (QA-0150, 2026-11-07).** An Editor may write rows
naming content no one can serve. The table therefore holds at most 512 re-fetches per entry
author (`MAX_CONTENT_RETRIES_PER_AUTHOR`; an author at its cap evicts among its own) inside the
4096 total; a full table evicts its most-failed, then oldest, idle entry for the new hash instead
of refusing it (the pre-fix refusal meant that, once one author had filled it, a legitimate asset
that failed once was never retried). Each hash gets 10 attempts (about 3.5 min); then its schedule
is dropped and re-armed only when its row is written again (`InsertRemote` → `wait`). Dropped
schedules never drop the waiting rows: `ContentReady` still lands them. Evictions are logged at the
1st, 2nd, 4th, 8th ... occurrence so a flood cannot flood the log.

## §isolated-write-sync — writes made with no gossip neighbor (QA-0145, 2026-11-07)

A local write's gossip broadcast reaches only live neighbors. After the editor restart in the
QA-0145 repro, the owner's hyparview held a stale pending `Neighbor` request for the editor and never
answered its new `Join`, so the editor had no neighbor on the verse topic and every write reached
no one (set reconciliation runs only on open/NeighborUp). The pump tracks `NeighborUp/Down`; an
`InsertLocal` while the set is empty schedules one `Doc::start_sync` with every peer this replica
has synced or exchanged entries with (`IsolatedWrites`): 1 s after the first write (a burst
coalesces), at most every 5 s, skipped if a neighbor came back. It never runs while a neighbor
is live, so it adds no traffic to a healthy swarm and cannot recreate the QA-0101 re-dial storm.
