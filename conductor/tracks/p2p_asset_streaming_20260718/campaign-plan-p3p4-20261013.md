---
type: plan
track: p2p_asset_streaming_20260718
created: 2026-10-13
title: "Campaign plan: P2P asset streaming Phases 3+4 (PAS-P3, PAS-P4, PAS-P1V) + petal.render_distance"
tags: [p2p_asset_streaming_20260718, campaign, plan, PAS-P3, PAS-P4, PAS-P1V]
resource: ./plan.md
---

# Campaign plan: PAS-P3 + PAS-P4 (+ PAS-P1V, petal.render_distance)

Planned at HEAD `877e801` (2026-10-13). Planner did not run cargo. Every code claim below is a
`file:line` read at that HEAD. Slices run **strictly sequentially**; each executor runs
`cargo check` + `cargo clippy --all-targets -- -D warnings` on its own crates only. **Tests are
written in every slice and run once, at campaign end** (RUNBOOK §3 working rules).

---

## 1. Goal and acceptance

**Goal (spec FR-3 + FR-4, decisions D-73 A4 / D-74 / D-77 E2):** a peer can fetch a tileset
from another peer as a HashSeq of verified, size-tuned bundles, resume it across restarts, and
render a petal whose scene residency is a per-frame, distance-ranked **pointer-set** instead of
"spawn everything up to a hard cap" — with tile bytes loaded lazily, shared once in RAM, and
evicted when they leave the ring.

Acceptance, verbatim from the ledger (`conductor/doneness-ledger.md:116-118`), plus the folded-in
decisions:

| Row | Acceptance | Proven by |
|---|---|---|
| PAS-P3 | Two-instance loopback chunked transfer with per-chunk blake3; tampered chunk rejected; `TilesetDownloadTracker` resumes after restart; in-memory transport impl used by tests | S2 tests, S4 harness scenario |
| PAS-P4 | Ledger resource replaces the boolean gate; nodes beyond `render_distance × 1.5` despawn and re-enter with hysteresis (moving-camera system test); lazy tile reads with an offset index; plan item 2.6 outcome recorded | S5, S7, S8 tests; §7 / S9 records 2.6 |
| PAS-P1V | fe-relay under Default / Custom / Disabled logs the relay mode, the default-relay warning, and `SyncStatus` health; output captured in the track | S4 script, run at campaign end |
| 2026-10-12 decision | "Per-petal render_distance: a `petal.render_distance` column (replicates); session override on top" — closes PAS-P4B's open durable-store question (`fe-ui/src/AGENTS.md:719-723`) | S6 |
| Plan 2.6 | Decide whether FR-4 subsumes runtime_instance_guardrails FR-6 | §7 (decided here), recorded in S9 |

Also delivered because the ratified design requires it (not new scope): D-74 B2 camera-forward
weighting with hysteresis, and D-74 B3 "phase 2" closed-loop GPU-byte horizon (DIAG-15M promoted
to a shared resource) — S8. The streamed (not-installed) tileset path — "stream in parts of
multiple hexons based on what's in the scene", the user's original ask — is slice S7.

## 2. Current-code reality (what exists, what is stub, what drifted)

### Transport / protocol (PAS-P3)

- **Drift that helps us:** the decision record says "no ALPN/protocol router is registered
  anywhere" (`conductor/decisions/p2p-streaming-20260718.md:108`). That is no longer true. Since
  the iroh 1.3 port, `DocsStack::spawn` builds an fs `iroh_blobs` store, a `BlobsProtocol`, and a
  `Router` accepting `iroh_blobs::ALPN` (`fe-sync/src/docs_engine.rs:117-120,159-163`). Inbound
  blob serving therefore already works. `DocsStack::blob_store()` (`:199`) and `endpoint()`
  (`:184`) are the handles the iroh transport impl needs.
- **D-77 drift:** E2 said "build on 0.35 behind a transport trait". The stack is now iroh 1.3 /
  iroh-blobs **0.103.1** (`fe-sync/src/AGENTS.md:50-60`). The trait is still worth having:
  iroh-blobs is pre-1.0 and will churn, tests need an in-memory impl, and PAS-P5 needs an HTTP
  impl behind the same seam. The first impl targets 0.103.1. The APIs it uses are in
  `~/.cargo/registry/src/index.crates.io-*/iroh-blobs-0.103.1/src/`:
  `api.rs:232 Store::blobs`, `:237 remote`, `:246 downloader(&Endpoint)`, `:227 tags`;
  `api/downloader.rs:404 Downloader::download(request, providers)`, `:575 Shuffled::new`;
  `api/blobs.rs:186 add_bytes`, `:381 get_bytes`, `:507 status`; `api/tags.rs:67 set`,
  `:92 list_prefix`; `hashseq.rs:11 HashSeq` (`FromIterator<Hash>`, `TryFrom<Bytes>`, `iter`);
  `hash.rs:365 HashAndFormat::raw`, `:373 hash_seq`; `api/remote.rs:496 Remote::fetch`
  (accounts for local partial state); `protocol.rs:809-846 GetRequest::builder().root(..)`.
  The fs store has **no GC by default** (`store/fs/options.rs:124 gc: None`), but we still tag
  everything we need to keep.
- **The stubs:** `handle_request_tileset_meta` replies with `{"error": "tileset not found
  locally"}` and `total_chunks: 0` (`fe-sync/src/sync_thread.rs:2097-2117`).
  `handle_request_chunk` always emits `ChunkFailed` (`:2119-2136`). Both are dispatched
  synchronously from the select loop (`:921-926`).
- **`TilesetDownloadTracker`** is an in-memory `HashMap<tileset_id, (peer, seq)>`
  (`sync_thread.rs:2010-2035`). `start()` and `get()` are `#[allow(dead_code)]`, never called
  (`:2022,2031`). It is created at `:673` and only `cancel()` is used (`:2140`). Nothing
  persists.
- **Advertising is dead end to end.** fe-ui sends `AdvertiseTilesets` with
  `advertisements_json: String::new()` (`fe-ui/src/actions/hexon.rs:39-42,118-121`, comment
  "refreshed by terrain layer", which nothing does). The handler skips empty payloads
  (`sync_thread.rs:2054-2058`). No inbound handler parses an advertisement
  (`fe-sync/src/AGENTS.md:117-124`; `distributed_query.rs:1082-1086` ignores non-compute
  frames). `SyncEvent::PeerTilesetAdvertisement` is never emitted.
- **Two `TilesetAdvertisement` types exist:** fe-sync's three-field one
  (`sync_thread.rs:2001-2007`) and fe-terrain's richer DTO (`fe-terrain/src/tiles/distribution.rs:29-47`).
- **The UI drives downloads by ping-pong, and only while the dialog is open.** On
  `TilesetMetaReceived` it asks for chunk 0. Each `ChunkReceived` asks for the next chunk, but
  only inside `if let ActiveDialog::HexonManager` (`fe-ui/src/terrain_map/events.rs:88-158`).
  **Closing the Hexon Manager stalls the download.** Status then stops at `Verifying` and nothing
  ever assembles or installs (`:131-133`). The sends use bare `.ok()` (`:116,154`;
  `actions/hexon.rs:43,82,122`), which breaks AGENTS anti-pattern #6.
- **Reusable pieces with no production callers:** `assemble_chunks` (`distribution.rs:161-305`),
  `PartialTileSource` (`:310-360`), and `TilesetDownload` (`:77-150`).
  `package_chunked` (`fe-terrain/src/tiles/builder.rs:289-397`) builds from the *network*
  (`download_tiles()`, `fetch` feature), not from an installed hexon. It writes
  `chunk_bounds: self.bounds`, which is the whole tileset rather than the chunk (`:374`). It
  stamps `chrono::Utc::now()`, so its output is non-deterministic (`:344-345`).
  `ChunkIndex` has no per-chunk hash (`fe-format/src/manifest.rs:133-143`).
- **Crate edges that shape the design:** fe-sync depends on neither fe-terrain nor fe-format, and
  fe-ui has no fe-terrain dependency (deliberate, root `AGENTS.md` §petal-map). fe-terrain
  reaches fe-runtime transitively through fe-renderer → fe-database. Only the binaries
  (`fractalengine`, `fractalengine-relay`) and `fe-test-harness` see both fe-sync and fe-terrain.
  So the host binary is the only place that can bundle or assemble, and fe-sync stays
  **format-agnostic**.

### Residency / tiles (PAS-P4)

- **Soft allowance exists, ledger does not.** `distance_ranked_allowance(distance, r, ceiling)`
  (`fe-ui/src/verse_manager/spawn.rs:34-56`) is only ever called with distance `0.0`, at
  petal-entry batch spawners: `petal_respawn.rs:97-109`, `primitive_materialize.rs:149-157`,
  `path_asset_materialize.rs:421-428`. The boolean `mesh_budget.exceeded` gate still wraps every
  call. No per-frame system ranks entities by camera distance. Nothing ever despawns a node for
  being far away (`fe-ui/src/AGENTS.md:751-761`).
- **Chain** (`fe-ui/src/verse_manager/mod.rs:170-183`): `apply_lifecycle_events →
  apply_db_results → respawn_on_petal_change → materialize_cached_primitives →
  reconcile_selected_primitive → reconcile_path_asset → materialize_path_assets`.
- **Watchdog** (`fe-ui/src/plugin.rs:46-145`): `MeshInstanceBudget { ceiling, exceeded, … }`
  is set in `Last` by `mesh_instance_watchdog`.
- **Tile residency is eager and doubled.** `HexonTileSource` holds every member as a decoded
  `HashMap<String, Vec<u8>>` (`fe-terrain/src/tiles/hexon_source.rs:22-33`), built by
  `from_archive` → `HexonArchive::import` (`:36-58`; `fe-format/src/archive.rs:256-387`). That
  import reads every entry. `TilesetRegistry.sources: RwLock<HashMap<String, HexonTileSource>>`
  (`registry.rs:51-54`) is filled by `load_all` (`:75-100`). Its only consumers are the fe-api
  tile routes (`fe-api/src/terrain.rs:504,542`, synchronous reads inside async handlers). The
  renderer loads a **second copy** (`fe-terrain/src/petal_binding.rs:153` →
  `store.rs:216-224` `std::fs::read` + full import) into
  `CompositeTileSource.hexon_sources: Vec<HexonTileSource>` (`composite.rs:35-36,91-93`).
  Evidence §5 quantifies this: 14.6 MiB today, ~3.8 GB at projected z12.
- **Main-thread reads:** `spawn_chunk` calls `get_tile_sync` / `get_satellite_tile_sync`
  (`terrain_plugin.rs:531-532`). A miss is recorded in `FailedTiles` until the next revision
  (`:487-513`). Under lazy loading, "not resident yet" must become **pending, not failed**.
- **Exit ring for terrain chunks already exists.** `update_terrain_lod` despawns chunks beyond
  `spawn_despawn_radii(.., DESPAWN_HYSTERESIS = 1.5)` (`terrain_plugin.rs:337-393`;
  `lod_ring.rs:12`). What is missing is **tile-byte** eviction and node/stamp eviction.
- **The async `fetch_tile` seam is never called** (`composite.rs:260-337`, `fetch` feature).
- **Member-granular zip reads already exist** in fe-hexon-registry: `read_zip_bytes`
  (`fe-hexon-registry/src/index.rs:34-46`). That is the shape the offset index generalises.

### Settings / per-petal render distance

- `AppSettings.render_distance` is global, default 500, clamped to `RENDER_DISTANCE_MAX` 50 000
  (`fe-ui/src/settings.rs:54,85,104-107`). `PetalRenderOverride` is session-only and set by
  `manifest_save` (`fe-ui/src/actions/hexon.rs:233-249`; `settings.rs:317-345`).
  `ResidencyBudget::render_distance_for` is the one read path (`spawn.rs:64-79`).
- The `petal` table (`fe-database/src/schema.rs:69-90`) has no `render_distance` column.
  `PetalHierarchyData` (`fe-runtime/src/messages.rs:1110-1116`) and `PetalEntry`
  (`fe-ui/src/verse_manager/mod.rs:65-70`) do not carry one.
- Replication pattern: `replicate_row_with_petal` (`fe-database/src/lib.rs:163-210`). Inbound
  petal rows are `MERGE`d by `apply_static_row` (`handlers/replicated_row.rs:184-198,324-400`)
  behind the A3 Editor+ gate (`:261`). The fe-ui tree does **not** react to
  `ReplicatedRowApplied` today (no arm in `verse_manager/db_results/`).
- Auth pattern for a scoped petal write: `set_scope_hidden_reply` → `authorize_edit`
  (`handlers/visibility_groups.rs:208-247`). REST/MCP pattern: `set_petal_terrain_core`
  (`fe-api/AGENTS.md:967-985`; `fe-api/src/mcp/mod.rs:209`).

### PAS-P1V

- RelayConfig, the default-relay warning, and health all exist (`fe-sync/src/relay_config.rs:89-140`;
  `fe-sync/src/AGENTS.md:461-572`). The 0.35 "EOL warning" is now the **rate-limited
  default-relay warning** (`AGENTS.md:562-570`), so the ledger wording drifted. What is still
  missing is a recorded headless run.

## 3. Slices (strictly sequential; one executor each)

Shared rules for every slice:

- Terse doc-comments in code. Rationale goes in the directory `AGENTS.md`.
- No bare `.ok()` on channel sends: `warn!` instead (root AGENTS anti-pattern #6).
- All new sync-thread file I/O uses `spawn_blocking` (`fe-sync/src/AGENTS.md`
  §sync-thread-blocking-io; NFR-3).
- No per-frame allocation in hot systems (anti-pattern #7).
- Never edit RUNBOOK.md, tracks.md, doneness-ledger.md, or the outstanding_decisions files.

---

### S1 — Deterministic spatial bundler + bundle index (fe-format, fe-terrain) · size M

**Owned files:** `fe-format/src/manifest.rs` (append only), `fe-format/src/AGENTS.md`,
`fe-terrain/src/tiles/bundle.rs` (new), `fe-terrain/src/tiles/mod.rs`,
`fe-terrain/src/tiles/distribution.rs`, `fe-terrain/src/tiles/builder.rs`,
`fe-terrain/src/tiles/store.rs` (**critic: `hexon_id` charset guard only**, see step 6),
`fe-terrain/src/AGENTS.md`, `fe-terrain/tests/tileset_bundles_test.rs` (new).

**Steps**

1. In fe-format, add `TilesetBundleIndex { schema: u32, tileset_id, tileset_meta: TilesetMeta
   (chunk_index = None), bundles: Vec<BundleEntry> }` and `BundleEntry { seq, blake3_hex,
   byte_len, tile_count, zoom: u8, x_range: [u32;2], y_range: [u32;2], hilbert_range: [u64;2] }`.
   Add `ChunkIndex.blake3: Option<String>` with `#[serde(default, skip_serializing_if)]` so
   legacy chunks stay byte-identical. This is the D-76 per-chunk hash slot that PAS-P5 will sign.
2. Add `fe-terrain/src/tiles/bundle.rs` with `bundle_tileset(source: &HexonArchiveData,
   max_bundle_bytes: u64) -> Result<TilesetBundles { index: TilesetBundleIndex, index_json:
   Vec<u8>, bundles: Vec<Vec<u8>> }>`:
   - group elevation and satellite by `(z,x,y)` so a pair never splits;
   - order coordinates by `(z, hilbert(x,y))` (PMTiles-style, per D-73);
   - greedily fill to `max_bundle_bytes`, with a single oversize pair allowed alone;
   - each bundle is a self-contained `.hexon` chunk archive via `HexonArchive::export_tileset`
     with `ChunkIndex { chunk_bounds = the bundle's real lat/lon extent }`;
   - **timestamps come from the source manifest, never `now()`**;
   - `DEFAULT_BUNDLE_BYTES = 1 MiB` (evidence §3);
   - an entity hexon / sub-threshold tileset yields exactly one bundle.
3. Add `assemble_bundles(store, index, bundle_paths) -> Result<InstalledTileset>`. It checks
   `blake3(file) == index.bundles[seq].blake3_hex` for every seq, then delegates to the existing
   `assemble_chunks` (`distribution.rs:161`). Keep the original `hexon_id`.
4. Re-point `package_chunked` at the shared splitter, so there is one split implementation. Fix
   its `chunk_bounds` and `now()` (builder.rs:344,374).
5. Docs: fe-terrain `AGENTS.md` new §bundles (determinism, pair co-location, Hilbert order, sizing
   evidence link); fe-format `AGENTS.md` §manifest gets the `TilesetBundleIndex` / `ChunkIndex.blake3` note.
6. **(Critic, ordering fix — moved here from PAS-P5 S6.)** `HexonStore::hexon_path` / `meta_path`
   (`store.rs:281-287`) join the archive's `hexon_id` into a path unchecked, so `../x` writes
   outside the store. S4 makes P2P-assembled installs reachable from **untrusted peers** in this
   campaign, before PAS-P5 runs, so the guard must land here: validate `hexon_id` with the
   registry charset (`[A-Za-z0-9_.-]`, non-empty, not `.`/`..`) in `install_tileset` **before any
   path join**, refusing with a typed error. PAS-P5 S6 keeps it as a regression row only.
7. **(Critic, one mapping not two.)** Add the pure `TilesetBundleIndex::bundle_for_tile(z, x, y)
   -> Option<u32>` in fe-format (over `x_range`/`y_range`/zoom). S7's `StreamedTileSource` and
   PAS-P5 S2 both call it; nobody re-implements the tile→bundle lookup.

**Invariants:** bundling is a pure function of the source bytes. The same input gives identical
bundle bytes, so identical blake3 values, which is the identity the pointer-set and cross-hexon
dedup depend on (decision record "Ratification" §1). Legacy archives without `blake3` still
import (fe-format `AGENTS.md` §archive caps untouched; every read stays under `limits::*`).

**Flow tests** (`fe-terrain/tests/tileset_bundles_test.rs`): build a synthetic multi-zoom
tileset with ~100 KB pseudo-random tile payloads (evidence: real pairs are ~110 KB), and use a
256 KiB bundle size to force at least 4 bundles.

- `a_bundled_tileset_reassembles_to_identical_tiles`: bundle → write files → `assemble_bundles`
  into a temp `HexonStore` → reload with `load_tileset` → every elevation and satellite tile is
  byte-equal to the source.
- `bundling_twice_yields_identical_hashes`: the determinism invariant.
- `elevation_and_satellite_share_a_bundle_and_bundles_are_hilbert_runs`: table over all coords;
  no bundle exceeds the max unless it holds a single pair.
- `assembly_rejects_a_bundle_whose_hash_disagrees_with_the_index`: one flipped byte → error
  naming the seq, nothing installed.
- `a_tiny_tileset_ships_as_one_bundle`.
- **(critic)** `install_refuses_path_escaping_hexon_ids` (table: `../x`, `a/b`, `a\\b`, `C:x`,
  `..`, empty): typed error, no file written outside `base_dir`, `registry.json` unchanged.
- **(critic)** `bundle_for_tile_table` (pure, table-driven): inside, on a range border, outside,
  wrong zoom.

---

### S2 — Transport trait, in-memory + iroh impls, `TilesetTransfer` service (fe-sync) · size L

**Owned files:** `fe-runtime/src/blob_transport.rs` (new — **critic:** the iroh-type-free trait
and `InMemoryNetwork`/`InMemoryTransport` live here, per PAS-P5 contract C-P3 #1, so PAS-P5's
fe-hexon P2P impl needs no fe-sync dependency), `fe-runtime/src/lib.rs` (mod only),
`fe-runtime/Cargo.toml` (`async-trait`, `bytes` from the workspace), `fe-runtime/src/AGENTS.md`
(§blob-transport, one paragraph), `fe-sync/src/blob_transport.rs` (new — `IrohBlobTransport`
only), `fe-sync/src/tileset_transfer.rs` (new),
`fe-sync/src/lib.rs` (mod + re-exports only), `fe-sync/Cargo.toml` (`async-trait` only if not
already transitively usable; no new heavy deps), `fe-sync/src/AGENTS.md` (new §tileset-transfer),
`fe-sync/tests/tileset_transfer_test.rs` (new).

**Steps**

1. `BlobTransport` trait (D-77: hash + HashSeq semantics only; no `RangeSpec`, `GetRequest`,
   `ChunkRanges` or any other iroh type in its signature):
   `add(bytes) -> Hash32`, `add_hash_seq(&[Hash32]) -> Hash32`,
   `set_tag(name, root)`, `delete_tag(name)`, `list_tags(prefix) -> Vec<(String, Hash32)>`,
   `fetch_hash_seq(provider: &ProviderRef, root) -> Vec<Hash32>` (root blob only),
   `fetch(provider, hash) -> Bytes`, `read_local(hash) -> Option<Bytes>`.
   - `Hash32` is `fe_runtime::blob_store::BlobHash` (`[u8;32]`).
   - `ProviderRef` is an opaque `EndpointAddr` JSON string (the same format
     `SyncEvent::Started.node_addr` carries).
   - No byte-range method: nothing would consume it yet (D-73 A3 is a future layout change).
2. `InMemoryNetwork` + `InMemoryTransport`: one shared map per "peer", plus `tamper(hash, bytes)`
   and `withhold(hash)` test hooks. This is a production-compiled pub type, not `cfg(test)`, so
   the harness and fe-sim can use it.
3. `IrohBlobTransport { store: iroh_blobs::api::Store, endpoint: SyncEndpoint }`:
   - `add` → `store.blobs().add_bytes`;
   - HashSeq root → `HashSeq::from_iter(..)` then `add_bytes` plus a named tag;
   - fetch → parse the provider `EndpointAddr`, `endpoint.add_peer_addr`
     (`fe-sync/src/endpoint.rs:143`), `store.downloader(endpoint.inner()).download(
     HashAndFormat::raw(hash), Shuffled::new(vec![id]))`, then `get_bytes`;
   - root-only fetch uses `GetRequest::builder().root(ChunkRanges::all()).build(root)`.
   - Every await is bounded by a per-request timeout constant.
4. `TilesetTransfer` service (pure orchestration over `Arc<dyn BlobTransport>` + a download root
   dir):
   - `publish(tileset_id, index_json, bundle_paths)` → HashSeq `[index, b0..bn]`, tag
     `fe/tileset/<id>`, catalog entry `{id, root, total, bytes}`;
   - `catalog_from_tags()` rebuilds the catalog at start (tags persist in the fs store);
   - `unpublish(id)`;
   - `request_meta(provider, tileset_id, root)` → `MetaResolved { root, chunk_hashes, index_json }`.
     The index blob is child 0 and passes through **opaque**;
   - `fetch_chunk(seq)` → bytes → **`blake3(bytes) == chunk_hashes[seq]` or reject** → atomic
     write `downloads/<id>/<seq>.bundle` (tmp + rename, `spawn_blocking`);
   - the persisted ledger `downloads/<id>/download.json { tileset_id, root, provider_candidates,
     chunk_hashes, index_hash }` is written on start. "Received" is **derived** from files whose
     blake3 verifies, so the ledger can never claim a chunk the disk does not hold;
   - `resume_all()` re-verifies files and re-queues only the missing seqs;
   - `cancel(id)` deletes the ledger, files, and the download tag;
   - bounded concurrency is a constant (4) with provider fallback in candidate order.
5. AGENTS §tileset-transfer: the trait rationale (D-77 drift note: E2 kept on 1.3 for test
   isolation, PAS-P5 HTTP parity, and 0.103 API churn), the on-disk layout, tag names, and the
   "received is derived from verified files" invariant.

**Invariants:** NFR-3 (no blocking I/O on the current-thread runtime). fe-sync never parses
fe-format types: the index blob is opaque bytes, and verification is against HashSeq children.
Tags protect published and in-flight roots from any future GC.
**(Critic, security)** `tileset_id` arrives from untrusted advertisements and is joined into
`downloads/<id>/…` and tag names: validate it with the same charset as S1 step 6 **before** any
path join or tag write, in `TilesetTransfer` (the one choke point), and refuse otherwise.
`ProviderRef` addresses come from the wire too: dial only via `endpoint.add_peer_addr` with the
per-request timeout, never an arbitrary URL.

**Flow tests** (`fe-sync/tests/tileset_transfer_test.rs`, two `TilesetTransfer`s over one
`InMemoryNetwork`, real temp dirs):

- `a_published_tileset_downloads_verified_and_complete`: every on-disk file's blake3 equals its
  HashSeq child, and the `Complete` outcome lists all seqs.
- `a_tampered_bundle_is_rejected_and_never_persisted`: `tamper(child2)` → `ChunkFailed{hash
  mismatch}`, no `2.bundle` on disk, not complete. After the provider is repaired, retry
  completes.
- `an_interrupted_download_resumes_after_restart_without_refetching`: download seqs 0-1 → drop
  the service → the provider `withhold`s 0-1 → a new service on the same dir runs `resume_all`
  and completes. This can only pass if 0-1 were **not** re-fetched (outcome-based, no call
  counts).
- `cancel_discards_partial_state`: after cancel, the dir and ledger are gone; a fresh request
  starts at zero.
- `iroh_transport_round_trips_a_hash_seq_on_loopback`: two real endpoints (`RelayMode::Disabled`,
  `MemoryLookup`, fs stores in temp dirs, mirroring iroh-blobs `examples/transfer-collection.rs`),
  publish → fetch root → fetch children → bytes equal.
- `catalog_survives_a_stack_restart`: publish on a `DocsStack` in a temp dir → shut down →
  respawn on the same dir → `catalog_from_tags` lists the same root.
- **(critic)** `a_tileset_id_that_escapes_the_download_dir_is_refused` (table: `../x`, `a/b`,
  empty): the request fails typed, and nothing is created outside `downloads/`.

---

### S3 — Sync-thread wiring, messages, advertisements, relay-health log (fe-sync) · size M

**Owned files:** `fe-sync/src/sync_thread.rs`, `fe-sync/src/messages.rs`, `fe-sync/src/status.rs`,
`fe-sync/src/lib.rs`, `fe-sync/src/AGENTS.md` (§tileset-transfer wiring + update the
§iroh-1.x "advertisement outbound-only" paragraph at `:117-124`).

**Steps**

1. Messages:
   - add `SyncCommand::PublishTileset { tileset_id, index_path, bundle_paths }`,
     `UnpublishTileset { tileset_id }`, and `DownloadTileset { tileset_id, provider:
     Option<String> }`;
   - keep `RequestTilesetMeta` / `RequestChunk` as the lower-level primitives. `RequestChunk`
     gains `provider: Option<String>` (S7 uses it to fetch single bundles for streaming);
   - `TilesetMetaReceived` gains `root_hash`, `chunk_hashes`, and `index_path`;
   - `ChunkReceived` swaps `chunk_bytes: Vec<u8>` for `path: PathBuf, byte_len: u64` (no MBs
     through Bevy channels; breaking change, no external consumers, RUNBOOK §3 2026-10-12);
   - new `TilesetDownloadComplete { tileset_id, index_path, bundle_paths }`,
     `TilesetPublished { tileset_id, root_hash }`, and `TilesetDownloadResumed { tileset_id,
     received, total }`;
   - extend the `status.rs:135-137` routing arm so the new events reach `TilesetEventBuffer`.
2. Build one `TilesetTransfer` after the `DocsStack` spawn (`sync_thread.rs:111-145`) when a
   stack exists. With no stack (offline/virtual), every tileset command answers `ChunkFailed` /
   `TilesetMetaReceived` with an honest "P2P offline" reason; no silent drop.
3. Run transfer futures as spawned tasks that report through an mpsc into a new `select!` branch
   (same pattern as `gossip_inbound_rx`, `:655`). Replace the stub handlers at `:2097-2136` with
   thin adapters. **Wire `TilesetDownloadTracker`**: replace the `HashMap` at `:2010-2035` with
   the persisted `TilesetTransfer` ledger and delete the `dead_code` allows. Call `resume_all()`
   once after the stack is online and emit `TilesetDownloadResumed` per resumed download.
4. Advertisements:
   - outbound is built **from the catalog**: `{kind: "tileset_ad", tileset_id, root_hash,
     total_chunks, approx_bytes, provider_addr}`, broadcast on publish and on `AdvertiseTilesets`
     (an empty payload now means "re-advertise my catalog", which is what fe-ui already sends);
   - inbound: before `handle_gossip_incoming`, try `decode_tileset_ad`. Mark it
     `authenticated = incoming.direct && provider id == incoming.from` (F23 pattern,
     `fe-sync/src/AGENTS.md` §transport-admission-control). Unauthenticated ads are hints,
     ordered after authenticated ones. Keep a provider map `tileset_id → [(provider, root,
     authenticated)]` and emit `PeerTilesetAdvertisement`;
   - delete the dead fe-sync `TilesetAdvertisement` struct (`:2001-2007`).
5. PAS-P1V support: if no `info!` is emitted on a `RelayHealthChanged` application, add one in
   `status.rs` (one line, for the headless log capture).

**Invariants:** the command loop is byte-identical between the real and virtual transports
(`fe-sync/src/AGENTS.md` §virtual-transport). Virtual mode just has no `TilesetTransfer` and
answers honestly. F23 sender-identity gating applies. The `select!` loop never awaits a transfer
inline.

**Flow tests** (in-crate `#[cfg(test)]` in `sync_thread.rs` / a `tileset_ad` module):

- `tileset_ad_codec_and_authentication_table`: direct + matching provider → authenticated;
  relayed → hint; malformed → falls through to `handle_gossip_incoming` unchanged.
- `offline_thread_answers_tileset_commands_honestly`: spawn `spawn_sync_thread_with_transport`
  with a virtual factory → `DownloadTileset` → a `ChunkFailed`/meta event whose reason says
  offline. The real two-peer flow is S4's harness scenario.

---

### S4 — Host wiring: download view, publish, assemble+install, harness, PAS-P1V script (fe-ui, fractalengine, fe-test-harness) · size M

**Owned files:** `fe-ui/src/terrain_map/{events.rs,mod.rs,dto.rs}`, `fe-ui/src/actions/hexon.rs`,
`fe-ui/src/dialogs/{mod.rs,hexon_manager.rs}`, `fe-ui/src/plugin.rs` (resource registration
only), `fe-ui/src/AGENTS.md` (§terrain-map), `fractalengine/src/terrain_bridge.rs`,
`fractalengine/src/main.rs` (system registration + startup publish), `fractalengine/src/AGENTS.md`
(new §tileset-p2p), `fe-test-harness/src/scenarios/two_peer_tileset_stream.rs` (new),
`fe-test-harness/src/scenarios/mod.rs`, `fe-test-harness/src/main.rs`,
`fe-test-harness/src/AGENTS.md` (§scenarios), `scripts/relay-health-verify.ps1` (new),
**(critic)** `fractalengine-relay/src/main.rs` (startup seeding, step 5) and its doc pointer in
`fractalengine/src/AGENTS.md` §tileset-p2p.

**Steps**

1. fe-ui:
   - move `download_progress` out of `ActiveDialog::HexonManager` (`dialogs/mod.rs:142`) into a
     new `TilesetDownloads` resource. The dialog renders from it;
   - `drain_tileset_events` becomes a **view updater**: no more ping-pong `RequestChunk`, because
     the sync thread drives;
   - `start_download` sends `DownloadTileset`;
   - `TilesetDownloadComplete` pushes `HexonOp::AssembleDownload { tileset_id, index_path,
     bundle_paths }`;
   - every `.ok()` send becomes `if let Err(e) … warn!`.
2. fractalengine `terrain_bridge`:
   - `drain_hexon_ops` (`terrain_bridge.rs:125-170`) handles `AssembleDownload` on a background
     thread (`IoTaskPool`): `assemble_bundles` → `registry.install` → refresh the dialog list
     (existing repopulate path);
   - `SetSeeding(id, true)` and startup (every `seeding_enabled` tileset) run `bundle_tileset`
     on a background task into `<hexon store>/bundles/<id>/` (reused when present and
     index-hash-valid), then send `PublishTileset`;
   - `SetSeeding(id, false)` / `Remove` send `UnpublishTileset`.
3. Harness scenario `two_peer_tileset_stream`:
   - alice and bob are real iroh loopback `TestPeer`s (`fe-test-harness/src/peer.rs:59-131`);
   - alice bundles a synthetic tileset (S1 helper, 256 KiB bundles) and publishes;
   - bob issues `DownloadTileset { provider: alice.sync_node_addr }` and waits for
     `TilesetDownloadComplete`, then `assemble_bundles` into a temp `HexonStore`, and every tile
     byte-equals alice's source;
   - **cross-restart:** alice shuts down, bob shuts down, bob respawns on the same `p2p` dir and
     reissues `DownloadTileset` → `TilesetDownloadComplete` with **no provider alive**. This
     proves persisted ledger + local verified state over the real fs store;
   - register it as scenario 16.
4. `scripts/relay-health-verify.ps1` (PAS-P1V) boots `fe-relay` three times with
   `FE_SYNC_RELAY=default | http://127.0.0.1:3340 | disabled`, `FE_P2P_DIR` in a temp dir and a
   ~20 s timeout each. It captures logs to `scripts/relay-health-verify.log` and asserts the
   resolved mode line, the default-relay warning (Default only), and a relay-health line per
   mode. It also kills exactly the PIDs it spawned (CLAUDE.md process-cleanup rule). It is run at
   campaign end and its output pasted into `evidence.md` §6 (S9).
5. **(Critic — moved in from the deferral list; former Q1 is answered by the record.)** The
   RUNBOOK §3 NO-GO "relay seeding" is the canonical-log item: the ledger row that owns it reads
   "**CDL-NET** | Canonical-log network rollout, relay seeding, inbound P2P"
   (`conductor/doneness-ledger.md:263`), and the CDL decision record scopes it the same way
   (`conductor/decisions/canonical-data-log-20260808.md:61`). Tileset bundles over iroh-blobs are
   not canonical-log traffic. So `fractalengine-relay` seeds too: after its `TilesetRegistry` loads
   (`fractalengine-relay/src/main.rs:365`), every installed tileset whose `seeding_enabled` flag is
   set is bundled on a background task (the same S1 helper and `<hexon store>/bundles/<id>/` reuse
   rule as the GUI) and published with `PublishTileset`. It reuses the S4 step-2 code path through a
   shared function, not a copy. This makes the always-on seeder that P2P needs when GUI peers are
   offline.

**Invariants:**
- fe-ui gains no fe-terrain dependency; registry work stays behind `PendingHexonOps` (root
  `AGENTS.md` §petal-map "Registry ops").
- NFR-4: an assembled P2P tileset installs into the hexon store, never into the online-origin
  disk cache.
- Bundling and assembly never run on the Bevy main thread.

**Flow tests:**
- fe-ui `closing_the_hexon_manager_does_not_stall_a_download`: a Bevy App with the dialog closed
  receives meta/chunk/complete events → `TilesetDownloads` shows progress → `PendingHexonOps`
  holds `AssembleDownload`. This is the regression for the ping-pong bug.
- fractalengine `a_completed_download_is_assembled_and_installed`: temp `HexonStore` + real S1
  bundles → the op runs → `registry.list_installed()` contains the id and a tile reads back.
- The harness scenario above.
- **(critic)** fractalengine-relay `a_headless_relay_publishes_its_seeding_tilesets_at_startup`:
  temp `HexonStore` with one seeding and one non-seeding tileset → the startup routine runs → the
  drained `SyncCommand` receiver holds exactly one `PublishTileset` (the seeding id), and its
  bundle files exist on disk.

---

### S5 — Lazy offset-index `HexonTileSource`, `Arc` dedup, async residency, tile-byte exit-ring eviction (fe-format, fe-terrain, fe-api) · size L

**Owned files:** `fe-format/src/archive_index.rs` (new), `fe-format/src/lib.rs`,
`fe-format/src/AGENTS.md`, `fe-terrain/src/tiles/{hexon_source.rs,registry.rs,composite.rs,store.rs}`,
`fe-terrain/src/tiles/residency.rs` (new), `fe-terrain/src/petal_binding.rs`,
`fe-terrain/src/terrain_plugin.rs`, `fe-terrain/src/AGENTS.md`, `fe-api/src/terrain.rs`,
`fe-terrain/tests/lazy_tiles_test.rs` (new), `fe-api/tests/hexon_tileset_test.rs` (extend).

**Steps**

1. fe-format `HexonArchiveIndex::open(path)` does the **header phase**: it reads the central
   directory once and parses `manifest.json`, `entries.json`, and `terrain/tileset_meta.json`
   under the existing `limits` caps. It builds a member table `name → {data_offset,
   compressed_size, size, method}`. `read_member(name)` reads one member by offset under the
   per-entry cap. Stored members use a direct positioned read; deflated ones are inflated within
   the cap. This is also the PAS-P5 "import split" primitive (see cross-cuts): one
   implementation, not two.
2. `HexonTileSource` becomes an index plus a bounded byte cache:
   - fields: `tileset_meta`, `Arc<HexonArchiveIndex>` (or an in-memory member table for
     `from_archive(bytes)`, which stays for tests and assembly), and `cache:
     Mutex<ByteLru<cache_key, Arc<[u8]>>>`;
   - `get_tile` / `get_satellite_tile` return `Option<Arc<[u8]>>`, **cache hit only**, so the
     main thread never reads the disk;
   - `load_tile_blocking(coord)` is the off-thread loader;
   - baked splats load the same lazy way;
   - make the public `tiles`/`satellite` maps private (the only external user is a test,
     `hexon_source.rs:319`).
3. `TilesetRegistry.sources` becomes `HashMap<String, Arc<HexonTileSource>>` and gains
   `source(id) -> Option<Arc<..>>`. `load_all` now opens indexes only (cheap).
   `petal_binding.rs:153` uses `registry.source(uri)`, and on a miss runs `load_tileset` then
   `registry.insert`. `CompositeTileSource.hexon_sources` becomes `Vec<Arc<HexonTileSource>>`
   (evidence §5 sketch, including the "old `Arc` survives until the next assignment" no-tear
   rule).
4. `tiles/residency.rs` `TileResidency` resource:
   - `fetch_and_spawn_terrain_chunks` asks `ensure_resident(coord)`;
   - misses dispatch a load on `IoTaskPool` (bounded in-flight) whose result lands in the
     source cache;
   - a coord whose load is in flight is **pending**: skipped this frame and not recorded in
     `FailedTiles` (`terrain_plugin.rs:487-513`). Only "member absent from every covering index"
     is failed;
   - this realises D-74's "async dispatch" decision and NFR-2 by construction.
5. Exit-ring byte eviction: when `update_terrain_lod` despawns a chunk (`:377-393`), its tile keys
   are released from the cache. The cache is byte-bounded (constant `TILE_BYTE_BUDGET` = 256 MiB;
   exposing it as a knob is not done — no ask). Pinned keys are those of live chunks, and only
   unpinned keys are LRU-evicted.
6. fe-api tile routes (`terrain.rs:504,542`) call the blocking loader inside `spawn_blocking` (the
   same pattern as `:725,733`).

**Invariants:**
- NFR-2: no file I/O in any Update system.
- NFR-4: the Offline-mode branch still never reads the disk cache (`composite.rs:173-182`).
- Every archive read goes through `limits` (zip-bomb guard, W2).
- The registry stays the single owner, and a composite never mutates a shared source.

**Flow tests** (`fe-terrain/tests/lazy_tiles_test.rs`):

- `a_lazy_source_serves_exactly_what_the_eager_import_did`: table over every member of a
  generated archive.
- `registry_and_active_petal_share_one_source`: a Bevy App with `SharedTilesetRegistry` +
  `apply_terrain_assignments` → `Arc::ptr_eq(registry.source(id), composite.hexon_sources[0])`.
- `a_cold_tile_is_pending_then_spawns_never_failed`: App with a moving camera over a real
  archive; the first frame has no chunk and the coord is not in `FailedTiles`; within N frames
  the chunk entity exists.
- `tile_bytes_leave_with_the_exit_ring_and_return_on_reentry`: fly the camera away past
  `1.5 × radius` → resident bytes drop below a threshold; fly back → chunks respawn with
  identical heights.
- fe-api `hexon_tileset_test`: the tile route still returns the installed tile bytes.

---

### S6 — `petal.render_distance` column, replicated, session override on top (fe-database, fe-runtime, fe-ui, fe-api) · size M

**Owned files:** `fe-database/src/schema.rs`, `fe-database/src/handlers/petal_render_distance.rs`
(new), `fe-database/src/handlers/mod.rs`, `fe-database/src/lib.rs` (dispatch arms),
`fe-database/src/handlers/crud.rs` (hierarchy loader field only, `:860-868`),
`fe-database/src/handlers/replicated_row.rs` (inbound validation of the new field only),
`fe-database/src/AGENTS.md`, `fe-runtime/src/messages.rs`, `fe-runtime/src/AGENTS.md`,
`fe-ui/src/settings.rs`, `fe-ui/src/actions/hexon.rs` (`manifest_save`),
`fe-ui/src/verse_manager/{mod.rs,spawn.rs}`, `fe-ui/src/verse_manager/db_results/*`,
`fe-ui/src/ui_shell/right_sidebar.rs`, `fe-ui/src/AGENTS.md` (§app-settings), `fe-api/src/terrain.rs`
(or a new `render_distance.rs`), `fe-api/src/server.rs`, `fe-api/src/mcp/{mod.rs,tools.rs}`,
`fe-api/AGENTS.md`, `fe-database/tests/petal_render_distance_test.rs` (new),
`fe-api/tests/petal_render_distance_api_test.rs` (new).
**(Critic — files the slice needs but did not own; verified at `877e801`):**
`fe-runtime/src/app.rs` (`correlation_of_command`/`correlation_of_result`, `:229-260`; without
the new arms the correlated REST reply is never routed — the `_ => None` arm hides it at compile
time), `fe-test-harness/src/peer.rs` (its `DbCommand` match is **exhaustive**, no wildcard; the
only catch-all is `Shutdown | Err(_)` at `:1086`), `fe-database/src/migration/mapping_inventory.rs`
(add `SetPetalRenderDistance` and `SetPetalTerrain` to `OTHER_DISPATCHED_MUTATION_KINDS`,
`:49`, per `migration/AGENTS.md` §inventory-scope "in the same change"),
`fe-database/src/handlers/petal_terrain.rs` (step 8).

**Steps**

1. Schema: `render_distance: Option<f64> => "TYPE option<float> ASSERT $value = NONE OR
   ($value >= 0 AND $value <= 50000)"`. 50000 is `RENDER_DISTANCE_MAX`. NONE means "inherit the
   global".
2. `DbCommand::SetPetalRenderDistance { petal_id, render_distance: Option<f32>, auth:
   CallerAuth, correlation_id }` and `GetPetalRenderDistance`, replying
   `DbResult::PetalRenderDistanceLoaded { petal_id, render_distance, correlation_id }`. Failures
   use the correlated pattern (fe-runtime `AGENTS.md` §api-reply-correlation).
3. Handler: resolve the petal scope → `authorize_edit` (Editor+, `visibility_groups.rs:208-226`
   pattern) → `UPDATE … RETURN AFTER` → `replicate_row_with_petal(…, "petal", petal_id,
   full_row_json, Some(petal_id))` with `verse_id` resolved through fractal → verse. The echo is
   the read-back value (house doctrine: the echo is authoritative).
4. Inbound: `apply_static_row` already MERGEs petal fields. Add the strictest-reader check: a
   non-null `render_distance` outside `[0, 50000]` or non-finite → `Denied` (RUNBOOK §5 "writer
   of a shared column must satisfy the strictest reader").
5. Hierarchy: `PetalHierarchyData.render_distance: Option<f32>` and the loader reads it;
   `PetalEntry.render_distance` follows.
6. fe-ui:
   - precedence `effective_render_distance(global, petal_column, session_override)`: session
     beats petal column, petal column beats global, each value finite and positive else skipped,
     everything clamped (`settings.rs:337-345` grows one argument);
   - `manifest_save` sends `SetPetalRenderDistance` (durable, auth `CallerAuth::Local`) instead
     of setting the session override;
   - the right-sidebar Settings section shows "This petal: <v> (shared)" (editable when the
     local role is Editor+, else read-only) and a "Session override" field that sets
     `PetalRenderOverride` (a local-only knob for viewers on weak machines; cleared on petal
     change, as now);
   - `ReplicatedRowApplied { table: "petal", outcome: Applied }` → `GetPetalRenderDistance` for
     that petal, so a peer's change lands without a full hierarchy reload.
7. fe-api: `GET|PUT|DELETE /api/v1/petals/{p}/render-distance` + MCP
   `set_petal_render_distance` (Editor+, petal scope, existence-oracle uniform denial per
   fe-api `AGENTS.md` §existence-oracle).
8. **(Critic — the `SetPetalTerrain` parity gap moved in; the deferral was not justified by a user
   decision, a NO_GO or an external dependency.)** Step 3 replicates the **full** petal row, and
   inbound `apply_static_row` MERGEs the whole payload (`replicated_row.rs:349-363`). So S6 would
   already carry `petal.terrain` to peers on every render-distance change, unvalidated, while a
   terrain-only change never replicates: half-convergence plus an unchecked shared column.
   Therefore:
   - `SetPetalTerrain` (`handlers/petal_terrain.rs`) replicates through the same
     `replicate_row_with_petal` call after its read-back echo;
   - inbound petal rows apply the strictest-reader check to `terrain` too: NONE, or a JSON object
     under a byte cap whose `tileset_hexon_uris` is an array of strings in the hexon-id charset
     (S1 step 6). Anything else → `Denied`. fe-database has no fe-terrain dependency, so this is
     the structural shape check; `terrain_bridge`'s `terrain_config_from_petal_json` stays the
     second line of defence;
   - both writes op-log first (fe-database `AGENTS.md` §log-first-commit), then mutate.

**Invariants:**
- Precondition validation precedes the write (fe-database `AGENTS.md` §log-first-commit). Both
  `SetPetalRenderDistance` and `SetPetalTerrain` op-log before they mutate (step 8).
- The A3 gate runs before mode dispatch (§replication-mode).
- The `RENDER_DISTANCE_MAX` ceiling also binds the petal column (`fe-ui/src/AGENTS.md:716-718`).

**Flow tests:**
- fe-database `petal_render_distance_test`:
  - `set_then_load_hierarchy_carries_the_value`;
  - `clearing_restores_inherit`;
  - `a_viewer_cannot_set_it`;
  - one table-driven `out_of_range_values_are_refused` (NaN, -1, 50001);
  - `an_editor_authored_replicated_petal_row_applies_and_a_viewer_authored_one_is_denied`
    through the real `ApplyReplicatedRow` handler;
  - `an_inbound_out_of_range_value_is_denied`.
  - **(critic)** `set_petal_terrain_replicates_and_a_peer_converges_on_it` (real `repl_tx`
    capture, then `ApplyReplicatedRow` on a second DB: its `terrain` equals the author's).
  - **(critic)** `an_inbound_petal_terrain_that_fails_the_shape_check_is_denied` (table: not an
    object, a non-string URI, a `../` URI, oversize).
  - **(critic)** `petal_settings_writes_land_in_the_op_log_before_the_row` (one row per command).
  - **Note for role propagation:** RP-3 flips the viewer-authored expectation in this file from
    `Denied` to `Deferred`; RP-3 owns that edit.
- fe-api `petal_render_distance_api_test`: PUT → GET round trip; DELETE → inherit; viewer
  denied; a foreign petal answers the uniform not-found-or-denied body.
- fe-ui: a precedence table test, and `manifest_save_persists_to_the_petal_column` (the
  `DbCommand` on the channel is the observable outcome at the thread seam).

---

### S7 — Streamed (not-installed) tilesets: tile pointer-set → bundle demand over P2P (fe-terrain, fractalengine) · size M

**Owned files:** `fe-terrain/src/tiles/streamed.rs` (new), `fe-terrain/src/tiles/{composite.rs,mod.rs}`,
`fe-terrain/src/petal_binding.rs`, `fe-terrain/src/AGENTS.md` (§streamed-tilesets),
`fractalengine/src/terrain_bridge.rs`, `fractalengine/src/main.rs`, `fractalengine/src/AGENTS.md`.

**Steps**

1. `StreamedTileSource { tileset_id, index: TilesetBundleIndex, resident:
   HashMap<seq, PartialTileSource-backed bundle> }` maps `(z,x,y)` → bundle seq through
   `TilesetBundleIndex::bundle_for_tile` (S1 step 7; critic: no second lookup). `CompositeTileSource` gains `streamed_sources` (checked after
   installed hexon sources, before the disk cache; Offline mode includes them because the bytes
   are hexon-origin, which preserves NFR-4).
2. **Tile pointer-set:** a `TileBundleDemand` resource, written each frame from the ring
   `fetch_and_spawn_terrain_chunks` wants, `{(tileset_id, bundle_hash)}`. This is the D-74
   pointer-set for tiles. Bundles demanded and not resident are emitted as
   `BundleFetchRequested` messages. Bundles of chunks evicted on the exit ring are dropped from
   memory (the disk copy stays in the download dir as a cache).
3. `petal_binding`: a `tileset_hexon_uri` that is **not installed** but has an advertised provider
   gets a `StreamedTileSource`. The binary supplies the index (from `TilesetMetaReceived.index_path`
   via `RequestTilesetMeta`). Until the index arrives the petal renders nothing for that source
   (pending, not failed).
4. `terrain_bridge` forwards `BundleFetchRequested` → `SyncCommand::RequestChunk { provider,
   tileset_id, chunk_seq }` and `ChunkReceived { path }` → loads the bundle off-thread into the
   streamed source. It also keeps the provider map from `PeerTilesetAdvertisement`.

**Invariants:**
- Spawns are gated before any spike: a chunk spawns only when its bundle is resident (NFR-1).
- Over-fetch at a ring edge is bounded to one bundle (D-73 evidence §3).
- No main-thread I/O.

**Flow tests** (fractalengine `terrain_bridge` tests; the thread boundary is the seam — a real
`crossbeam` `SyncCommand` receiver is drained by the test, which answers with `ChunkReceived`
pointing at real S1 bundle files):

- `a_petal_on_an_uninstalled_tileset_demands_only_ring_bundles`: the demanded seq set is a strict
  subset of all bundles and equals the ring's bundles.
- `answered_bundles_become_terrain_chunks`.
- `moving_the_camera_demands_new_bundles_and_releases_old_ones`.

---

### S8 — fe-ui residency ledger: node + stamp-group pointer-set, camera-forward weighting, hysteresis, GPU-pressure loop (fe-ui, fe-runtime) · size L

**Owned files:** `fe-ui/src/verse_manager/residency_ledger.rs` (new),
`fe-ui/src/verse_manager/{mod.rs,spawn.rs,petal_respawn.rs,primitive_materialize.rs,path_asset_materialize.rs}`,
`fe-ui/src/plugin.rs` (watchdog stays; registration only), `fe-ui/src/ui_shell/right_sidebar.rs`
(residency readout line), `fe-ui/src/AGENTS.md` (§residency-ledger rewrite),
`fe-ui/src/verse_manager/AGENTS.md`, `fe-runtime/src/diag15m.rs`, `fe-runtime/src/app.rs`,
`fe-runtime/src/AGENTS.md`.

**Steps**

1. `ResidencyLedger` resource: `admitted_nodes: HashSet<String>`, `admitted_stamp_groups:
   HashSet<String>` (track ids), `epoch`, and reusable scratch buffers (no per-frame allocation).
   This is the node/stamp half of the D-74 pointer-set; S7's `TileBundleDemand` is the tile half.
2. `update_residency_ledger` goes into the chain **after `apply_lifecycle_events` +
   `apply_db_results`, before `respawn_on_petal_change`**. It must see this frame's tree and
   precede every spawner. That is how D-74's "first system" is read: first ahead of all
   spawners. It works as follows:
   - weighted distance = `d × (1 − w·max(0, cosθ))`, where θ is the angle to camera-forward,
     `w = 0.5` (B2);
   - **enter** at weighted ≤ r, **exit** at raw distance > 1.5 r. The exit uses raw distance so
     rotating the camera can never evict anything inside r (no rotation thrash);
   - ranked nearest-first and capped at `entity_cap.min(MAX_PETAL_NODES)`;
   - when `mesh_budget.exceeded`, nothing new is admitted, but nothing inside the ring is evicted
     either (prevention, not recovery, NFR-1);
   - r = `ResidencyBudget::render_distance_for(petal)` (S6 precedence) × the GPU-pressure scale
     (step 5).
3. Spawners consume the ledger instead of `if exceeded {0} else {..}`:
   - `petal_respawn` spawns only admitted nodes;
   - a new `apply_node_residency` system (right after `respawn_on_petal_change`) spawns
     newly-admitted nodes (≤ `MAX_NODE_SPAWNS_PER_FRAME`) and despawns evicted ones
     (`.despawn()`, never recursive: anti-pattern #5; respect the deferred-despawn rule);
   - `primitive_materialize` only materialises admitted primitive nodes;
   - **stamp groups are the residency unit for stamps**: a track group is admitted if its nearest
     sample is within r and evicted beyond 1.5 r, and on restamp samples are sorted nearest-first
     so `take_stamp_budget` truncates the farthest. `PathAssetApplied` fingerprints include the
     admission bit, so a group restamps only on a residency transition and never per frame (the
     existing churn concern, spec NFR-1).
   - The selected node (`NodeManager`) is always admitted (never despawn what the user is editing).
4. `distance_ranked_allowance` stays as the pure taper used for the stamp budget. The batch
   "distance 0" comment block (`fe-ui/src/AGENTS.md:751-761`) is rewritten.
5. **B3 closed loop.** Promote DIAG-15M: add `RenderPressure(Arc<AtomicU64>)`, inserted in both
   the main and render worlds. `diag_log` (render world) stores instance + preprocess buffer bytes
   into it; the debug logging stays. Pure `radius_scale_for_pressure(bytes)` returns 1.0 below
   1 GiB, a linear ramp to 0.25 at 1.5 GiB, and 0 (halt admission) at ≥ 1.75 GiB, against the
   2 GiB bind-group limit. The ledger multiplies r by it.
6. Right-sidebar readout: "Resident: N nodes · M stamp groups · K tile bundles (X MiB)", reading
   the ledger plus `TileBundleDemand` / `TileResidency` byte counters. It needs a fe-terrain → fe-ui
   path, so the binary mirrors two counters into a fe-runtime `ResidencyStats` resource (fe-ui
   still has no fe-terrain dependency).

**Invariants:**
- Hard backstops untouched (`MAX_PETAL_NODES`, `MAX_STAMPS`, `MAX_STAMPS_PER_PETAL`, 2 M
  watchdog, terrain `max_chunks` — decision record "Ratification" §3).
- `kept_node_ids` is collected before despawn commands (root `AGENTS.md` "Commands").
- Stamp delete-cascade `stamp_is_orphaned` is preserved verbatim (`fe-ui/src/AGENTS.md:770-773`).
- Under 16 params per system (`ResidencyBudget` `SystemParam` pattern).

**Flow tests:**
- `residency_ledger.rs` pure, table-driven: ranking order; the forward-weighting effect; the
  enter/exit band; cap saturation; NaN/negative inputs → admit nothing; pressure scale table.
- `moving_camera_streams_nodes_in_and_out_with_hysteresis`, a Bevy App over a petal of 200 nodes
  on a line:
  - step the camera along the line → the set of `SpawnedNodeMarker` ids at each step equals
    {nodes within r} ∪ {previously admitted within 1.5 r};
  - oscillating the camera ±0.2 r around a boundary inside the band causes **zero** spawns or
    despawns (churn assertion on entity sets, not call counts);
  - flying back re-admits.
- `rotating_in_place_never_evicts_inside_r`.
- `mesh_budget_exceeded_freezes_admission_without_evicting`.
- `a_far_stamp_group_is_evicted_and_restamped_on_return`.
- `gpu_pressure_shrinks_the_admitted_set`: set `RenderPressure` to 1.6 GiB → fewer admitted nodes;
  1.8 GiB → no new admissions.

---

### S8b — Adversarial SECURITY review-and-fix (critic-added) · size M · opus, fresh context

**Why it was added:** the plan had no review step, even though RUNBOOK §7 step 3 says every
workflow has "a reviewer that fixes". S2/S3/S4 take untrusted network input (advertisements,
provider addresses, bundle bytes, tileset ids), S1/S4 install archives assembled from peers, and
S6 adds an authz-gated write plus an inbound replicated column.

**Owned files:** fix-only edits to any S1–S8 file. The one-line verdict is recorded by the
orchestrator in the RUNBOOK review ledger, because this plan does not own that file.

**Brief (refute, don't confirm):**
- path traversal through `hexon_id` (S1 step 6) and `tileset_id` (S2) at every join site;
- zip-bomb caps (`fe_format::limits`) on every read of a peer-supplied bundle, including
  `assemble_bundles` → `assemble_chunks`;
- F23 sender-identity gating of tileset ads, and that an unauthenticated ad can only *order*
  providers, never make the download skip the per-child blake3 check;
- Risk 2, the blob ALPN serving any hash (`docs_engine.rs:120`). Decide and implement, or record
  why not, an iroh-blobs request-event filter. Constraint: it must keep `SyncCommand::FetchBlob` /
  `two_peer_blob_exchange` and CDL-SPEC10 A6's device-payload blob fetch working, so the filter keys
  on verse membership, never on "tileset tags only";
- S6: Editor+ through `authorize_edit`; the inbound `render_distance` **and** `terrain` shape
  checks; existence-oracle uniform denial on the REST/MCP routes;
- no `.ok()` on sends, and no blocking I/O on the sync thread (NFR-3).

**Tests:** each finding fixed gets a named test that fails if the guard is removed.

---

### S9 — Docs, decision record, track plan, evidence (docs only) · size S

**Owned files:** `conductor/tracks/p2p_asset_streaming_20260718/{plan.md,spec.md,evidence.md,metadata.json}`,
`conductor/decisions/p2p-streaming-20260718.md`, root `AGENTS.md` (§petal-map: one line on the
render-distance column, and the fe-sync blurb on tileset transfer). **Not** RUNBOOK, tracks.md,
the ledger, or outstanding_decisions.

**Steps**

1. Tick Phase 3/4 tasks with file:line evidence.
2. Record **2.6** (see §7).
3. Add a D-77 drift note to the decision record ("trait kept on iroh 1.3 / blobs 0.103.1;
   reasons"), and correct "no ALPN/router" to "Router registered since the 1.3 port".
4. Spec open questions: strike 2.6 and the PAS-P4B durable-store question (now the column).
5. evidence.md §6: an empty "PAS-P1V run" slot for the campaign-end log excerpt.
6. Verify every AGENTS section named in S1–S8 exists and is consistent (one terse pointer per
   code site).

## 4. Docs to update (summary)

- `fe-sync/src/AGENTS.md`: new §tileset-transfer (S2/S3); rewrite of the §iroh-1.x
  advertisement paragraph (`:117-124`).
- `fe-terrain/src/AGENTS.md`: new §bundles (S1), §lazy-tiles/§tile-residency (S5),
  §streamed-tilesets (S7); updates to §petal_binding (Arc sharing) and §terrain_plugin (pending
  vs failed).
- `fe-format/src/AGENTS.md`: §manifest (`TilesetBundleIndex`, `ChunkIndex.blake3`) and new
  §archive-index (S5).
- `fe-ui/src/AGENTS.md`: §residency-ledger rewritten (S8), §app-settings precedence + column
  (S6), §terrain-map download view (S4).
- `fe-ui/src/verse_manager/AGENTS.md`: chain order including the ledger and
  `apply_node_residency` (S8).
- `fe-database/src/AGENTS.md`: §handlers (render distance) and §replication-mode note (petal
  field validation) (S6).
- `fe-runtime/src/AGENTS.md`: new messages (S6), and DIAG-15M → `RenderPressure` /
  `ResidencyStats` (S8).
- `fe-api/AGENTS.md`: route table + §render-distance (S6).
- `fractalengine/src/AGENTS.md`: §tileset-p2p (S4/S7).
- `fe-test-harness/src/AGENTS.md`: §scenarios scenario 16 (S4).
- Track `plan.md` / `spec.md` / `evidence.md` and the decision record (S9).

## 5. Risks

1. **Advertisement trust before PAS-P5 signatures.** A tileset_id → root mapping is
   authenticated only as "the provider vouches for its own root". A verse member can advertise
   junk under a known id. Mitigations now: F23 direct-sender check, authenticated providers
   first, and installs marked unsigned (`assemble_chunks` already writes `publisher_did: ""`,
   `distribution.rs:256`). The real fix is D-76 D2 in PAS-P5 (sign the index digest / HashSeq
   root).
2. **Blob ALPN serves any hash.** `BlobsProtocol::new(&store, None)` (`docs_engine.rs:120`) has no
   request filter, so anyone who learns a hash can fetch it. This includes replica row blobs.
   It is pre-existing exposure, not introduced here, but tileset ads now broadcast root hashes.
   Recommend the security review pass check it; iroh-blobs supports a request-event filter.
3. **sync_thread.rs merge pressure.** CDL W4/SPEC-10 also edit the select loop. Keep the tileset
   code in `tileset_transfer.rs` / `blob_transport.rs` and leave only thin adapters in
   `sync_thread.rs`.
4. **Lazy-tile async adds latency at first sight** (a chunk appears a frame or more later). That
   is acceptable and the price of NFR-2. Validate in-app (USER_GATED).
5. **Stamp-group residency granularity.** A long track near the camera admits the whole group
   (capped). If in-app shows waste, per-sample residency is the follow-up (§6).
6. **`ChunkReceived` wire change** breaks any out-of-tree consumer. There are none (RUNBOOK §3
   2026-10-12 "no consumers, breaking is cheap").
7. **Determinism depends on zip writer defaults** (no `time` feature on `zip` 2,
   `fe-format/Cargo.toml:14`). If someone enables `time`, the S1 determinism test goes red, which
   is intended.
8. **Process leaks in PAS-P1V.** The script must kill only the PIDs it spawned (CLAUDE.md
   cleanup rules).
9. **Real-network test flakiness:** the harness loopback dials. `FE_SYNC_RELAY=disabled` keeps it
   hermetic (RUNBOOK §5).

## 6. Deferral audit

| Item | Left out? | Justification class |
|---|---|---|
| `fractalengine-relay` (headless) seeding tilesets via `PublishTileset` | **No — moved into S4 step 5 (critic)** | The claimed NO_GO is canonical-log-only by the record: ledger row CDL-NET "Canonical-log network rollout, relay seeding, inbound P2P" (`doneness-ledger.md:263`). No ambiguity, so no user decision is needed. |
| PartialHexonFetch HTTP impl, registry chunk-by-seq/meta routes, D-76 signed digests, `HexonArchive::import` split callers | Not in this campaign step | **Not a deferral**: PAS-P5 is the next ratified campaign step (RUNBOOK §7.3). This plan builds its primitives (`BlobTransport`, `HexonArchiveIndex`, `TilesetBundleIndex`, `ChunkIndex.blake3`) so P5 is wiring only. |
| Byte-range (bao) reads in `BlobTransport` / D-73 A3 layout | Not added | **Not a deferral of ratified scope**: D-73 locked A4. A3 is a later layout option; a range method with no consumer would be dead code (ui_ux §6 "no faked" spirit). |
| D-73 "refine to A2 per-tile for hot zoom levels" | Not built | Conditional in the ratified text ("only if over-fetch proves painful"); it needs in-app evidence the corpus can't give (evidence §3: today's tilesets fit in 1-8 bundles). |
| Per-sample stamp residency | Not built (group-level instead) | Design choice recorded as an assumption (§8); revisit trigger: in-app waste. |
| 3-tier LOD from `render_distance_lod_20260407`; data-horizon layers from `relay_data_horizon_20260407` | Not built | Out of D-74 scope per evidence §4 (orthogonal; "not scheduled" in the ratified digest). |
| "Share my working set" hash-list export | Not built | Ratification text calls it "an optional export, never a required primitive". No ask. |
| In-app verification (dense-petal live run, NFR-1/NFR-2 frame-time checkpoint) | Automated proxies only | **Hard external dependency**: USER_GATED in-app runs (RUNBOOK §4 "every in-app verification" is user-owned). S5/S8 add the strongest headless proxies. |
| Relay-assisted connection via self-hosted iroh-relay 1.3 | Only the Custom-mode log/health capture in PAS-P1V | Owned by RUNBOOK §7.5 (D-23 host undecided, R6). |
| Tile byte-budget as a user knob | Constant | No ask; adding a knob without a request is speculative. Reversal: one `AppSettings` field. |
| `SetPetalTerrain` not replicating / not op-logged | **No — moved into S6 step 8 (critic)** | "Outside these rows" is not a justification class. S6's full-row replication would already ship `terrain` unvalidated, so the gap is created by this plan and has to close here. |

## 7. Decision 2.6 (made here, recorded by S9)

**FR-4 subsumes runtime_instance_guardrails FR-6 wholesale.** That track is already ARCHIVED
(2026-07-24) with the note "FR-6 render horizon delivered CROSS-TRACK by p2p_asset_streaming
FR-4 … dense-petal live-run acceptance bar transfers with it"
(`conductor/tracks/_archive/runtime_instance_guardrails_20260717/metadata.json:11`). Its
crash-guardrail half (FR-1..FR-5: caps, sanitize, watchdog, root-cause fix `fa55be4`) is done.
Therefore:
- (a) no work is reopened there;
- (b) its FR-6 acceptance ("dense-petal camera-locality") is carried here as S8's
  moving-camera system test plus a USER_GATED live run;
- (c) the ledger row FEUI-RESIDENCY-LEDGER is already merged into PAS-P4
  (`doneness-ledger.md:20`).

Reversal cost: none. It records an existing state.

## 8. User decisions and assumptions

**Genuine user decisions: none remain.**

- ~~Q1 (relay seeding)~~ **Resolved by the critic from the record.** The NO-GO is ledger row
  CDL-NET (canonical-log). Relay tileset seeding is now S4 step 5. Reversal (cheap): drop the
  startup call.

**Assumptions (highest reversal cost first):**

1. Bundle wire format = `HashSeq [index_json, bundle_0 … bundle_n]`, with each bundle a
   self-contained `.hexon` chunk archive and a 1 MiB default.
   - Reversal: format change before any consumer exists, so cheap now, expensive after
     published swarms.
2. Tile residency is async from the start (pending ≠ failed) rather than measuring sync reads
   first.
   - Reversal: keep the cache, call the loader inline (cheap).
3. Ledger placement is after `apply_db_results`, before every spawner ("first" = first ahead of
   spawners).
   - Reversal: a one-line chain move, accepting a one-frame lag.
4. Stamps are resident per track group, with nearest-first truncation.
   - Reversal: per-sample residency (moderate).
5. `petal.render_distance` writes need Editor+, and NONE means inherit. A session override beats
   the petal value, and the petal value beats the global.
   - Reversal: the role threshold is one constant.
6. P2P-assembled tilesets install as unsigned until PAS-P5.
   - Reversal: P5 adds verification at install.
7. The download dir is `<FE_P2P_DIR>/downloads/<id>/`, seeding bundles live in
   `<hexon store>/bundles/<id>/`, and tags are `fe/tileset/<id>` / `fe/download/<id>`.
   - Reversal: path constants.
8. Tile byte cache = 256 MiB constant. Camera-forward weight w = 0.5. Pressure thresholds are
   1.0 / 1.5 / 1.75 GiB.
   - Reversal: constants.
9. PAS-P1V Custom mode points at `http://127.0.0.1:3340`. If no local iroh-relay container is up,
   the run still proves mode logging and a Degraded/Unknown health transition.
   - Reversal: rerun when the D-23 host exists.

## 9. Cross-cuts

- **PAS-P5:**
  - reuse `fe_format::HexonArchiveIndex` (S5) for the `HexonArchive::import` header/member
    split, so it isn't built twice;
  - the P2P impl of `PartialHexonFetch` = `TilesetTransfer` over `BlobTransport` (S2);
  - the HTTP chunk-by-seq route must serve **the same bundle bytes** (same blake3) so one
    conformance suite compares hashes;
  - D-76 D2 signs `TilesetBundleIndex` (or the HashSeq root), and `ChunkIndex.blake3` (S1) is
    the per-chunk slot.
- **CDL-W4 / CDL-SPEC10:**
  - both edit `fe-sync/src/sync_thread.rs`. Sequencing avoids build overlap, but they must
    rebase on S3's adapters, and the tileset logic stays in its own modules;
  - tileset blob fetch is not canonical-log traffic and doesn't touch the NO-GO
    inbound-canonical / `/ws/canonical` items. The blobs ALPN was already routed.
- **Role propagation:** S6's replicated `petal.render_distance` is admitted by A3 Editor+ resolved
  from local tables. Until roles replicate, only the owner (`created_by`) and locally-granted
  editors pass on peers. After role propagation lands, add a two-peer test where a non-owner
  editor's change applies. Tileset ads could later be membership-gated by replicated roles.
- **2026-10-13 decisions:**
  - D-5/D-17 (generic asset + `hexon_p2p_bucket` on one asset model): `BlobTransport` is the
    natural transport for bucket and device-payload assets; HPB-SPEC FR-5 overlaps PAS-P3
    (`doneness-ledger.md:274`);
  - D-38 snap-to-terrain (MSA-P4) reads `TerrainHeightField`, which under lazy tiles is
    populated only for resident chunks. Snaps outside the ring fall back to Y=0 unless the
    snapping path requests residency;
  - R6 relay host undecided: PAS-P1V uses a local container;
  - D-54 f64 egress positions: not affected (residency math stays in f32 world units).
- **2026-10-12 decisions:**
  - the `petal.render_distance` column is folded in as S6, which also closes PAS-P4B's open
    question;
  - flag day / no 0.35 window: no 0.35 compatibility for tileset ads or HashSeq layout;
  - self-hosted relay default: PAS-P1V Custom mode;
  - "a writer of a shared JSON column must satisfy the strictest reader" is applied to inbound
    render_distance.
- **Parity gap found:** `SetPetalTerrain` (`fe-database/src/handlers/petal_terrain.rs`) neither
  replicates nor op-logs. **(Critic: closed in S6 step 8; no ledger row needed.)**

## 10. Critic amendments (2026-10-13) — what changed and who owns what

The build order is fixed: PAS-P3P4 → PAS-P5 → CDL-W4 → CDL-SPEC10 → role propagation. Where two
plans touch the same thing, **this plan (earlier) owns it** and the later plans note the
dependency.

| # | Change | Class |
|---|---|---|
| C1 | `hexon_id` path guard moved from PAS-P5 S6 into **S1 step 6**. S4 makes peer-assembled installs reachable here, a phase before P5. | ordering bug + security |
| C2 | `tileset_id` path guard and the provider-dial rule added to **S2**, with a test. | security, missing test |
| C3 | The `BlobTransport` trait and in-memory impl move to **fe-runtime** (iroh impl stays in fe-sync). This satisfies PAS-P5 contract C-P3 #1, so P5 never needs an fe-sync→fe-hexon edge. **Canonical names: P5 adapts to these.** | cross-plan collision |
| C4 | `TilesetBundleIndex::bundle_for_tile` (pure, fe-format) is the only tile→bundle lookup. PAS-P5 S2 must not add `ChunkTable::chunk_for_tile`. | cross-plan collision |
| C5 | `HexonArchiveIndex` (S5, `fe-format/src/archive_index.rs`) is **the** lazy archive reader. PAS-P5 S2 extends it (header doc, verified reads, `chunk(seq)`) instead of adding `reader.rs`. | cross-plan collision |
| C6 | Relay tileset seeding moved in (S4 step 5). Q1 resolved from the record (CDL-NET). | unjustified deferral |
| C7 | `SetPetalTerrain` replication, the inbound `terrain` shape check and op-log parity moved into **S6 step 8**. | unjustified deferral + security |
| C8 | S6 now owns `fe-runtime/src/app.rs`, `fe-test-harness/src/peer.rs` and `mapping_inventory.rs`. The harness match is exhaustive and correlated replies route through `app.rs`. Every later plan adding `DbCommand` variants must do the same (CDL-SPEC10 A3 amended). | missing files |
| C9 | **S8b security review-and-fix** added. Its blob-ALPN filter must not break `FetchBlob` (CDL-SPEC10 A6). | missing review |
| C10 | HashSeq child 0 is the opaque `TilesetBundleIndex` JSON **for this campaign step only**. PAS-P5 replaces it with the signed `HeaderDoc` and adds a header-confirm phase (P5 S7a). `fe-sync` stays format-agnostic in both. | cross-plan contract |
| C11 | Later-phase files touched by these changes: role propagation RP-3 flips this plan's `Denied` expectations to `Deferred`, and RP-4 adds referential confinement for petal rows. Both are owned there. | later-phase dependency |
