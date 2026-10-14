# fe-runtime/src — module rationale

## §scene-change

`messages.rs::SceneChange` is the process-wide scene-delta contract. Every
node-scoped variant carries its owning `petal_id`; a WebSocket receiver must
use `SceneChange::petal_id()` to scope both deltas and transform rollbacks.
Producers must not emit a node-scoped scene change when they cannot determine
that petal. This makes deleted-node events safe to filter without a post-delete
database lookup.

## §property-bridge (`bim_primitives_on_paths_20260712`, C5)

`shared_node.rs::PropertyValue` is the Tauri↔Bevy bridge's authoritative
property shape (`String`/`Number`/`Boolean`/`Array`/`Json`). `fe_sdk::property::PropertyValue`
(`fe-sdk/src/property.rs`) is the extension-facing mirror with the same four
scalar shapes plus its own `Json(serde_json::Value)` catch-all. `From`/`Into`
impls at the bottom of `shared_node.rs` keep the two convertible without a
third enum: `fe_sdk::PropertyValue::Array` doesn't exist on the SDK side, so
the `fe-runtime → fe-sdk` direction folds `PropertyValue::Array` into `Json`
losslessly via `serde_json::to_value`. `fe-runtime` depends on `fe-sdk` (not
the reverse) — `fe-sdk` must stay serde-only/engine-decoupled per its own
`AGENTS.md`.

Primitive descriptors (`fe_sdk::primitive::PrimitiveDescriptor`) ride on this
bridge as `PropertyValue::Json(descriptor.to_json())` — see
`fe-ui/src/verse_manager/AGENTS.md` §primitives for the render-side contract.

**Untagged-deser caveat:** `PropertyValue`'s variant order (`String`, `Number`,
`Boolean`, `Array`, `Json`) means untagged deserialization tries earlier
variants first, so a `Json` payload shaped like a scalar or array (e.g.
`Json("hi")`, `Json([1,2,3])`) deserializes back as `String`/`Array`, not
`Json` — round-trip is lossless only for JSON *objects*. Primitive descriptors
are always objects, so this doesn't affect them. Do not "fix" this by
reordering variants or making the enum tagged without confirming nothing
depends on the current untagged wire shape (Tauri↔Bevy bridge).

## §blob-store (P2P Mycelium Phase A)

`blob_store.rs` defines the `BlobStore` trait, decoupling asset-byte storage
from the database layer. It lives in `fe-runtime` (no crate dependencies) so
that both `fe-database` and `fe-sync` can depend on it without creating a
cycle:

```text
fe-runtime (trait)  <---  fe-database (uses handle)
       ^
       |
    fe-sync (FsBlobStore impl)  --->  fe-database (existing dep)
```

Hashes are raw BLAKE3 digests (`[u8; 32]`); hex encoding is provided for DB
rows (`content_hash` column) and paths (`blob://{hex}.glb`).

**Canonical on-disk layout (RT-FSBLOB, 2026-10-11).** Every filesystem blob
store lays a blob out as `{root}/{hex[0..2]}/{hex}` (`blob_relative_path` /
`canonical_blob_path`), and the default root is
`{data_local_dir}/fractalengine/{DEFAULT_BLOB_DIR_NAME}` (`blobs`). Both
`fe_sync::FsBlobStore` (the runtime store) and `fe_hexon::registry::FsBlobStore`
(hexon installs, material/texture loading) build paths only through these
helpers, so a hexon-installed blob is servable by the runtime store and a
runtime-ingested blob loads in the material loader
(`fe-test-harness/tests/blob_store_cross_read.rs`). Content addressing is the
cross-read contract: the hexon store refuses a hash that is not 64 hex chars
or does not match `blake3(data)`, and both write atomically (tmp + rename).

`MockBlobStore` must produce the same digest as `FsBlobStore` so tests can
cross-check hashes — that is why `fe-runtime` takes a direct `blake3`
dependency despite the trait itself needing none. If that dep weight ever
becomes an issue, move the mock into `fe-sync` instead.

## §ktx2-guard (RUSTSEC-2026-0344 interim mitigation, 2026-10-13)

`ktx2_guard.rs` refuses glTF assets that would reach bevy's KTX2 decoder,
because KTX2 Zstandard supercompression is decoded by `ruzstd` 0.8.3, which
has a decompression-bomb advisory (an untrusted GLB can OOM the GUI). Bevy
0.18 through 0.20 pins `ruzstd` ^0.8, so no dependency bump fixes it yet.
The policy is to refuse, not strip: a stripped texture would silently render
wrong, and a refusal names the advisory.

**What counts as KTX2.** An image whose `mimeType` is `image/ktx2` (compared
case-insensitively, as `bevy_image::ImageFormat::from_mime_type` lowercases).
The `KHR_texture_basisu` extension, in `extensionsUsed`/`extensionsRequired`
or on a texture. An image `uri` ending `.ktx2` or `.basis`, or a
`data:image/ktx2` URI. Image bytes that start with the KTX2 identifier
`«KTX 20»\r\n\x1A\n` (`AB 4B 54 58 20 32 30 BB 0D 0A 1A 0A`), either in a bufferView of the GLB's BIN chunk or in
a base64 data URI. That last check is defence in depth: bevy picks the
decoder from the mime type and never sniffs bytes.

**Judge what bevy sees, not what the file says** (adversarial review
2026-10-13). bevy_gltf percent-decodes every image URI before it tests for
`data:` or resolves a path, so the guard decodes first too:
`data:image%2Fktx2%3Bbase64,...` is refused. A URI (image or buffer) that is
not UTF-8 once decoded is refused as `Malformed`, because bevy `.unwrap()`s
that decode, and under `panic = "abort"` a peer GLB could kill the GUI. The
`gltf` crate also picks GLB vs JSON by content (the `glTF` magic), not by
extension, so `check_asset_bytes` runs the JSON check on any bytes that look
like a JSON object, whatever the path's extension. A JSON glTF stored as
`blob://<hash>.glb` is guarded like a `.gltf`.

**Bounded parsing.** The 12-byte header must say `glTF` v2 with a total
length equal to the byte count. Chunk lengths are checked against the bytes
actually present before anything is sliced, and the JSON is capped at
`MAX_GLTF_JSON_LEN` (64 MiB) before parsing. The JSON is deserialized into
narrow structs (`serde::de::IgnoredAny` for the rest), so nothing is
allocated from a declared length. A GLB whose first chunk is not JSON passes:
bevy's `gltf` parser refuses it (`GlbError::ChunkType`), so it can never
reach a decoder, and existing header-only fixtures keep passing
`fe_api::upload::validate_glb`.

**Where it runs.**
1. `bevy_blob_reader::BlobAssetReader::read`: the choke point. Every
   `blob://` load passes here, whatever wrote the blob (REST/MCP upload,
   peer sync, hexon install, local import, path-asset stamp). A refusal is
   an `AssetReaderError::Io(InvalidData)` plus a `warn!`. External images
   and buffers that a glTF references are read through this same reader, so
   a `.ktx2` sibling blob is caught by extension or by its magic bytes.
2. `fe_api::upload::validate_glb` (REST multipart and MCP `upload_asset`):
   HTTP 422 (`UploadError::Ktx2Texture`), so the refused bytes are never
   stored.
3. `fe_database::handlers::crud::import_gltf_handler` (local import): a
   clear error instead of a model that silently fails to load. Local imports
   are guarded too, because the blob store is the replicated surface: an
   imported blob is shared with peers.

Not guarded: Bevy's default `file://` source (the app's own `assets/`
folder holds trusted, shipped files). The other image decoders in the
workspace use the `image` crate (`load_from_memory`), which has no KTX2
support, so they cannot reach `ruzstd`.

**Removing it.** When a bevy release requires `ruzstd` >= 0.9.1, bump bevy,
delete the guard and its three call sites, and drop the RUSTSEC-2026-0344
ignore in `deny.toml` together with the SECURITY.md entry.

## §timeseries (M2/F6)

`timeseries.rs` is the canonical per-verse timeseries-fabric vocabulary:
`TimeseriesMode` (mirror/sharded/balanced) and `VerseTimeseriesSettings`
(mode + `replication_factor` R + `bucket_width_ms`, epoch-aligned). It lives
here for the same reason `RoleLevel` lives in fe-policy: every layer needs it
(fe-database persists and re-emits the `ts_*` verse-row columns, fe-sync's
fabric/placement/retention dispatch on it, fe-ui renders it) and fe-runtime
is the lowest common dependency — one definition, no drift, no cycle.

- `mirror` is the `#[default]` **and** the pre-F6 behavior exactly (every
  peer holds every shard), so a fresh verse and an F5-era peer are
  byte-compatible with the F6 fabric.
- `sanitized(mode, R, bucket)` validates raw settings input and returns a
  human-readable reason on bad input — the DB handler rejects with it; row
  parsing never calls it (see below).
- `from_verse_row` parses the `ts_*` columns with **clamping, never
  rejection**: missing or invalid fields fall back to the mirror defaults,
  because an inbound verse manifest must never fail to converge a fabric
  over a settings typo (a manifest is data, not a form submission). This is
  the same asymmetric-validation split fe-sync's `ShardLedgerEntry::from_row`
  uses. The pre-F6 row (no `ts_*` at all) parses as pure defaults.
- `serde` shape: the enum serializes lowercase (`"mirror"`|`"sharded"`|
  `"balanced"`, `#[serde(rename_all = "lowercase")]`) — the verse-row column
  value, the shard-ledger row value, and the dump-JSON value are all the
  same label, matched by `as_str`/`parse` (not serde) on the DB-column paths
  where the value rides a plain `String`.
- `DEFAULT_BUCKET_WIDTH_MS` = 1 day. Bucket indices are
  `recorded_at_ms.div_euclid(bucket_width_ms)` (fe-sync `ShardId`), so the
  default aligns buckets to day boundaries.

## §api-reply-correlation

`app.rs::PendingApiRequests` holds the API threads' pending `DbResult` waiters.
`DbResult` mostly carries **no correlation id** (exceptions below), and results are not only replies to
API commands: the DB thread also fans unsolicited work into the same channel
(the relay startup scan's `HierarchyLoaded`, GUI-initiated hierarchy reloads).
Pairing the next result with the oldest waiter (plain FIFO) therefore
cross-pairs whenever an unsolicited result races a pending request — the
relay's production `/ready` ping rides exactly that window (a `HierarchyLoaded`
would satisfy the ping while the ping's real `Pong` went to the hierarchy
waiter, which then either mishandled it or dropped it).

The queue is therefore keyed by reply **family** (`ReplyKind`), not by arrival
order:

- `enqueue_for(cmd, tx)` derives the family with `reply_kind_of_command`;
  `reply_kind_of_result` derives it from a result; `try_deliver` only hands a
  result to the oldest waiter of that one family.
- `DbResult::Error` is a wildcard: it is delivered to the oldest waiter of any
  family (`deliver_to_oldest`), because a failure has no family-specific shape.
- A result whose family has no waiter is dropped, never delivered to some
  other family.
- **Correlated families (DEC-C13, 2026-10-09).** Within one family the queue
  is FIFO and `deliver_to` SKIPS closed entries (a timed-out caller dropped
  its receiver). That cross-delivered the F24 IoT ingest fallback: caller A
  times out at 10s, B enqueues, A's late `IotReadingsInserted` pops A's
  closed entry, skips it, and lands on B (B's own reply then finds no
  waiter; with more callers queued the whole family shifts by one). The fix
  is per-family correlation, not a routing-wide change: `InsertIotReadings`
  carries a `correlation_id` the DB thread echoes on `IotReadingsInserted`/
  `IotReadingsRejected` (the `CreateNode` precedent); `PendingEntry` stores
  it (`correlation_of_command`), and `deliver_correlated` hands a correlated
  reply ONLY to its own entry — even a CLOSED one, which consumes and drops
  it (requeue is impossible: a oneshot cannot be re-armed, and consuming is
  what keeps the queue aligned). An id with no entry is not delivered.
  Uncorrelated replies (`None`, every other family) keep the old semantics.
  F13 adds two correlated families: `CreateAsset` → `AssetCreated` and
  `CreateNodeWithAsset` → `GltfImported` (`ReplyKind::PlacedAsset`, mapped
  ONLY when the echoed id is `Some` — a GUI `ImportGltf` result keeps its
  unmapped legacy routing and can never land on an API waiter).
  The fe-api fallback additionally checks the echoed id + `petal_id` and
  answers 502 on a mismatch rather than leak another caller's result.
  **M4 fix B2 (2026-10-09):** `RawQuery` → `QueryResult` is correlated too.
  The API channel fallback (`query_guard::run_guarded_query_via_state`, gis
  `run_select`) sends a fresh ULID; the DB thread echoes it on `QueryResult`
  and answers a correlated failure with `QueryFailed { error,
  correlation_id }` (GUI queries send `None` and keep failing with `Error`,
  so fe-ui's error routing is untouched). An UNCORRELATED reply now reaches
  only an uncorrelated entry (`deliver_uncorrelated`) — before, a GUI
  Query-tab `QueryResult` was handed to the oldest API waiter (a public
  share redeemer could receive the desktop user's ad-hoc rows). fe-ui's
  dispatcher consumes only `correlation_id: None` query results; correlated
  ones flow solely to `try_deliver`.
  **Terrain sets (2026-10-10).** `SetPetalTerrain` carries a
  `correlation_id` (REST PUT/DELETE + MCP `set_petal_terrain` send a fresh
  ULID; every GUI site sends `None`). The DB thread
  (`fe_database::handlers::petal_terrain::set_petal_terrain_reply`) echoes
  it on `PetalTerrainLoaded` with the terrain READ BACK from the written
  row, and answers a correlated failure with `PetalTerrainSetFailed { petal_id,
  error, correlation_id }` (GUI failures stay `Error`). Family
  `ReplyKind::PetalTerrainSet` is mapped ONLY for a `Some` echo (PlacedAsset
  precedent): an uncorrelated `PetalTerrainLoaded` (GUI set echo, any Get)
  stays in the `PetalTerrain` Get family and can never satisfy an API set.
  **Bevy fan-out decision:** a `DbResult` is never "taken" by the API path —
  the relay's `deliver_pending_api_results` and fe-ui's `apply_db_results`
  both read the Bevy `Messages<DbResult>` stream and only *clone* into
  `try_deliver`, and `fractalengine/src/terrain_bridge.rs` reads the same
  stream. So an API-initiated set reaches its waiter AND every host
  consumer: fe-ui's `PetalTerrainLoaded` arm and `bridge_petal_terrain`
  match `{ petal_id, terrain, .. }` (correlated or not) and apply it — but
  ONLY for the host's active petal (both check `NavigationManager::
  active_petal_id`; the terrain runtime holds one petal, and an API write may
  target any petal the token covers, so an unfiltered bridge would replace
  the viewport's terrain with petal B's). That keeps the desktop's map
  picker / live terrain consistent with what the API just wrote. Filtering correlated echoes out of those arms (the QueryResult
  pattern) would be wrong here: a query result is private to its caller, a
  terrain write is shared scene state. `PetalTerrainSetFailed` has no Bevy
  consumer (the API caller gets its 502; the host state did not change).
  **Terrain reads (2026-10-11, FEAPI-GETTERRAIN-CORR).** `GetPetalTerrain`
  carries a `correlation_id` too (the fe-api tile-auth fallback
  `terrain.rs::assigned_tilesets` sends a ULID; fe-ui's petal-switch load
  sends `None`). A correlated read is answered by a DISTINCT variant,
  `PetalTerrainFetched { petal_id, terrain, correlation_id }`
  (`handlers::petal_terrain::get_petal_terrain_reply`), in its own family
  `ReplyKind::PetalTerrainFetch`. Unlike a set, a read is private to its
  caller (the QueryResult precedent): fe-ui's `PetalTerrainLoaded` arm and
  `terrain_bridge` never see it, so an API tile request no longer re-drives
  the desktop's terrain runtime, and a GUI load / GUI set echo for the same
  petal (family-identical before) can no longer satisfy the API waiter.
- **Typed correlated failure — `RequestFailed` (2026-10-11, RT-CORR-EDGE).**
  `DbResult::RequestFailed { error, correlation_id }` is the generic failure
  for correlated commands whose family has no family-specific failure
  variant: `GetPetalTerrain`, `CreateAsset`, `CreateNodeWithAsset` (DB side:
  `handlers::correlated_failure`, which returns the legacy `Error` for
  `None`). `try_deliver` routes it family-blind by id
  (`deliver_by_correlation_id`: scans every queue for that id; a closed entry
  consumes it, an unknown id is dropped). Before this, a DB-layer upload /
  placement failure answered the correlated waiter with `Error`, which
  `deliver_to_oldest` never hands to a correlated waiter (DEC-C19 N2), so the
  API caller hung for the full 5 s `db_round_trip` timeout. The GUI never
  sees `RequestFailed`; each API caller maps it to its error status. Later
  correlated families reuse it instead of minting a new failure variant.
- **Elevated queries (2026-10-11, FEAPI-ELEV-WRITES).** `ElevatedQuery`
  always carries a `correlation_id: String` (only the API sends it — fe-api
  `rest.rs::execute_elevated_query`, a fresh ULID) and is answered by
  `ElevatedQueryResult { data, changed_rows, correlation_id }` in its own
  family `ReplyKind::ElevatedQuery`, or by `RequestFailed` (the precedent
  above — no new failure variant). The API awaits 7 s; a later reply is
  consumed by its closed entry. fe-ui's dispatcher reads
  `ElevatedQueryResult` only to reload its tree when `changed_rows > 0`,
  then still `try_deliver`s it.
- **Visibility groups (2026-10-11, VIS-PH4/6).** `SetPetalVisibilityGroups`,
  `GetPetalVisibilityGroups` and `SetScopeHidden` carry `correlation_id` and
  are correlated families (`ReplyKind::VisibilityGroups` — Set and Get both
  answer `PetalVisibilityGroupsLoaded` — and `ReplyKind::ScopeHidden`);
  failures are `RequestFailed`. The desktop correlates its own writes too
  (`fe-ui:groups:{petal}` / `fe-ui:scope-eye:{id}`), so its echoes and
  refusals have no API entry and are dropped by `try_deliver`
  (`app.rs::tests::visibility_group_replies_reach_only_their_own_api_waiter`)
  while fe-ui still reads them off the Bevy stream. Like a terrain set, a
  registry write is shared scene state: fe-ui applies ANY
  `PetalVisibilityGroupsLoaded` for its active petal, API-initiated included.
  `SetNodeView` (VIS-PH3, `ReplyKind::NodeViewSet`) is correlated the same
  way on BOTH sides (`correlation_of_command` + `correlation_of_result`): a
  correlated command filed without its id would never match its own echo.
- **Pop-and-drop (2026-10-11, RT-CORR-EDGE).** Uncorrelated delivery
  (`deliver_to`, `deliver_uncorrelated`, and the `Error` wildcard which funnels
  into `deliver_uncorrelated`) no longer SKIPS a closed entry: the oldest
  eligible entry consumes the result, and a closed one (its caller timed out)
  drops it. The DB thread is serial and answers each awaited command exactly
  once, so within an API-only family the k-th reply belongs to the k-th
  entry — skipping shifted every later caller by one. The invariant this
  needs is "every enqueued entry is answered exactly once", so
  `drain_api_commands` calls `PendingApiRequests::cancel(id)` (the id
  `enqueue_for` returns) when the DB send fails — an entry for a command the
  DB never received would otherwise swallow the family's next reply.
  **Residual:** GUI-shared uncorrelated families (`NodeCreated`,
  `NodeDeleted`, `NodePropertySet`, `VerseCreated`, `Hierarchy`) still pair
  FIFO in the GUI binary — a GUI-initiated reply of the same family can land
  on (or be consumed by) an API entry. Remedy when a caller needs it:
  correlate that family (the IoT/RawQuery/terrain pattern).
- **Known sharp edge (NARROWED 2026-10-09 — DEC-C19 N2, was DEC-C13).**
  `Error` stays family-blind, but `deliver_to_oldest` now delivers only to
  the oldest UNCORRELATED waiter (correlated families always receive typed
  replies — `QueryFailed`/`PetalTerrainSetFailed`/`IotReadingsRejected`/
  `RequestFailed` — so a bare `Error` can never reach a correlated waiter;
  if only correlated waiters are pending the error is dropped). The
  skip-closed shifting edge among uncorrelated waiters is CLOSED by
  pop-and-drop (2026-10-11); what remains is the GUI-shared-family residual
  above.

**Maintenance rule:** every `DbCommand` that expects a reply must be mapped in
`reply_kind_of_command` and its reply in `reply_kind_of_result` —
`reply_kind_mapping_is_consistent_for_every_awaited_command` fails otherwise.
Fire-and-forget commands (`TransformPersist`, telemetry writes) must send
directly and must NOT enqueue a waiter: registering a waiter for a command the
DB thread never answers leaves a dangling entry that later results can
mis-deliver into.

## §asset-gc (FEAPI-BLOB-GC, 2026-10-11)

`asset_gc.rs` — `setup_core_systems` (so both binaries) inserts
`AssetGcSchedule::from_env()` (env read once: `FE_ASSET_GC_INTERVAL_SECS`,
default 3600, `0` disables; `FE_ASSET_GC_GRACE_SECS`, default 86 400) and runs
`schedule_asset_gc`: a `Local<Timer>` that every interval `try_send`s
`DbCommand::GcUnreferencedAssets { grace_secs }` on the Bevy-side
`DbCommandSender` (never blocking the main thread; a full/closed channel is a
`warn!`). It is **fire-and-forget** under the maintenance rule above: no waiter
is registered and the command is deliberately absent from
`reply_kind_of_command`; its success reply `AssetsGarbageCollected` maps to no
family in `reply_kind_of_result`, so it can never satisfy an API waiter and
just reaches Bevy messages. The DB side and the GC rules: fe-database
src/AGENTS.md §handlers (`asset_gc`), fe-api AGENTS.md §asset-ingest.

## §packaged-layout (release archives, 2026-10-13)

`app.rs::packaged_root()` returns the executable's folder when an `assets/`
directory sits beside the binary. That is the layout the release archives ship
(`.github/workflows/build-artifacts.yml` and `release.yml` package
`fractalengine/assets/samples/` as `assets/samples/` next to the binary). Two
things key off it:

- `file_asset_root()` gives Bevy's default file source an ABSOLUTE root:
  `<exe dir>/assets` when packaged, else `<cwd>/fractalengine/assets` (a
  workspace checkout run from the repo root). An absolute path bypasses Bevy's
  own base (`BEVY_ASSET_ROOT`, then the *runtime* `CARGO_MANIFEST_DIR` that
  `cargo run` sets, then the exe folder), which would otherwise resolve to
  `fractalengine/fractalengine/assets` under `cargo run`.
- The GUI binary's `main` calls `std::env::set_current_dir(packaged_root)`
  first thing, so every CWD-relative path (`data/fractalengine.db`,
  `data/p2p`, `data/panic.log`, the legacy `fractalengine/assets/imported`
  backfill source) lands beside the binary no matter where it was launched
  from. Without this, a double-click and a terminal launch from `~` would open
  two different databases.

A dev build is never "packaged": `target/{debug,release}/assets` does not
exist, so `cargo run` behaves exactly as before. The relay does not use this:
it is a server and keeps `FE_DB_PATH` / `FE_P2P_DIR` (CWD-relative defaults).
Per-user state outside `data/` is unaffected: the blob store
(`<data_local_dir>/fractalengine/blobs`), settings
(`<config_dir>/fractalengine/settings.ron`) and the OS keychain identity.

## §sim-control (F9/A20)

`sim_control.rs` — the API→fe-sim bridge seam (`SimControlCommand` in a
`SimControlCall { cmd, reply_tx }`, typed `SimControlError`). It lives here
for the `distributed_query.rs` reason: fe-api needs the vocabulary and must
never depend on fe-sim, so wire payloads stay `serde_json::Value` and the
bridge parses `ScenarioScript`/`ScriptedEvent` on its side. Reply is a
crossbeam sender (the bridge thread has no async runtime). Surface rules:
`fe-api/AGENTS.md` §sim-control; driver/bridge: `fe-sim/src/AGENTS.md`
§session.
