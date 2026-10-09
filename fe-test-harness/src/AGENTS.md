# fe-test-harness/src — harness rationale

Package `fractalengine-test-harness` has two targets: the P2P scenario runner
binary (`main.rs`, §scenarios below) and a library (`lib.rs`) exposing the
API-integration harness (§api-harness) and the in-process peer model
(§peer-model).

## §peer-model (F8/M3 generalization)

`peer.rs` — the in-process P2P peer — is a **library** module (`pub mod peer`)
since F8: the sim lab (fe-sim) builds its `SimPeer` on the same machinery
instead of the harness growing a parallel peer model. `TestPeer::spawn`
keeps the real-transport shape; `TestPeer::spawn_with_transport(name, dir,
Some(factory))` passes a `fe_sync::VirtualTransportFactory` through to
`spawn_sync_thread_with_transport`, so a simulated peer's sync thread binds
no iroh endpoint and sources replicas from the virtual transport — the same
DB loop, blob store, channels, and wait helpers either way. The bin
re-exports the lib module (`pub use fractalengine_test_harness::peer;` in
`main.rs`) so the scenario files' `crate::peer::TestPeer` imports keep
resolving. `TestPeer::spawn_with_identity(.., keypair)` (F9) takes a
caller-supplied `NodeKeypair`: the sim lab seeds identities so DID-ordered
shard placement repeats run to run (fe-sim `src/AGENTS.md` §identity).

## §api-harness

`api.rs` — reusable in-process fe-api integration harness, consumed from
`fe-api/tests/` as a dev-dependency (`fractalengine-test-harness = { path =
"../fe-test-harness" }`; the dev-dep cycle with fe-api's normal dep is legal
cargo). Import path for test authors:

```rust
use fractalengine_test_harness::api::ApiHarness;
```

**What `ApiHarness::spawn()` builds** (no network, no Bevy, no DB thread):

- In-memory SurrealDB (`engine::local::Mem`, ns/db `test`/`test`) with the
  full schema (`fe_database::schema::apply_all` + api-token schema).
- The **real router** via `fe_api::server::build_router` over a real
  `ApiState`: `db_reader = Some(db)` (so every direct-read handler works),
  a tempdir-backed `FsBlobStore`, and a live `api_cmd_tx` whose receiver is
  held open but **never serviced** — handlers that require the
  crossbeam→DB-thread round-trip (most writes, `get_hierarchy`, …) will hang
  or 5xx; test against `db_reader`-backed handlers or seed directly.
- Requests go through `tower::ServiceExt::oneshot` (`h.request(req)`), with
  `get`/`post_json` conveniences returning `(StatusCode, lenient JSON)`.
  Raw bodies: `api::body_bytes(resp)`.

**Auth approach**: the harness generates its own `NodeKeypair`, installs its
verifying key in `ApiState`, and `mint_token(scope, role)` mints a real signed
JWT via `fe_identity::api_token::mint_api_token` (1h TTL, fresh jti) — so the
production `auth_middleware` runs unmodified: 401/403 paths are the real ones.
Scopes use the repo grammar (`VERSE#v`, `VERSE#v-FRACTAL#f-PETAL#p`);
`SeededHierarchy::verse_scope()` gives the covering verse scope.

**Seeding**: `seed_verse`/`seed_fractal`/`seed_petal`/`seed_node` (+
`seed_hierarchy` for the full chain) run the same `CREATE ... CONTENT`
statements the fe-database handlers write — including the mandatory
`<geometry<point>>` cast (fe-database/src/AGENTS.md §geometry-inserts) and
omit-when-absent optional fields. The real DbCommand dispatch loop is
quarantined inside fe-database's DB thread (SurrealKV-only, not callable
against Mem), so statement-parity is the strongest "real write path"
available in-process; `h.db` is the same handle as `state.db_reader` for
bespoke seeding/assertions. Smoke test proving the wiring end-to-end:
`fe-api/tests/api_harness_smoke.rs`. Run with
`RUST_MIN_STACK=134217728 cargo test -p fe-api --test <file>` (surrealdb-core
stack gotcha, see project memory).

Standalone binary (package `fractalengine-test-harness`) that functional-tests
the P2P Mycelium stack headlessly: each scenario spawns isolated `TestPeer`s
(own in-memory DB, blob store, sync thread, and identity — no Bevy/GPU) and
drives them via `DbCommand`/`SyncCommand` messages. `main.rs` runs every
scenario in sequence and exits non-zero on any failure; `peer.rs` owns peer
spawning and `wait_for` result matching; `fixtures/` provides minimal asset
bytes (`create_minimal_glb`).

## §scenarios

| Scenario (file) | Purpose | Gotchas |
| --- | --- | --- |
| 1 Blob Store Roundtrip (`blob_roundtrip.rs`) | `ImportGltf` writes the bytes to the blob store, returns a `blob://` asset_path, and the BLAKE3 hash in the path matches the original file bytes (store holds the exact bytes). | — |
| 2 Legacy Base64 Migration (`migration.rs`) | Legacy base64 asset → decode → blob-store write → hash/`blob://` URL construction. | Tests the migration *pattern*, not `migrate_base64_assets_to_blob_store` — that function is private to fe-database, so the scenario verifies the public blob-store API produces the same BLAKE3 hash the migration would. |
| 3 Invite Flow (`invite_flow.rs`) | Alice creates a verse and an invite string; Bob joins and his hierarchy contains the verse with the correct name. | — |
| 4 Verse Sync Infrastructure (`verse_sync.rs`) | `SyncCommand::OpenVerseReplica` + `WriteRowEntry` are accepted without errors. | Infrastructure stub: actual two-peer P2P sync requires `IrohDocsReplicator` fully wired (Phase F+); this only proves the command pipeline and sync thread process commands without panicking. |
| 5 Two-Peer Blob Exchange (`two_peer_blob_exchange.rs`) | A blob written by Alice lands in Bob's store with the same hash, identical bytes, and the same `blob://` URL — content-addressability and portability across peers. | The "network fetch" is a manual byte copy between the two stores; no transport is exercised. |
| 6 Two-Peer Verse Join (`two_peer_verse_join.rs`) | Full lifecycle: Alice's verse/fractal/petal, invite with `include_write_cap=true`, Bob joins and creates his own fractal in the joined verse. | Proves invite-based collaboration over *independent* DB state — nothing is replicated between peers. |
| 7 Two-Peer Sync Pipeline (`two_peer_sync_pipeline.rs`) | Full sync command pipeline for two peers sharing a verse: replica open, row-entry write, and close on both sides. | Bob opens his replica with the namespace_id from Alice's invite; the blob crosses via manual store-to-store copy (simulated fetch), as in scenario 5. |
| 8 Two-Peer Real Replica Sync (`two_peer_replica_sync.rs`) | **A2**: two in-process peers with real iroh endpoints over loopback — Alice `WriteRowEntry` → `Doc::set_bytes` → real transport → Bob's inbound pump → `ApplyReplicatedRow` → **READ-BACK from Bob's durable store** (RawQuery), plus the `RowApplied` event and `Applied` outcome echo. | Requires BOTH peers online: each `TestPeer` now gets its own `p2p` data dir (redb exclusive lock — the pre-fix state degraded every peer but the first to the offline mock) and its `sync_node_addr` (from `SyncEvent::Started`) is what bob dials via `OpenVerseReplica.bootstrap_peers`. Verse secrets come from the shared `ns_secrets` map (`TestPeer::namespace_secret`) instead of the former thread-local. The verse manifest lands on bob through the A3 **bootstrap window** (bob's store has no verse row yet — replica capability is the admission). |
| 9 Two-Peer Tombstone Sync (`two_peer_tombstone_sync.rs`) | **A5** (N-4 through the real transport): live node row → bob applies (READ-BACK live) → empty-entry tombstone (`add_blob(b"")` → `Doc::del`) → bob converges (READ-BACK tombstoned + `AppliedTombstone`) → alice re-publishes the stale live row → bob refuses the resurrection (`SkippedTombstoned`, READ-BACK still tombstoned). Each leg waits for bob's `ReplicatedRowApplied` outcome before the next write, so ordering never depends on transport semantics. | The author DID (iroh NodeId → did:key) equals the verse `created_by` DID (same ed25519 seed backs both), so alice resolves to Owner on bob once the manifest converged — the node rows cross the A3 gate; a non-admitted author would see `Denied` instead. |
| 10 Two-Peer Timeseries Union Sync (`two_peer_timeseries_sync.rs`) | **A12** (union CRDT through the real transport), F5: an `iot_reading` row published the way `insert_readings_with_replication` emits it (blob-store the row bytes → `WriteRowEntry` keyed by `reading_id`) crosses to bob and applies; then (a) the identical row re-delivered stays one row, (b) a conflicting payload for the same `reading_id` never overwrites (`Applied`, value unchanged), (c) an empty-entry tombstone on the timeseries key is `NotApplicable` and the fact survives, (d) an *earlier*-timestamped second reading delivered later still joins the set. Every leg is READ-BACK from bob's durable store; the same manifest-converges-first setup as scenario 9 puts alice at Owner so the rows cross the A3 gate. | Publish through `publish_row` (blob store + `WriteRowEntry`) rather than a DB command: this scenario drives the exact post-handler emit shape. ~~there is no `DbCommand` that ingests readings~~ **STALE, corrected F23 (2026-10-08):** `DbCommand::InsertIotReadings` EXISTS since F7 (handled in `fe-database/src/lib.rs`'s dispatch loop; both reply-family halves mapped in `fe-runtime/src/app.rs`), so a peer *can* ingest through the DB-thread seam. This scenario still prefers `publish_row` to exercise the replication emit shape directly; use `InsertIotReadings` when specifically testing the DB handler seam (the sim lab F8 routes fleet ingestion through `insert_readings_with_replication`). `value` is a reserved SurrealQL word — read readings back with `SELECT *` and index `value` in the JSON. |
| 11 Two-Peer Shard Fabric (`two_peer_shard_fabric.rs`) | **A13/A14** (F6 shard fabric through the real transport): (1) a `balanced`/R=2 verse manifest plans BOTH peers for the first shard and the `__shards` ledger row + reading converge on bob (READ-BACK); (2) re-publishing the manifest with `ts_mode=mirror` switches both fabrics (polled via `GetShardLedger`) and the next shard records `mirror` with all-peer hosts; (3) `sharded` + bob declaring a 1-byte capacity excludes him from the next shard's plan (1 host) and a LATER reading for that shard never lands in bob's store (receive-side retention); (4) bob capped at ~2 shards' worth of bytes still sees all 8 one-host shards placed — the fleet total is bounded by the SUM of capacities, never the MINIMUM; (5) both peers at ~zero capacity still place a brand-new shard (least-utilized last resort — a shard is never left homeless). | The fabric is sync-plane state: `__shards`/`__peers` rows have NO `DbResult` echo (the seam consumes them before the DB), so ledger assertions poll `GetShardLedger` → `SyncEvent::ShardLedger` via `TestPeer::wait_sync_event`. A skipped reading row is silent by design — leg 3 sleeps a grace window (`ROUTE_GRACE`) after the ledger demonstrably converged, then asserts absence. Each reading is one per minute (60s bucket width) so every reading lands in its own bucket → its own shard. |
| 12 Distributed Query Fan-out (`distributed_query.rs`) | **A15/A16** (F7 fan-out transport through the real gossip transport, 3 peers): leg 1 three-host `WindowAggregate` merged == whole-petal ground truth (exact per-anchor counts/sums/min/max — the mean monoid); leg 2 `AllReadings` raw union dedupes by `reading_id` against ground truth; leg 3 `LatestPerAnchor` keeps the max timestamp; leg 4 every planned shard `covered`; leg 5a a creator-held copy of a carol-planned shard is still `covered` (row attribution) while leg 5b a genuinely carol-exclusive shard is `missing` after her replica closes; leg 6 at R=2 the surviving mirror serves the row (row-in-result). | Deterministic placement via two-sided probes: `ingest_until` polls the author's local ledger for the settled entry, `ingest_exclusively_until` waits for survivor convergence + the exclusive host's durable-store ABSENCE (retrying buckets on over-retention) — placement luck is never relied on. The HOST's open is sequenced 500ms before the joiners' opens (gossip Join-drop race — see fe-sync/src/AGENTS.md §gossip-bootstrap), and bob/carol's opens dial alice via `bootstrap_peers` with `add_node_addr` registration. Run relay-disabled (`FE_SYNC_RELAY=disabled`, set by harness `main.rs` — loopback-only by design); `FE_HARNESS_SCENARIO=<substring>` filters to one scenario for flake-hunting loops. A17 surface guards are covered separately by `fe-api/tests/distributed_query_test.rs` (fake sync seam, no transport). |
| API Token Lifecycle (`api_token_flow.rs`) | Mint → list → JWT claim verification → revoke → re-list. | One file, two entry points registered as separate scenarios in `main.rs`: `run` (lifecycle) and `run_edge_cases` (empty scope, excessive TTL, double revoke, wrong JTI). |
