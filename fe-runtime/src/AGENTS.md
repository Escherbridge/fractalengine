# fe-runtime/src — module rationale

## §qa-controls

`qa_debug::DialogAnswerBatch` and `WindowAction` extend the existing scoped QA bridge. Dialog batches contain
1–32 answers, each with exactly a path or cancellation, and the host queue is capped at 64. Window size is
physical pixels within 640–8192 by 480–8192; app UI scale is finite within 0.75–2.0. The GUI routes requests
through the same shell executor as caption actions and returns busy rather than replacing a queued action.
`build_app_with_confirmed_close` disables automatic OS close only for the editor, whose UI supplies confirmation;
the original `build_app`/`build_app_with_window` helpers retain their default close behavior.

`messages::FileReadLease` owns standard file/ancestry handles across a deferred import, without GUI type coupling.
`DbCommand::ImportGltf` carries an optional lease; the DB worker drops it after the real import handler returns.
Normal/API/harness constructors use `None`. GPX/Hexon UI queues carry their own `DialogPath` until parsing finishes.

## §scene-change

`messages.rs::SceneChange` is the process-wide scene-delta contract. Every
node-scoped variant carries its owning `petal_id`; a WebSocket receiver must
use `SceneChange::petal_id()` to scope both deltas and transform rollbacks.
Producers must not emit a node-scoped scene change when they cannot determine
that petal. This makes deleted-node events safe to filter without a post-delete
database lookup.

## §deferred-admission (role_propagation_20261013 RP-2/RP-3, 2026-10-24)

`ReplicatedRowOutcome::Deferred(reason: DeferralReason)` signals a row that
failed only because a prerequisite has not yet converged (plan §2.4). Three
wait reasons exist:

- **`Manifest`**: The verse manifest row has not landed locally yet. All role
  statements pre-manifest are deferred on Manifest; once the manifest lands
  with an owner, deferred statements are re-validated fully (the owner-dependent
  checks run for the first time). Content rows with no owner known also defer.
- **`Parent { table, id }`**: A referenced parent row (fractal, petal, or node)
  is unknown locally. Deferral placement is per-table: a `device_format` without
  its parent `device_payload` defers, a `device_payload` without its parent
  petal defers. Enrolments defer on missing petal; content rows defer on
  missing content parents. A scope that names a fractal/petal where the
  hierarchy does not put it is denied permanently (it can never resolve).
- **`AuthorRole(did)`**: The author (or the grantor of a role statement) has
  not yet reached the authority threshold the row requires. Content rows wait
  until the author's role rises; role statements wait in a durable store
  (`role_statement_pending`) and are re-evaluated every time any author's
  authority changes (RP-3 partition logic).

**Content row retry:** A deferred content row is held in the DB thread's
in-memory `InboundAdmissions` queue (4096 per verse, 512 per author, drop-oldest
on overflow). Retries run as a worklist after each apply: manifest apply retries
`Manifest` waiters; a parent apply retries its `Parent` waiters; an authority
change retries all `AuthorRole` waiters for that DID. A restart or overflow
loses entries; the next open's snapshot replay re-feeds them.

**Role statement retry (durable):** Role statements are never deferred in the
in-memory queue. An inadmissible statement lands in `role_statement_pending`,
which survives restarts and has a per-grantor budget (1024 keys per verse,
refuse-new on overflow). Every partition (after a write, manifest, or authority
change) promotes held statements that are now admissible and demotes admitted
statements that are no longer admissible. This durable retry mechanism ensures
the converged role set is a function of the set of statements received, not
their order (plan §2.2, §2.4).

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

**Canonical on-disk layout (RT-FSBLOB, 2026-10-11; default root corrected
OF-1/S1b, 2026-10-14).** Every filesystem blob store lays a blob out as
`{root}/{hex[0..2]}/{hex}` (`blob_relative_path` / `canonical_blob_path`), and
the default root is now **`data/{DEFAULT_BLOB_DIR_NAME}`** (`data/blobs`,
CWD-relative, overridable by `FE_BLOB_DIR`) — co-located with this install's
own DB, not the pre-2026-10-14 per-user `{data_local_dir}/fractalengine/blobs`
(that expression survives only as `legacy_default_path()`, the one-time
migration source for `fe-database::handlers::blob_migration`). Both
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

## §blob-transport (PAS-P3 S2, 2026-10-13)

`blob_transport.rs` is the content-addressed transfer seam (decision D-77):
`BlobTransport` speaks only BLAKE3 `Hash32` (= `BlobHash`), HashSeq roots
(a blob of concatenated 32-byte child hashes, root = BLAKE3 of that blob),
named tags and an opaque `ProviderRef` (an iroh `EndpointAddr` JSON string on
the real transport). No iroh type (`RangeSpec`, `GetRequest`, `ChunkRanges`, …)
appears in it, and there is no byte-range method because nothing consumes one
yet (D-73 A3 is a later layout). It lives here, not in fe-sync, so PAS-P5's
fe-hexon P2P impl needs no fe-sync edge (contract C-P3 #1, critic C3).
`InMemoryNetwork`/`InMemoryTransport` is a production-compiled impl (tests,
the harness, fe-sim): one store per peer name, plus `tamper`/`withhold`/
`repair`/`set_online` hooks on the provider side. Like iroh, a fetch returns a
complete local copy first and caches remote bytes only when they hash to the
request, so tampered bytes reach the caller but never the store. Callers
(fe-sync `TilesetTransfer`) re-hash everything they persist and never rely on
a transport's verification. The iroh impl is `fe_sync::IrohBlobTransport`
(`fe-sync/src/AGENTS.md` §tileset-transfer).

**Size caps are part of the contract (S8b).** `fetch(provider, hash,
max_bytes)` and `fetch_hash_seq(provider, root, max_children)` take the
caller's cap; a transport that can learn a blob's size before downloading it
(iroh: BAO-verified size) must refuse an over-cap blob before its bytes enter
the store, and every transport refuses it before returning
(`ensure_within_cap`). The cap is the caller's because only the caller knows
what the blob is (bundle, index, HashSeq root). See `fe-sync/src/AGENTS.md`
§tileset-security.

## §safe-id (S8b, 2026-10-14)

`safe_id::is_safe_path_id` is the ONE guard for an untrusted content id
(tileset/hexon id) that becomes a path component, file stem or tag suffix:
`[A-Za-z0-9_.-]`, non-empty, not `.`/`..`, at most 128 bytes, and no Windows
device stem (`CON`, `NUL.x`, `COM1`, … — `CON.hexon` opens the console, not
a file). It lives here because every crate that joins such ids depends on
fe-runtime: fe-sync (`is_safe_tileset_id`), fe-format (re-exported as
`fe_format::is_safe_path_id`, which fe-terrain's `HexonStore` and
`safe_cache_dir` use) and fe-database (inbound petal terrain). Before S8b
there were three hand-copied charsets, and only fe-sync's refused device names.

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
  **Join failures (polish_lane_20261031 OD-2/M-03).** `JoinVerseByInvite`
  carries no `correlation_id` at all (GUI-only command, no API door calls
  it), so its failure reply, `DbResult::JoinVerseFailed { error }`, is not a
  "correlated family" entry above — it is simply never mapped by
  `reply_kind_of_result`/`correlation_of_result` (falls to their `_ => None`
  arms) and so never reaches `try_deliver`'s API waiters. It exists purely
  so fe-ui's Join dialog can distinguish "the join failed" from an unrelated
  bare `Error` and show the failure inline instead of a vanishing toast
  (fe-ui `verse_manager/AGENTS.md` §db-results). Do not add a
  `correlation_id` to it without also giving `JoinVerseByInvite` one — an
  id-bearing result with no way to mint/thread an id would be unreachable.
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
  **Petal render distance (2026-10-13, p2p_asset_streaming S6).**
  `SetPetalRenderDistance` / `GetPetalRenderDistance` share ONE reply family
  (`ReplyKind::RenderDistance`) and ONE result variant,
  `PetalRenderDistanceLoaded { petal_id, render_distance, correlation_id }`
  (mirrors `PetalVisibilityGroupsLoaded`'s shared-shape precedent, not
  terrain's split Fetch/Set families — render distance has no desktop
  live-apply consumer to protect from a correlated echo). Failures use the
  generic `RequestFailed` (below), never a bespoke variant.
  **Device enrolment (2026-10-14, CDL-W4 W4-8).** `EnrolLocalDevice` /
  `RevokeLocalDevice` carry `correlation_id: Option<String>` and share ONE
  family, `ReplyKind::DeviceEnrolment`, answered by `LocalDeviceEnrolled` /
  `LocalDeviceRevoked` (each echoes the id). Failures are `RequestFailed`
  when correlated and `Error` when not (`handlers::correlated_failure`). No
  caller exists yet: the commands are explicit-only by design (fe-database
  `canon_log/AGENTS.md` §device-enrolment). Test:
  `device_enrolment_replies_reach_only_their_own_waiter`.
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
**Correction (OF-1/S1b, 2026-10-14):** the blob store is no longer per-user
state — it is per-install, under `data/blobs` beside `data/fractalengine.db`,
the same `set_current_dir(packaged_root)` guarantee covers it. Only settings
(`<config_dir>/fractalengine/settings.ron`) and the OS keychain identity
remain per-user outside `data/`; the old per-user blob path is named only as
`FsBlobStore::legacy_default_path()`, the one-time migration source.

## §render-pressure (p2p_asset_streaming S8, D-74 B3)

DIAG-15M (`diag15m.rs`) began as temporary logging for the 2026-07-17 crash,
where the bind group hit 2.7 GiB. It is now the sensor for the residency
ledger's closed loop.

- **`RenderPressure(Arc<AtomicU64>)`** is created by `Diag15MPlugin::build`
  (`init_resource`, idempotent with fe-ui's own init). `finish` clones the same
  `Arc` into the render sub-app. The render-world `diag_log` stores into it and
  the main-world ledger reads it, so the two worlds share one cell with no
  extract step. Relaxed ordering is enough: the value is a gauge, and it is
  one frame stale at worst.
- **What it measures.** `instance_pressure_bytes` returns the LARGEST single
  binding: the current input buffer (`len × size_of::<MeshInputUniform>`) or
  any phase's data buffer (`len × size_of::<MeshUniform>`), whichever is
  bigger.
  - The 2 GiB `max_*_buffer_binding_size` limit applies per binding. A sum
    would halt admission on scenes where no single buffer is near the limit.
  - It uses element counts × element size, not the GPU `buffer.size()`. The
    buffers are grow-only, so their size would never fall after eviction, and
    the loop would stay latched.
  - It stays `0` when GPU preprocessing is off (CPU-building path, no
    `BatchedInstanceBuffers`). `0` means unmeasured, so the horizon is not
    shrunk.
  - The debug logging is unchanged.
- **`ResidencyStats { tile_bundles, tile_bytes }`** sits beside it. The main
  binary mirrors the fe-terrain counters into it
  (`terrain_bridge::mirror_tile_residency_stats`) for fe-ui's Settings readout.
  It lives in fe-runtime because fe-ui has no fe-terrain dependency (root
  `AGENTS.md` §petal-map).

## §sim-control (F9/A20)

`sim_control.rs` — the API→fe-sim bridge seam (`SimControlCommand` in a
`SimControlCall { cmd, reply_tx }`, typed `SimControlError`). It lives here
for the `distributed_query.rs` reason: fe-api needs the vocabulary and must
never depend on fe-sim, so wire payloads stay `serde_json::Value` and the
bridge parses `ScenarioScript`/`ScriptedEvent` on its side. Reply is a
crossbeam sender (the bridge thread has no async runtime). Surface rules:
`fe-api/AGENTS.md` §sim-control; driver/bridge: `fe-sim/src/AGENTS.md`
§session.

## §correlated-bridge (`bridge.rs`, agent_vision advocate pass, 2026-11-01)

ONE generic envelope for every API→Bevy request/reply seam (screenshot,
vision, input): `BridgeRequest<Op, Output> { correlation_id, caller_scope, op,
reply_tx }`, `BridgeReply<Output> { correlation_id, result }`, and
`bounded_bridge(capacity)`. It is **Request-Reply with a Correlation
Identifier** (§api-reply-correlation in miniature): every request carries a
ULID the reply echoes (the API rejects a foreign echo), each request owns its
oneshot (a reply can never land on another waiter), the API `try_send`s (a
full bridge is a busy refusal, never a blocked async worker), and
`BridgeRequest::reply` drops a late reply for a timed-out caller at debug
level.

`caller_scope` is a REQUIRED field, not an option: the token scope the Bevy
host authorizes against (agent_vision SEC-ADDENDUM: the scope must cover the
active verse at arrival AND at frame time). Making it part of the envelope
means a new seam cannot forget to carry it — `BridgeRequest::new(id, scope,
op)` is the only convenient constructor. The API end is ONE function,
fe-api `bridge_client::round_trip` (mint → `try_send` → bounded wait → check
the echo), whose `RoundTripError` each surface maps to its own typed error.

Each surface keeps its OWN channel (own capacity, own Bevy drain); only the
envelope and the round trip are shared. `ScreenshotRequest`/`VisionRequest`
are type aliases of the envelope.

## §screenshot-bridge (agent screenshot surface, 2026-10-31)

`screenshot.rs` holds only the API→Bevy capture seam's types (no render
dependency) on the §correlated-bridge envelope: `ScreenshotRequest =
BridgeRequest<CapturePrimaryWindow, Result<png bytes, String>>`, the matching
`ScreenshotReply`, `SCREENSHOT_SCOPE_DENIED` (the host's uniform denial text,
mapped to fe-api's 404 `NOT_FOUND_OR_DENIED`), and `screenshot_bridge()` — a
bounded(4) channel. The gate (opt-in → Owner → rate) is fe-api's
(fe-api/AGENTS.md §screenshot); the scope checks and the capture are the
binary's (fractalengine/src/AGENTS.md §screenshot).

## §vision-bridge (agent vision — dynamic capture + camera read-back, 2026-10-31)

`vision.rs` is a **second, separate** API→Bevy channel, deliberately not a
rename or extension of `screenshot.rs`: `GET /api/v1/debug/screenshot`'s
bytes stay untouched by anything the dynamic POST/`/camera` surface needs to
change later. The two share only the §correlated-bridge envelope
(`VisionRequest = BridgeRequest<VisionOp, VisionResult>`, bounded(4),
`try_send` only, a late reply is dropped at debug level) and fe-api's one
`round_trip`. `denied()` (`VisionFailureClass::Denied`, code
`not_found_or_denied`) is the uniform scope/existence refusal.

**Validate-once newtype.** `CaptureSpec` (the wire body: `mode`, `pose`,
`width`/`height`, `settle_frames`, `restore`) is deliberately permissive on
`pose`'s shape — it is a plain struct of five `Option<...Pose>` fields, not a
tagged enum, so a zero- or multi-key `pose` object parses fine at the serde
level and only fails inside `CaptureSpec::validate()`. That keeps every
malformed-pose case a **typed** `VisionRefusal { code, field, message }`
instead of an opaque serde error with no stable `code` for fe-api to map to a
400. `validate()` is the ONLY constructor of `ValidCaptureSpec` (private
fields, accessors only) — once a spec is a `ValidCaptureSpec`, nothing
downstream (fe-renderer's pose math, the Bevy capture orchestrator) ever
re-checks finiteness. A NaN that reached `Transform`/`looking_at` is exactly
the crash class (`ui-semantics-audit-20260808`'s `smart_aim` panic) this
newtype exists to make structurally unreachable.

**Check order is a documented contract**, not an implementation detail:
`validate()` runs spec `agent_vision_20261031` §5.2's table top to bottom and
returns on the first failure — `non_finite` beats `pose_must_have_exactly_one_kind`
beats the size/mode/settle checks beats the per-pose-kind range checks
(`fov_out_of_range` → `orbit_out_of_range` → `degenerate_camera` →
`geo_out_of_range` → `tile_out_of_range`, though only the active pose kind's
row can ever fire). A given malformed body therefore always refuses the same
way; the fixed order matters for tests asserting an exact `(code, field)` and
for an agent client learning the grammar from refusals.

**Defaults are mode- and pose-dependent, not static.** A window capture with
no `pose` needs no settle (`settle_frames` defaults to `0` — nothing moved);
every other case (window-with-pose, scene) defaults to `DEFAULT_SETTLE = 30`.
`width`/`height` only exist in scene mode (`None` in window mode — the
window's own size is authoritative there); `restore` only exists in window
mode (`None` in scene mode — there is nothing to restore). `ValidCaptureSpec`
exposes these as `Option` accessors so a mode mismatch cannot be
misread as "the caller asked for 0×0".

**Degenerate-camera math is pure `[f64; 3]` arithmetic**, not `bevy::Vec3` —
despite this crate already depending on `bevy` (for `SceneChange` etc.),
`vision.rs` stays render-free on purpose: the canonical pose math
(`compute_orbit_transform`, `orbit_from_look_at`, tile/geo framing) is V3's
job in `fe-renderer`/`fe-terrain`, not V1's. V1 only needs to detect the
*structural* degenerate cases (`position == target`, `up` zero or parallel to
the view direction) before a float ever reaches a renderer, using the same
dot/cross-product checks `Transform::looking_at` would otherwise panic on.

**`ValidPoseSpec`/`FocusTarget` carry resolved defaults**, not raw wire
optionals: `up` becomes `[0,1,0]` if absent, `fov_deg` becomes
`DEFAULT_FOV_DEG` (45°, matching the main camera's `FRAC_PI_4`), a `geo`
pose's `altitude_m`/`heading_deg`/`pitch_deg` get their spec-mandated
defaults, and a `focus` pose's `node_id`/`petal_id` collapse into one
`FocusTarget::{Node, Petal}` with `navigate` resolved to `false` if absent.
Downstream resolvers (V5) read a fully-resolved pose and never re-derive a
default.

**`ResolvedPose`/`CameraState`** are the reply shapes (spec §5.2 bottom,
§5.6) — the `X-FE-Capture-Pose` header's JSON, the MCP text part, and
`GET /camera`'s body all serialize the same `ResolvedPose`. They live here
(not in fe-api) because the Bevy-side `VisionOutput::Capture`/`::Camera`
variants that the bridge reply carries must already speak this exact shape;
fe-api only adds the HTTP/MCP envelope, never reshapes the payload.

**`VisionFailure::status_class`** is the one place a Bevy-side failure
chooses its HTTP status family (`Unresolvable` → 422, `Busy` → 409,
`NoWindow` → 503/422, `Failed` → 500/504), mirroring
`ScreenshotError::status()`'s precedent so fe-api's mapping stays a single
`match` with no cross-bridge special-casing.

## §input-bridge (agent_input_surface, folded into agent_vision_20261031 I1, 2026-11-01)

`input.rs` is a **third, separate** API→Bevy channel on the same
§correlated-bridge envelope the INPUT-ADDENDUM mandated: `InputOutcome =
Result<InputOutput, InputFailure>`, `InputRequest = BridgeRequest<InputOp,
InputOutcome>`, bounded(4) via `input_bridge()`, `try_send` only, a late
reply dropped at debug level — exactly `vision.rs`'s shape, never a fourth
bridge primitive. `caller_scope` on the shared envelope is what carries the
scope-confinement check (INPUT-ADDENDUM item 2: covered at drain time AND at
injection time, check-at-use, never cached) — this module only supplies the
vocabulary and the two typed failure constructors fe-api/the Bevy
orchestrator need; the scope check itself lives on those sides.

**Why `InputKey` is a `String`, not `bevy::input::keyboard::KeyCode`.**
`bevy::input::keyboard::KeyCode` and `bevy::input::mouse::MouseButton` only
derive `Serialize`/`Deserialize` behind `bevy_input`'s own `serialize`
feature, which this workspace's root `Cargo.toml` does not enable (verified
against `bevy-0.18.1`/`bevy_input-0.18.1`'s `Cargo.toml`: `default_app` does
not pull in `serialize`, and no other enabled feature does either). Rather
than widen the workspace's bevy feature surface for one wire type, `InputKey`
wraps the exact string spelling of the `KeyCode` variant name (`"Enter"`,
`"KeyA"`, `"ArrowLeft"`); the Bevy-side translator (plan I4,
`fractalengine/src/input_bridge.rs`) is the one place that matches the
string back to the real enum. `PointerButton` is this module's own
`{Left, Right, Middle}` for the identical reason, kept deliberately smaller
than bevy's `MouseButton` (which also has `Back`/`Forward`/`Other(u16)`) —
the v1 vocabulary (AIS spec §4.2) only asks for the three primary buttons.

**Validate-once newtype, same doctrine as `vision::ValidCaptureSpec`.**
`InputSpec::validate()` is the ONLY constructor of `ValidInputSpec` (private
fields, accessors only); nothing downstream re-checks finiteness, the batch
cap, the key-name shape or the `wait_frames` cap. Checks run in table order
— `no_commands` → `too_many_commands` → each command's own structural check
in **batch order** (including `no_effective_input` for an empty `text_entry`:
it would plan zero input frames, INPUT-ADDENDUM item 7) →
`wait_frames_out_of_range` — so a given malformed body
always refuses the same way (same rationale as vision's documented check
order).

**`InputRefusal.field` is an owned `String`, not `&'static str`.** Unlike
`vision::VisionRefusal` (whose dotted paths are a fixed, small set known at
compile time), an input batch's bad field is index-scoped —
`commands[2].text`, `commands[0].to.x` — so the path has to be built at
validation time. `code` stays `&'static str`: the stable machine string a
client matches on never depends on where in the batch the error occurred.

**`InputFailure::new(class, code, message)` is the one builder** both the
fe-api transport mapping and the Bevy orchestrator use; the named helpers
below are the two failures with a cross-crate contract.

**Two typed failure constructors, not just the uniform denial.**
`denied()` mirrors `vision::denied()` exactly (code `not_found_or_denied`,
`InputFailureClass::Denied` → 404) for the scope/existence case. A second,
input-specific one, `camera_locked_by_capture()` (code
`camera_locked_by_capture`, `InputFailureClass::Busy` → 409), is the
INPUT-ADDENDUM item 4 contract: the Bevy orchestrator returns it while
`CameraCaptureLock` (from `fe-renderer::camera`) exists, so synthetic input
can never fight a held capture pose. `InputFailureClass` otherwise mirrors
`VisionFailureClass` field-for-field (`Unresolvable`/`NoWindow`/`Failed` kept
for status-mapping parity even though only `Busy`/`Denied` are reachable
from this slice's own checks).

**`InputCommand` is a serde-tagged enum**, unlike `vision::PoseSpec` (a
permissive struct of `Option`s). Vision needed the permissive shape because
"exactly one of five pose keys" is a *cardinality* rule `validate()` must
enforce as a typed refusal. Input has no such ambiguity — each command
already names its own `kind` — so a malformed `kind` or a field/type
mismatch is a plain `serde` parse failure, which fe-api's JSON extractor
already turns into the generic `malformed_body` 400 (the same place vision's
own `deny_unknown_fields`/oversized-body row is handled); `input.rs` does
not need a second code path for that case.

**One journey test, not a scatter of point tests** (CLAUDE.md "Tests model
real flows" + the 2026-11-01 test-policy directive: swap/extend, or — for a
brand-new region with nothing to extend — one long journey). The single
`#[test]` in this module walks every row of `validate()`'s table in order,
the defaults a valid batch resolves to, a full mixed-vocabulary batch
round-tripped through JSON, both failure constructors, and the bridge's own
bounded capacity — one function, each assertion red if its branch inverts.

## §qa-debug

`qa_debug.rs` defines the bounded correlated read bridge for UI tree, UI state
and logs. It shares the existing BridgeRequest envelope. Filter strings are
bounded substrings, never regexes; unsupported query keys are refused.
`QaLogRing` holds at most 2000 redacted records, with messages bounded to 4096
bytes. The sequence is monotonic across verse generations. A verse change
clears retained records; startup/no-verse records are not retained. Payloads
are retained only for explicitly scoped events whose verse matches the active
verse. Other events expose severity, static target and a withheld-message
marker; navigation never assigns background payloads to the active scope.

The shared redactor removes JWT-shaped base64url triples and Bearer values
before truncation. The host Layer additionally redacts sensitive named fields.
It is deliberately conservative and may hide nonsecret dotted identifiers.
No redactor can infer every possible secret embedded in arbitrary prose;
callers must continue to avoid logging secrets at their source.

## Correlated node properties

Set/Get/DeleteNodeProperty and their three typed replies carry optional correlation IDs. GUI requests use None; every API origin, including background tracking writes with a dropped receiver, uses a fresh ID. The existing reply-family hub delivers only an exact correlation match and consumes closed waiters without shifting a reply. Database failures reuse correlated_failure and RequestFailed. This prevents both lost GUI-filtered acknowledgments and unrelated property results satisfying an API waiter.

## �ordinary-object-history

CreateObject/CommitNodeMutation reply NodeMutationCommitted; ReadNodeHistory replies
NodeHistoryLoaded or NodeStateAtVersionLoaded according to at_version. SetFieldSchema
uses QueryResult. All use RequestFailed on refusal and echo request IDs exactly.
Typed snapshot/receipt/history DTOs live in node_history.rs. Historical preview
results are distinct from current-state materializations and never apply to the
live scene by themselves.
CreateNode/NodeCreated now also participate in correlation extraction. The GUI-only NodeMaterialized birth
for CreateObject has its own reply family with no pending command and a fresh ID: they still reach GUI consumers but
cannot claim another request's pending waiter. The dedicated committed receipt is
separate. NodeMutation is a bounded operation vocabulary; full transforms use Euler
XYZ radians. History versions are per object and independent of transform edit_seq.

NodeAccessGrant is a non-deserializable internal enum: Local or Scoped(String).
The three ordinary history commands now require grant. API constructs Scoped
from validated token claims; the desktop adapter explicitly supplies Local.
UnavailableScope is a read-only history availability value; foreign or unknown
facts carry static unavailable_interval entries with no original payload. The
public REST envelope and committed materialization DTO shapes are unchanged.


Local now resolves the real DB-thread local identity and checks stored roles;
it never trusts a supplied actor label. Local write/read/schema floors are
Editor/Viewer/Manager. SetFieldSchema also requires grant; GetFieldSchema is the
correlated Viewer descriptor-read command. API schema GET/PUT still return the
same JSON payloads. The enum remains non-deserializable from request bodies.


## Object data projections

`node_measurements` owns the desktop measurement DTOs and shared 64-series/1,024-point bounds.
`ReadNodeMeasurements` is a desktop-only command and is intentionally not registered as a pending
API reply family. Its fresh `ui-*` request ID is consumed by the Inspector state. API measurement
queries continue through their existing admitted query surfaces. Typed history replies retain their
existing API families; native views claim only their own exact GUI request IDs.
## Hierarchy details read messages

GetEntityDetails/EntityDetailsLoaded are a GUI-local read family with a u64
process-local request nonce, separate from API correlation IDs and reply kinds.
The typed result echoes requested kind/id/nonce even on refusal, and its payload
contains only Viewer-authorized stored target settings and ancestry. The GUI
checks both outer and inner identities and the expected scope. No HTTP/MCP
surface or API waiter can use this as a reply to an unrelated command.
