# fe-test-harness/src — harness rationale

API-token mint requests and replies retain optional correlation IDs in the
peer model. Uncorrelated scenario callers use None; correlated failures echo
RequestFailed rather than a wildcard Error. The authorization contract is
unchanged. Correlation preparation is source-only until the integrated sweep.

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

**Bounded-channel discipline (DEC-C13, 2026-10-09).** Every peer channel is
bounded(64) EXCEPT the DB command channel, which takes production's
`fe_runtime::channels::CHANNEL_BUFFER` (256; R5 sweep 2026-10-25): the sync
thread forwards inbound rows into it with drop-on-full, and since RP-4's
inbound gate made each apply slower, a 64 buffer dropped ~20% of a 320-row
mirror burst that production's 256 keeps (fe-sim's deadlock guard then never
converged). The DB/sync threads' result/event sends to the driver are
mostly BLOCKING — correct for awaited replies (the driver is in `wait_for`),
deadlock-prone for unsolicited output nobody waits for. Two rules:

- The DB loop's `ApplyReplicatedRow` echo (`ReplicatedRowApplied`) is
  unsolicited (the inbound pump triggers it, not the driver), so it is a
  `try_send` + drop-and-count (§replication-backpressure). A blocking send
  here parked a replica's DB thread once ~64 echoes queued; the driver's
  next `send` then blocked on the full command channel — a cycle the 30s
  settle deadline never broke (the M3 sim-lab deadlock at ≳129 retained
  rows between store barriers). Real-transport scenarios that `wait_for` an
  echo are unaffected: they move a handful of rows, far below 64.
  Awaited replies keep blocking sends (dropping one would turn a test into a
  30s timeout). `TestPeer::send` stays a blocking send — a timeout there
  would only convert the cycle into a panic, not break it.
- The sync thread's `ComputeResultReady` echo (one per distributed query)
  is unsolicited too and is likewise a `try_send` in fe-sync (fe-sync
  `src/AGENTS.md` §distributed-query "diagnostics echo"): a driver that
  loops `SubmitComputeTask` without draining sync events (the
  `distributed_query` leg-1 retry loop) filled the bounded(64) event
  channel and, with a blocking send, froze the peer's whole sync runtime.
- `shutdown_inner` sends Shutdown with `try_send` and joins while draining
  BOTH receivers (`send_draining`/`join_draining`): the peer owns them, so
  a thread parked on a full result/event send would otherwise never see
  Shutdown and `join` would hang (reachable from fe-sim's `ScenarioSession`
  Drop, which tears down mid-run).

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
  held open but **not serviced by default** — handlers that require the
  crossbeam→DB-thread round-trip (most writes, `get_hierarchy`, …) will hang
  or 5xx; test against `db_reader`-backed handlers or seed directly. A test
  that needs the round-trip takes `h.api_command_receiver()` and services it
  on its own thread, filing requests in `PendingApiRequests` and answering
  with the production fe-database reply builder against `h.db` (e.g.
  `fe-api/tests/terrain_mutation_test.rs`).
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
| 9 Two-Peer Tombstone Sync (`two_peer_tombstone_sync.rs`) | **A5** (N-4 through the real transport): live node row → bob applies (READ-BACK live) → empty-entry tombstone (`add_blob(b"")` → `Doc::del`) → bob converges (READ-BACK tombstoned + `AppliedTombstone`) → alice re-publishes the stale live row → bob refuses the resurrection (`SkippedTombstoned`, READ-BACK still tombstoned). Each leg waits for bob's `ReplicatedRowApplied` outcome before the next write, so ordering never depends on transport semantics. | The author DID (iroh EndpointId → did:key) equals the verse `created_by` DID (same ed25519 seed backs both), so alice resolves to Owner on bob once the manifest converged — the node rows cross the A3 gate; a non-admitted author would see `Denied` instead. |
| 10 Two-Peer Timeseries Union Sync (`two_peer_timeseries_sync.rs`) | **A12** (union CRDT through the real transport), F5: an `iot_reading` row published the way `insert_readings_with_replication` emits it (blob-store the row bytes → `WriteRowEntry` keyed by `reading_id`) crosses to bob and applies; then (a) the identical row re-delivered stays one row, (b) a conflicting payload for the same `reading_id` never overwrites (`Applied`, value unchanged), (c) an empty-entry tombstone on the timeseries key is `NotApplicable` and the fact survives, (d) an *earlier*-timestamped second reading delivered later still joins the set. Every leg is READ-BACK from bob's durable store; the same manifest-converges-first setup as scenario 9 puts alice at Owner so the rows cross the A3 gate. | Publish through `publish_row` (blob store + `WriteRowEntry`) rather than a DB command: this scenario drives the exact post-handler emit shape. ~~there is no `DbCommand` that ingests readings~~ **STALE, corrected F23 (2026-10-08):** `DbCommand::InsertIotReadings` EXISTS since F7 (handled in `fe-database/src/lib.rs`'s dispatch loop; both reply-family halves mapped in `fe-runtime/src/app.rs`), so a peer *can* ingest through the DB-thread seam. This scenario still prefers `publish_row` to exercise the replication emit shape directly; use `InsertIotReadings` when specifically testing the DB handler seam (the sim lab F8 routes fleet ingestion through `insert_readings_with_replication`). `value` is a reserved SurrealQL word — read readings back with `SELECT *` and index `value` in the JSON. |
| 11 Two-Peer Shard Fabric (`two_peer_shard_fabric.rs`) | **A13/A14** (F6 shard fabric through the real transport): (1) a `balanced`/R=2 verse manifest plans BOTH peers for the first shard and the `__shards` ledger row + reading converge on bob (READ-BACK); (2) re-publishing the manifest with `ts_mode=mirror` switches both fabrics (polled via `GetShardLedger`) and the next shard records `mirror` with all-peer hosts; (3) `sharded` + bob declaring a 1-byte capacity excludes him from the next shard's plan (1 host) and a LATER reading for that shard never lands in bob's store (receive-side retention); (4) bob capped at ~2 shards' worth of bytes still sees all 8 one-host shards placed — the fleet total is bounded by the SUM of capacities, never the MINIMUM; (5) both peers at ~zero capacity still place a brand-new shard (least-utilized last resort — a shard is never left homeless). | The fabric is sync-plane state: `__shards`/`__peers` rows have NO `DbResult` echo (the seam consumes them before the DB), so ledger assertions poll `GetShardLedger` → `SyncEvent::ShardLedger` via `TestPeer::wait_sync_event`. A skipped reading row is silent by design — leg 3 sleeps a grace window (`ROUTE_GRACE`) after the ledger demonstrably converged, then asserts absence. Each reading is one per minute (60s bucket width) so every reading lands in its own bucket → its own shard. |
| 12 Distributed Query Fan-out (`distributed_query.rs`) | **A15/A16** (F7 fan-out transport through the real gossip transport, 3 peers): leg 0 (RP-8) every shard's declared host genuinely holds its row locally, polled against the ingesting peers' ground truth, before the fan-out ever asks; leg 1 three-host `WindowAggregate` merged == whole-petal ground truth (exact per-anchor counts/sums/min/max — the mean monoid), now also the host-first delivery order (RP-6/RP-7 settlement + responder filtering); leg 2 `AllReadings` raw union dedupes by `reading_id` against ground truth; leg 3 `LatestPerAnchor` keeps the max timestamp; leg 4 every planned shard `covered`; leg 5a a creator-held copy of a carol-planned shard is still `covered` through alice's RP-6 holder (`__holders`) declaration, not host-or-role, while leg 5b a genuinely carol-exclusive shard is `missing` after her replica closes; leg 6 at R=2 the surviving mirror serves the row (row-in-result). | **RP-8: roles ARE now provisioned** — alice (the owner) grants bob and carol Editor through the real `AssignRole` handler right after the scaffold converges, before any reading is ingested. This stopped being optional once RP-6/RP-7 landed: responder filtering is no longer "answer every requested shard" (the pre-RP-7 permissive default this file used to describe), so a reading that never admits on a NON-authoring host (the A3 role gate defers it `AuthorRole`) leaves that host with nothing to answer from — there is no creator-copy fallback for a plain non-holder, non-host answerer anymore. Do not revert the grants to "test the no-role default": that default is covered in isolation by `role_propagation.rs`'s acceptance leg, not by hiding it here. Deterministic placement via two-sided probes: `ingest_until` polls the author's local ledger for the settled entry, `ingest_exclusively_until` waits for survivor convergence + the exclusive host's durable-store ABSENCE (retrying buckets on over-retention) — placement luck is never relied on. The HOST's open is sequenced 500ms before the joiners' opens (gossip Join-drop race — see fe-sync/src/AGENTS.md §gossip-bootstrap), and bob/carol's opens dial alice via `bootstrap_peers` with address-book registration (`SyncEndpoint::add_peer_addr` — iroh 1.x `MemoryLookup`, successor of 0.35's `add_node_addr`). Run relay-disabled (`FE_SYNC_RELAY=disabled`, set by harness `main.rs` — loopback-only by design); `FE_HARNESS_SCENARIO=<substring>` filters to one scenario for flake-hunting loops. A17 surface guards are covered separately by `fe-api/tests/distributed_query_test.rs` (fake sync seam, no transport). |
| API Token Lifecycle (`api_token_flow.rs`) | Mint → list → JWT claim verification → revoke → re-list. | One file, two entry points registered as separate scenarios in `main.rs`: `run` (lifecycle) and `run_edge_cases` (empty scope, excessive TTL, double revoke, wrong JTI). |
| 16 Two-Peer Tileset Stream (`two_peer_tileset_stream.rs`) | **PAS-P3P4 S4** (campaign plan `p2p_asset_streaming_20260718`): real network proof of the whole P2P tileset path — alice bundles a synthetic tileset (`fe_terrain::tiles::bundle_tileset`) and `PublishTileset`s it; bob, joined to the same verse's gossip topic (`bootstrap_peers`, same mechanism as scenario 11), receives the real gossip-borne `PeerTilesetAdvertisement`, sends `DownloadTileset`, and the sync thread (S3) drives meta resolution + every bundle fetch to `TilesetDownloadComplete` with no `RequestChunk` ping-pong from the harness; bob then assembles+installs into a fresh `HexonStore` and every elevation tile byte-equals alice's source. **Cross-restart leg:** both peers shut down, bob respawns on the SAME `p2p` dir (`TestPeer::spawn` derives it from `temp_dir/name`, reused verbatim) with alice OFFLINE — the startup task (`sync_thread.rs`'s `pending_downloads`/`resume_all`) emits `TilesetDownloadResumed { received: total }` from the persisted, re-verified-on-disk ledger alone, proving the install depends on local verified state, not a live provider. | Unlike scenarios 5/7 (which "simulate" the network leg via a manual store-to-store byte copy), this one rides the real iroh-blobs transport end to end — the first scenario to do so for tileset bytes. `bob2`'s `TestPeer` gets a FRESH random keypair on respawn (the harness always generates one); this is fine because the download ledger lives in the `p2p` dir and is keyed by tileset_id, not by peer identity. A resend of `DownloadTileset` after the restart is included (mirrors what a real UI reopening the Downloads tab would do) but is NOT what the assertion waits on — with alice down, `resolve_providers_and_root` fails fast on the empty in-memory provider map before ever touching the ledger, so it cannot race the automatic `resume_all` pass; the assertion waits on the automatic `TilesetDownloadResumed` event instead, which is the honest, deterministic signal. |
| 17 Two-Peer Device-Payload Sync (`two_peer_device_payload_sync.rs`) | **CDL-SPEC10 A6** (R5 generic asset / device-payload model through the real transport): alice declares a `device_format`, builds a fractal/petal/node scaffold and ingests a `device_payload` referencing 2 generic `asset` rows through the real `DbCommand::InsertDevicePayloads` door (durable-first, then it replicates the asset rows, then the payload row — CDL-SPEC10 A3); bob — already converged on the verse/fractal/petal/node/device_format scaffold (A3's R5 referential-confinement gate denies a `device_payload`/`asset` row whose petal/node/format is not already locally known, so the scaffold must land and be confirmed first) — applies all three rows (READ-BACK, ordered `asset_ids`); a blob miss on bob triggers `SyncCommand::FetchBlob` and, once the content is locally present, `SyncEvent::BlobReady` with byte-identical content; a re-delivered `device_payload` (same `payload_id`) stays one row. | None of `CreateFractal`/`CreatePetal`/`CreateNode` carry a replication sender in this harness, so the verse/fractal/petal/node rows are hand-published; the `device_format` row is NOT — it is declared through the real `RegisterDeviceFormat` door after the manifest converges, and `register_format_with_replication` publishes it (CDL-SPEC10 review-and-fix closed A6's "format never leaves the declaring peer" gap). The scaffold rows are hand-published via `publish_row` (the scenario 9/10 technique), using the real ids `CreateFractal`/`CreatePetal`/`CreateNode` returned on alice's side. `CreateAsset` is one of the handful of commands this harness's simplified DB loop does not simulate (`"API asset ingest not simulated by the test-harness peer"`), so the two `asset` rows are created via `RawQuery CREATE asset CONTENT {...}` directly, matching the real `create_asset_handler`'s shape. Real P2P blob **content** transfer is still a Phase-D stub (`fe-sync/src/sync_thread.rs::handle_fetch_blob`, "peer discovery not yet implemented") — like scenarios 5/7, the byte motion itself is a direct store-to-store copy; this scenario is the first to also exercise the real `FetchBlob`→`BlobReady` command/event pair once the content is locally present (a genuine miss emits no event at all under the current stub). |
| 18 Role Propagation (`role_propagation.rs`) | **RP-8**, the role-propagation track's harness end-to-end leg, real transport, 6 peers (alice the owner, bob, carol a FRESH peer never granted anything directly, dave, erin, frank): (1) alice→bob Editor; carol admits bob's node row AND reading with no local provisioning — SYNC-ROLE-PROPAGATION's acceptance; (2) alice→bob Manager, bob→dave Editor; carol admits dave's row through a grantor whose own authority traces to the owner; (3) R7 subtree: alice makes erin Manager of ONE petal, erin→frank Editor there; carol admits frank's row in that petal and Defers his row in the sibling petal erin has no authority over; (4) T4 cascade: alice revokes bob, and dave's next row (admitted only through bob's now-void authority) Defers on carol; (5) P2P-REF-CONFINE, two-peer: a node bob plants in verse A naming a petal that genuinely belongs to a second verse B (carol joined both) is Denied on carol, never merely deferred. | A Manager-issued grant (bob→dave, erin→frank) needs the GRANTOR's own authority to have converged on their OWN replica first — `assign_role_retrying` retries the `AssignRole` itself (not a settled-echo wait) until that local check passes, since it is a real network race, not a bug. The T4 cascade leg polls with fresh row ids (`publish_until_deferred`) for the same reason: dave's very next row might still beat the revoke's propagation to carol. Verse B exists solely so carol has a DURABLE, correctly-tagged foreign petal to reject against — `locate_petal` returns `Unknown` (Deferred, not Denied) for a petal nobody has ever seen, so the foreign-petal row must already be known-foreign, not merely unknown. |

## §scaffold (role propagation RP-4)

Since RP-4 the inbound gate admits nothing but the verse row before the manifest, and confines
every content row to parents the receiving peer already knows (fe-database `src/AGENTS.md`
§handlers "replicated_row"): a row naming an unknown petal, node, asset, payload or format is
`Deferred` until that parent lands, never applied unchecked. Scenarios that hand-publish
content rows therefore publish the scaffold first through `scenarios/scaffold.rs`
(`publish_manifest`, `publish_scaffold`, `publish_and_await` — each waits for every receiver's
settled `Applied` echo, skipping the interim `Deferred` one). Scenarios 9-12, 18 and
`api_db_sync_cross_thread` use it; scenario 17 builds its own scaffold. The harness invite
(`peer.rs::generate_invite`/`join_verse`) mirrors production: the invite pins the verse owner
(`owner_did` + `verse_created_at`, signed), and the joiner writes them as its local manifest.
A non-owner author's row that has not yet cleared the A3 role gate is `Deferred AuthorRole`
(RP-3), never `Denied` outright — it is retried the moment that author's effective role rises.
Scenario 12 no longer exercises the no-role default itself (RP-8: alice grants bob and carol
Editor before ingest, the production flow); that default, and the `AuthorRole` defer/retry path
it depends on, is scenario 18's (`role_propagation.rs`) job to cover in isolation.

**The author's own DB (R5 sweep 2026-10-25).** A published row is never echoed back to its
author's DB (sync-thread loop prevention), so a scaffold the author must itself judge against —
a peer's child row naming it, or an `AssignRole` at a fractal/petal scope (the grant precondition
requires the scope to exist on the granter) — is created locally first with
`scaffold::create_local_fractal` / `create_local_petal` (real `CreateFractal`/`CreatePetal`) and
published under the returned ids. Every peer's anchor that another host may judge a reading
against is published too (scenario 12 publishes bob's and carol's, not only alice's). A scenario
that never publishes a manifest leaves every fabric without an owner or `default_access`, and
since W5s/R1 a publisher's blob gate then refuses every requester (scenario 16 publishes one).

## GUI hierarchy details command

The in-process peer model explicitly ignores GetEntityDetails alongside its
existing unsupported GUI EntitySettings commands. Real details readback and
Viewer admission journeys use the production fe-database dispatcher/handler;
no fake peer read is counted as that evidence.

The simplified peer model explicitly refuses ReadNodeUrl with a typed unavailable reply. It
does not own the desktop placed-model URL reader and never fabricates a successful URL snapshot.
Production DB authorization/read tests belong to fe-database's existing real-store journey.

## Terminal reset peer refusal

The running TestPeer does not own the desktop service supervisor. It explicitly refuses the terminal-reset command with its dedicated correlated result and leaves its data unchanged. Real terminal worker exit/readback proof is in fe-database shared_handle_test, not the peer's legacy clear scaffold.

The simplified peer import adapter explicitly marks its complete GUI birth
materialization unavailable. It is not the production scoped initial-history or
full initial-TRS receipt implementation; real-store and UI App journeys cover
those contracts separately. Its existing API identity/correlation shape remains.
