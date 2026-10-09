# fe-api — API gateway (REST + WS + MCP)

Runs on its own multi-thread tokio runtime (`spawn_api_thread`), talking to the
Bevy/DB threads over `crossbeam::channel` (`ApiCommand` -> `DbCommand` ->
`DbResult`, matched via `tokio::sync::oneshot` per request — see
`fe-runtime/src/app.rs` `drain_api_commands` + `PendingApiRequests`, read-only
from here). Replies are matched by **reply family**, not arrival order, because
results also arrive unsolicited (relay startup scan, GUI reloads) on the same
channel — rationale + rules in `fe-runtime/src/AGENTS.md` §api-reply-correlation.
Where a `db_reader: Option<Arc<Surreal<Db>>>` is configured,
handlers may bypass that round-trip entirely with a direct SurrealDB query
(`direct_*` helpers in `rest.rs`/`assets.rs`) — this is the established escape
hatch for reads that don't have (or don't need) a dedicated `DbCommand`.

## §websocket

`ws.rs` treats every petal and node target as deny-by-default: resolve its
hierarchical scope, require token containment, then subscribe or mutate. A
transform broadcast carries the resolved petal identifier rather than trusting
client attribution. `SceneChange` events are filtered by their mandatory
petal attribution; transform rollbacks use the same filter. When either
broadcast receiver lags, the connection receives fresh snapshots for its
currently subscribed petals. This is lag hygiene only: scene versions remain
connection-local until the canonical commit-cursor protocol exists. An empty
petal is a valid snapshot; database, channel, timeout, decode, or unexpected
result failures are instead reported as `resync_required` and emit no snapshot.
Lag recovery clears the connection's scene subscriptions before that error so
no incremental changes resume until the client explicitly resubscribes.

## §routes

Route inventory for `server.rs::build_router`. The `.route()` registrations
there are the source of truth; this table is the browsable index — update it
when a route lands.

**Public** (no auth; WS authenticates after the upgrade handshake):

| Method | Path | Handler |
|--------|------|---------|
| GET | `/api/v1/health` | inline |
| GET | `/ready` | `ready_handler` (DB ping) |
| GET | `/ws` | `ws::ws_handler` |
| GET | `/api/v1/shared/{token}` | `share::redeem_share_url` (§share — the signature is the credential) |

**Authenticated** (Bearer JWT via `auth::auth_middleware`):

| Area | Routes |
|------|--------|
| Hierarchy CRUD | `GET /api/v1/hierarchy`; `POST /api/v1/verses`, `POST …/verses/{v}/fractals`, `POST …/fractals/{f}/petals`, `POST …/petals/{p}/nodes`; legacy flat `POST /api/v1/nodes` |
| Node ops | `PATCH\|GET /api/v1/nodes/{id}/transform`, `PATCH\|GET …/properties`, `DELETE …/properties/{key}`, `PATCH /api/v1/nodes/{waypoint_id}/move`, `GET /api/v1/nodes/{track_id}/elevation-profile`, `GET …/stats` |
| Per-endpoint surface (§endpoint-surface, T5) | `GET\|DELETE /api/v1/nodes/{id}`, `GET /api/v1/nodes/{id}/address`, `GET /api/v1/address?uri=…`, `GET /api/v1/petals/{p}/nodes?kind=…`, `GET /api/v1/petals/{p}/earthwork/summary`, `POST /api/v1/petals/{p}/paths/{path}/instances/{i}/promote` |
| Waypoints | `POST /api/v1/petals/{p}/waypoints` |
| GIS reads (§gis) | `GET /api/v1/petals/{p}/gis/nodes`, `GET …/gis/tracks` |
| Assets (§assets) | `GET /api/v1/assets/{content_hash}`, `GET /api/v1/assets/by-id/{asset_id}`, `GET /api/v1/nodes/{id}/asset` |
| Petal archive / GPX | `GET /api/v1/petals/{p}/export`, `POST …/import`, `POST …/import/gpx`, `GET …/export/gpx` |
| Terrain config | `GET\|PUT\|DELETE /api/v1/petals/{p}/terrain` |
| Tile data plane | `GET /api/v1/tiles/elevation/{id}/{z}/{x}/{y}.png?petal_id=...`, `GET /api/v1/tiles/satellite/{id}/{z}/{x}/{y}.jpg?petal_id=...`, `GET /api/v1/tilesets?petal_id=...`, `GET /api/v1/tilesets/{id}/meta?petal_id=...` |
| Field defs | `POST /api/v1/field-defs`, `GET /api/v1/field-defs/{scope}`, `PATCH\|DELETE /api/v1/field-defs/by-id/{id}` |
| Query / BI egress (§query-guard, §export, §share) | `POST /api/v1/query` (body `distributed: true` → §distributed-query), `POST /api/v1/query/elevated`, `POST /api/v1/query/share`, `GET /api/v1/petals/{p}/export.parquet`, `GET …/export.csv`, `POST /api/v1/analytics/query` (merged `iot_reading` table via §distributed-query) |
| IoT ingest (§iot-ingest) | `POST /api/v1/petals/{p}/iot/readings` |
| Hexon tilesets | `POST /api/v1/hexons/tilesets/install?petal_id=…`, `DELETE /api/v1/hexons/tilesets/{id}?petal_id=…`, `PATCH …/{id}/seeding?petal_id=…`, `GET /api/v1/hexons/tilesets?petal_id=…`, `GET /api/v1/hexons/storage?petal_id=…` |
| Hexon crate registry | `POST /api/v1/crates/publish`, `POST /api/v1/crates/{uri}/install?petal_id=…`, `DELETE …/{uri}/uninstall?petal_id=…`, `GET /api/v1/crates/search?petal_id=…`, `GET /api/v1/crates/installed?petal_id=…`, `GET /api/v1/crates/{uri}?petal_id=…`, `GET …/{uri}/entries?petal_id=…`, `GET …/{uri}/entries/{entry_id}/asset?petal_id=…`, `GET /api/v1/crates/available?petal_id=…` |
| MCP | `POST /mcp` (`mcp::mcp_handler`) — 11 tools: 6 base + per-endpoint CRUD `read_node` / `node_address` / `delete_node` / `promote_instance` (§endpoint-surface) + `query_timeseries` (§distributed-query) |

## §endpoint-surface (`endpoint.rs`, track `endpoint_api_surface_20260725`, T5)

Makes every object a stable `fe://verse/fractal/petal/node` endpoint you can
**read** (its full data) and **write** (drive it) over REST + MCP (D-A4).

- **Addressing (FR-1).** The canonical URI is T1's `fe_entity_store::NodeAddress`
  (defined at the data layer; Q-1 override). This crate *exposes* it —
  `GET /api/v1/nodes/{id}/address` (id → URI) and `GET /api/v1/address?uri=…`
  (URI → components) — and the render side reconciles via
  `fe-renderer/src/addressing.rs::RenderNodeAddress`, a lock-step mirror pinned
  byte-for-byte by test (fe-renderer can't depend on fe-entity-store). The URI is
  a pure function of the four ids, so it survives rename/move.
- **Read (FR-2).** `GET /api/v1/nodes/{id}` → `EndpointNodeDto` (address, scope,
  `kind` tag, full row) via the tombstone-filtered `load_node`. Unknown/deleted →
  typed `{"ok":false}`; never a panic. Shared with the MCP `read_node` tool.
- **Write (FR-3).** `DELETE /api/v1/nodes/{id}[?cascade=true]` routes through T1's
  sync-safe `TombstoneNode` / `CascadeTombstoneNode` ops (never a raw drop, N-4);
  `POST …/paths/{path}/instances/{i}/promote` through `PromoteInstance`. The API
  is a *client* of the same ops the UI uses.
- **Auth model (FR-3/FR-4, Q-2).** API/MCP callers are
  `CallerAuth::Identified { did, role }` — the token's already-resolved role
  ceiling (never `CallerAuth::Local`, that is the desktop UI's). The handler does
  the same scope-containment check every node op does (`require_scope` on the
  resolved node/petal scope); **fe-policy is the authoritative Editor+ gate** at
  the DB thread (N-5 — the API layer carries identity, does not decide policy).
- **Generic node abstraction + type tags (FR-6, N-10).** Promoted stamps (T2) and
  earthwork regions (T3) are ordinary `node` rows tagged by
  `properties.node_kind` (`stamp` / `earthwork_region`). This track reads them
  through `fe_query::spatial_nodes` (the tag vocabulary + type-specific keys are
  the JSON contract T2/T3 write and T5 reads — no fe-api→T2/T3 file coupling):
  `GET /api/v1/petals/{p}/nodes?kind=stamp&path_id=…` ("all stamps on path X") and
  `GET /api/v1/petals/{p}/earthwork/summary` (real-unit total cut/fill). Both run
  under the `limits` row cap + byte ceiling (`run_capped_select`).
- **Egress seam (FR-5).** The analyst/context-menu (T4) copy-API-string and
  report verbs call `fe-ui gis::egress_strings::{api_string_for, report_for}`
  (pure string formatting, no `block_on`).
- **Tombstone hygiene.** All REST/MCP/export node read surfaces filter
  `tombstone = NONE` so soft-deleted nodes never leak
  (`rest.rs`/`gis.rs`/`gpx.rs`/`format.rs`/`export.rs`, `endpoint.rs`). The raw
  BI `/query` path is intentionally *not* filtered — an analyst may query
  historical/tombstoned rows deliberately (documented open item).

## §gis

Two petal-scoped read endpoints (`src/gis.rs`), under the JWT-authenticated
router in `server.rs`:

| Method | Path | Notes |
|--------|------|-------|
| GET | `/api/v1/petals/{petal_id}/gis/nodes` | geo-positioned nodes with their `gis.annotation.*` bundle; optional `bbox` / `bbox_ll` / `radius` filters. |
| GET | `/api/v1/petals/{petal_id}/gis/tracks` | GPX track nodes (`properties.gpx_type == "track"`) with the cached stats GPX import wrote. |

**RBAC**: Viewer+ role + petal-scope coverage, resolved via
`rest::direct_resolve_petal_scope` (or the `ResolvePetalScope` channel command
when no `db_reader` is wired). Deny-by-default and real HTTP status codes,
mirroring `§assets`: 400 (bad ULID / bad filter params), 403 (role or scope),
404 (unknown petal), 502 (query transport failure). This is stricter than the
older `rest.rs` handlers that return 200 + `{"ok":false}`; GIS reads follow the
asset-delivery precedent because external consumers (dashboards, IoT) care
about status codes.

**Annotation-key contract** (shared with the `gis_query_ui_20260711` track):
reserved node-property keys `gis.annotation.title` / `.body` / `.color`. The
canonical constants live in `fe_query::gis` and are **re-exported** from
`gis.rs` (`pub use fe_query::gis::{ANNOTATION_*_KEY}`) so the endpoint and the
query layer share one definition and can't drift. These keys are stored by
`DbCommand::SetNodeProperty` as **flat dotted keys** inside the node's
`properties` object (`properties[$key] = $val`, per
`fe-database/handlers/entity_property.rs`), so extraction reads
`properties["gis.annotation.title"]` — `annotation_str` also tolerates a nested
`{"gis":{"annotation":{...}}}` shape defensively. Absence of all three keys ⇒
no `annotation` field on the DTO.

**Coordinate model**: `position` is a SurrealDB `geometry<point>` stored as
`[x, z]` (local meters, XZ plane) with `elevation` as a separate `y` column.
SELECTs decode `position.coordinates`; this crate never *writes* geometry (see
`fe-database/src/AGENTS.md §geometry-inserts`). `bbox` / `radius` are in
petal-local meters. `bbox_ll` (lat/lon) is converted **API-side** using
`fe_terrain::projection::Projection` seeded from the petal's
`terrain.origin.{origin_lat,origin_lon,origin_ele}` — the same equirectangular
projection GPX import uses, so a round-tripped GPX box lands where its nodes do.
If `bbox_ll` is requested on a petal with no terrain origin, the endpoint
returns 400 rather than guessing an origin. At most one spatial filter may be
supplied per request (else 400).

**Filter-in-Rust tradeoff** (deliberate divergence from the `fe-query` spatial
builders — flag for the coordinator sweep): the SQL rendered locally in
`gis.rs` selects the petal's nodes (`WHERE petal_id = $pid`, plus the verified
`properties.gpx_type = 'track'` navigation for tracks — mirroring `gpx.rs`, not
the novel `(properties ?? {})[...]` construct) and the spatial predicate is
applied in-process over the decoded rows (`Bbox::contains` / `within_radius`,
both pure + unit-tested).

`fe_query::gis` now ships `nodes_in_bbox`/`nodes_within_radius`/
`annotated_nodes` and the coordinator asked to prefer them. We reuse its
annotation **constants** but keep local Rust filtering for the spatial
predicates for two reasons: (1) **correctness** — `nodes_within_radius` renders
`geo::distance(position, …)`, which in SurrealDB is a **geodesic** (haversine,
lon/lat→meters) computation; our `position` values are **petal-local meters**
on the XZ plane, so `geo::distance` is semantically wrong for a local-meter
radius (it only matches at the exact center). Euclidean `(x-cx)²+(z-cz)² ≤ r²`
is the correct metric and lives in `within_radius`. (2) **verifiability** — the
`geo::inside`/`geo::distance` cast-in-argument forms are new to this repo and
unverified until the coordinator's cargo sweep; this crate must not `cargo`
here, so betting the endpoint on unverified DB constructs is avoided. Swapping
to the `fe-query` builders is a localized change once the sweep confirms the
`geo::*` path executes **and** the geodesic-vs-local-meter radius semantics are
resolved (either a `math::`-based Euclidean builder, or a decision that
`position` is lon/lat). Keeping the math in pure functions also satisfies the
track's "bbox filter math" test requirement without a live DB.

Data access rides `db_reader` directly (like the other read handlers) and falls
back to the `DbCommand::RawQuery` gateway channel (single SELECT, bound vars,
no `;`, no blocked bare-word keywords). No new `DbCommand`/`DbResult` variants
were added (the dispatch match lives in the quarantined
`fe-database/src/lib.rs`).

## §assets

Three GET endpoints, all under the JWT-authenticated router in `server.rs`:

| Method | Path | Notes |
|--------|------|-------|
| GET | `/api/v1/assets/{content_hash}` | raw blob by BLAKE3 hash, `application/octet-stream`, immutable cache headers. No RBAC scope check (content hash carries no ownership) — pre-existing endpoint, unchanged. |
| GET | `/api/v1/assets/by-id/{asset_id}` | resolves `asset` row -> blob, serves with the **real** `content_type`/`name`/`size` from the DB. RBAC scope resolved via the first `node` referencing that `asset_id`. |
| GET | `/api/v1/nodes/{node_id}/asset` | resolves `node.asset_id` -> `asset` row -> blob. RBAC scope resolved from the node's parent chain (same `resolve_node_scope` helper as `/nodes/:id/transform`). |

Both new endpoints require Viewer+ role and require the node's/asset's scope
be covered by the token, require a **valid ULID** path param (reuses
`types::is_valid_ulid` — length + charset check, no separate validator), and
never build a filesystem path from user input: the blob path always comes
from `BlobStore::get_blob_path(hash)`, keyed by the DB's `content_hash`
column, never from the request. Every error path returns a structured JSON
body (`{"ok": false, "error": "..."}`) with a real HTTP status
(400/403/404/500/502/503/501/413) and a `tracing` log line — this deliberately
differs from the rest of `rest.rs`, where most handlers return HTTP 200 with
`{"ok": false, ...}` for historical reasons; asset delivery is byte-stream
territory, so real status codes matter for HTTP caches/CDNs/downloaders.

**Asset scope resolution caveat**: the `asset` table has no scope/owner column
of its own — only `node.asset_id` links an asset into the hierarchy, and
nothing stops two nodes (even in different petals) from pointing at the same
`asset_id`. `by-id` lookups authorize against whichever node happens to be
returned first by `SELECT petal_id FROM node WHERE asset_id = $aid LIMIT 1`.
This is fine for the common case (one asset, one importing node) but is not a
real multi-owner model; if assets need independent RBAC, that's a schema
change in `fe-database` (out of scope here — see integration requests below).

**Directory-asset extension point**: the asset model is heading toward "any
file, or a directory of files behind a placeholder" (the P2P bucket / "3D
visual IPFS" idea). Today `serve_asset_by_id` special-cases a sentinel
`content_type` of `application/x-fe-directory` and responds `501 Not
Implemented` with a structured JSON body instead of guessing at bytes; when
directory assets are real, that branch is where a manifest/listing response
slots in (e.g. `GET .../asset` returning a file listing + per-entry sub-URLs
when `content_type` is the directory sentinel, falling through to today's
single-blob behavior otherwise) — no route or RBAC changes needed, only that
one branch grows.

## §query-guard + §limits

## §analytics-query

`POST /api/v1/analytics/query` is a deliberately narrower DataFusion surface
than `/api/v1/query`: its body requires both `sql` and a concrete `petal_id`.
The handler requires Viewer+, validates and directly resolves that petal's
hierarchical scope, then requires token containment *before* registering the
`EntityStore` table. The table is always registered with the authorized petal
filter; caller SQL can never select an all-petals table. Analytics has no
crossbeam scope-resolution fallback: without a direct DB reader, or when the
petal cannot be resolved, it returns an explicit unavailable error.

The `EntityStore` is hydrated before the desktop API starts, but remains a hot,
eventually refreshed cache rather than the canonical database. Authorization is
fresh from the direct reader, while data can lag a just-committed mutation until
the cache receives its scene change; the bounded bridge has no automatic
resync after a drop. Callers requiring read-after-write semantics must use the
scoped DB/export surfaces instead.

**Host wiring (F24, 2026-10-08).** `src/entity_store_bridge.rs` is the shared
analytics-cache wiring every host binary rides: `hydrate_entity_store`
(startup snapshot from live nodes, 10s-bounded, fail-closed on a malformed
row) and `runtime_scene_change_to_store` (the fe-runtime → fe-entity-store
`SceneChange` conversion — fe-entity-store deliberately stays fe-runtime-free,
so each host converts at its own seam). Both binaries now wire the cache: the
GUI drains via its Bevy `drain_scene_changes_to_store` system, and the relay
(the headless host) creates the store, subscribes the scene-change broadcast
BEFORE its DB thread spawns (no event gap), drains on a dedicated thread, and
hydrates from pre-startup rows when its API read connection opens — closing
the relay's `entity_store: None` TODO that kept the merged analytics surface
dead on every platform. A relay-side hydration failure fails the surface
closed (`entity_store: None`) rather than serving a partial nodes table;
the GUI's pre-extraction local copies of these fns remain until its
mechanical migration.

**Honest-unavailable posture on per-handle-lock platforms (Windows).** The
analytics authorization resolves the petal scope through the DIRECT reader by
design (no crossbeam fallback — this section's first paragraph), and
SurrealKV's per-handle file lock (os error 33, M1/F4-documented) rejects the
second in-process connection while the DB-thread writer lives. On such
platforms the analytics surface is therefore honestly unavailable live on
BOTH binaries — `analytics authorization unavailable (no direct DB reader)` —
even with the `EntityStore` wired. This is deliberate fail-closed
posture, not an oversight to be "fixed" by routing analytics authorization
through the fallback: F24 added a DB-thread fallback for IoT ingest (a write
with a sanctioned `InsertIotReadings` command), but the analytics
authorization design predates F24 and stays direct-only — extending it to
the channel seam is a design change for the orchestrator to weigh, not
something F24 forced. The merged surface serves live wherever the read
connection opens.

**Subquery/whitespace hardening (2026-07-15 security review):** the table
whitelist is enforced on EVERY `FROM` clause via `from_clause_tables`
(subqueries included; non-identifier FROM targets like `$var` or
`type::table(...)` are rejected outright) — first-FROM-only checking let a
nested `SELECT` read any table. `ROLE`/`VERSE_MEMBER` were removed from the
read whitelist (RBAC data is not BI egress; the role-gated elevated endpoint
retains them). The elevated endpoint's DDL screen moved from bypassable
multi-word substring matching ("DEFINE TABLE" vs "DEFINE  TABLE") to
whole-word keyword bans + all-occurrence target checks in
`validate_elevated_sql`. Fail-closed tradeoff accepted: keywords inside
string literals reject the query.

`src/query_guard.rs` is the single guard pipeline for every read-only SQL
egress path: `/api/v1/query`, both export routes, and shared-URL redemption.
It was factored **verbatim** out of `rest.rs::execute_query` (error strings
preserved) so new egress handlers cannot bypass a guard by construction:
`guard_and_prepare_query` = rate limit (1s sliding window, keyed string) →
`validate_select_sql` (semicolon reject, SELECT-only, keyword blocklist,
table whitelist) → `build_scope_filter`/`inject_scope_filter` (petal-scoped
tokens get `petal_id = '…'` injected into node-table queries). Execution goes
through `run_guarded_query`, which owns the pre-existing **5s statement
timeout** (do NOT add another) and the FR-4 **row cap**. The row-cap policy is
**error, not truncate**: exceeding it returns `row cap exceeded (limit N rows…)`
so a BI tool never silently sees partial data. `enforce_byte_ceiling` guards
serialized response size the same way.

`src/limits.rs` holds every cost knob as a named constant (plan D4):
`/query` 10 000 rows / 8 MiB; exports 500 000 rows / 128 MiB; rate limits
10/s per DID (`/query`, exports) and per token (share redemption); share TTL
default 1h / max 24h. Change limits there, nowhere else.

## §export

`src/export.rs` — `GET /api/v1/petals/:petal_id/export.parquet|export.csv`
(`?query=<urlencoded SELECT>&coords=local|latlon`), the FR-2 BI egress. Flow:
Viewer+ role → valid ULID → petal scope coverage (`resolve_petal_scope`,
deny-by-default, real HTTP statuses per the §assets precedent) → shared guard
pipeline → **forced petal pre-filter** (`petal_id = :path_petal` injected
regardless of the query text — FR-6 export pre-filtering) → rows mapped to
`EntitySnapshot` → fe-query's GeoParquet writer (`write_nodes_parquet_bytes`,
in-memory; no temp files) or local CSV serialization.

- Export queries are **node-table-only** (400 otherwise): the output schema is
  the snapshot/GeoParquet nodes table; other tables belong to `/query`.
- Parquet responses ship `Content-Type: application/vnd.apache.parquet`,
  `Content-Length` (axum), and `Accept-Ranges: bytes` so DuckDB httpfs can
  `read_parquet('<url>')` (plan D1).
- CSV is RFC-4180 with a leading `# crs=<label>` comment line (documented
  choice: comment line + `X-FE-CRS` header; a sidecar column would bloat every
  row) and **properties as one JSON-string column** (flattening arbitrary keys
  would make the header schema query-dependent).
- `coords=latlon` converts through the petal `Projection` at the API layer
  (never in fe-query/fe-database); position becomes `[lon, lat, ele]`
  (GeoParquet EPSG:4326 axis order) / `lon,lat,ele_m` CSV columns. 400 when
  the petal has no terrain origin. Precision note: parquet positions pass
  through `EntitySnapshot`'s `f32` (≈1 m at mid-latitudes) — acceptable v1,
  revisit if survey-grade egress is needed.
- Status mapping: 400 bad query/coords, 403 role/scope, 404 unknown petal,
  413 row-cap/byte-ceiling, 429 rate limit, 502 query transport, 503 no
  db_reader, 504 statement timeout.

## §share

`src/share.rs` — FR-2/FR-6 signed shareable query URLs (plan D2: signed scoped
URL, NOT an embedded bearer JWT). Token = `b64url(payload).b64url(sig)` where
payload is `{v, sql, scope, fmt, exp, sub}` and sig is **ed25519** over the
exact payload bytes, verified with `verify_strict` (repo standard; chosen over
HMAC because the identity stack is already ed25519 — no new secret type).

- `POST /api/v1/query/share` (authed, Viewer+): body `{sql, format, ttl_secs}`
  (the fe-ui egress-card contract). Every static guard runs at mint (fail
  fast); TTL default 1h, max 24h; **scope ceiling = the issuer's token scope
  at signing time**, embedded verbatim.
- `GET /api/v1/shared/{token}` (public route): the signature is the
  credential. Verification failure → 401, expiry → 410. Redemption re-runs the
  full guard pipeline with the token's scope ceiling substituted for live
  claims scope, rate-limited per token fingerprint. `fmt=json` returns the
  `/query` envelope (incl. `crs`); `fmt=parquet|csv` reuses the §export
  pipeline and therefore requires a petal-scoped ceiling (400 otherwise —
  enforced at mint too).
- **Key lifetime**: the signing keypair (`ApiState.share_signer`) is generated
  per process in `run_server` — restarts invalidate outstanding links, which
  is acceptable at ≤24h TTL. Wiring the node's persistent keypair through
  `ApiConfig` is a one-line integration in `main.rs` left as an integration
  request (outside `fe-api/**`).

## §crs

`src/crs.rs` — FR-5/D3 egress CRS resolution. `resolve_petal_crs` walks the
three branches: (1) petal terrain config references an installed hexon tileset
→ label carries the hexon's `crs`/`native_scale` (`TilesetMeta` from
`hexon_scale_orchestration_20260712`) as `datum=`/`native_scale=` suffixes;
(2) terrain origin only → `PETAL-LOCAL:meters;origin=<lat>,<lon>,<ele>`;
(3) neither → documented placeholder `PETAL-LOCAL:meters;origin=unset`
(matches fe-query's GeoParquet default — a local-meters export is **never**
silently labeled EPSG:4326; only `coords=latlon` output gets `EPSG:4326`).
The `/query` JSON envelope gains an optional `crs` field: petal-scoped tokens
resolve their petal, broader scopes get the `origin=per-petal` marker because
one response can mix petals with different origins.

**Integration requests** (would require edits outside `fe-api/**`, so left as
requests rather than done here):
- ~~`fractalengine/src/main.rs` passes `blob_store: None`~~ **CLOSED (A11/F5,
  re-verified F23 2026-10-08).** Both binaries now pass `blob_store:
  Some(handle)` — `fractalengine/src/main.rs` (`blob_store_for_api`, shared
  with the DB thread, sync thread, Bevy `blob://` source, and the download
  bridge) and `fractalengine-relay/src/main.rs`. The `None` branch remains in
  `fe-api` only for tests / a future relay-only deployment. Load-bearing since
  A11: without it every asset endpoint 503s and the IoT ingest emit seam
  degrades to durable-but-unpublished.
- A `DbCommand::GetNodeAsset { node_id }` / `DbCommand::GetAssetMeta
  { asset_id }` variant (returning name/content_type/size_bytes/content_hash)
  would let these endpoints work over the crossbeam channel when no
  `db_reader` is configured (e.g. a future relay-only deployment). Today they
  return 503 in that case, same as `blob_store` being absent.
- Share-URL signing key persistence (§share): pass the node's `NodeKeypair`
  into `ApiConfig`/`run_server` from `fractalengine/src/main.rs` so shareable
  links survive a restart; today the key is ephemeral per process.

## §hexon-scope

All authenticated Hexon crate and tileset-management routes require a
`petal_id` query parameter. `hexon::require_hexon_petal_access` validates the
identifier, resolves its hierarchy scope, checks token containment, and enters
the `fe_hexon::authz` policy gate. Crate discovery and reads are filtered to
the requested petal binding; peer announcements are deliberately not exposed
through this surface because they carry no verifiable petal scope.

Tileset installation and removal both require an **exclusive existing terrain
binding**: the archive's declared ID (for install) or route ID (for removal)
must appear in the requested petal's `terrain.tileset_hexon_uris`, and no other
petal may reference it. This is checked before any local-store mutation, so an
editor for Petal A cannot create, overwrite, or remove an unbound ID or an ID
owned by Petal B. Listing and storage summaries are filtered to the requested
petal's bindings. API seeding changes are denied while P2P distribution is
disabled. The tile data plane is authenticated and requires the same
`petal_id`: each list, metadata, and tile-byte request re-resolves the petal
scope and permits only IDs bound in that petal's terrain configuration. Unbound
or unknown IDs both return 404, and byte responses are `private, no-store` so a
shared cache cannot replay a scoped tile to another caller.

## §iot-ingest (iot_spatial_reporting_20260714)

`iot.rs` — `POST /api/v1/petals/:petal_id/iot/readings`, the minimal-v1
external ingestion hook (batch JSON; webhook-able; a poll connector catalog is
future work). Design notes:

- **Guard order mirrors export.rs**: `require_role("editor")` (writes need
  Editor+, unlike the Viewer+ read endpoints) → ULID check →
  `resolve_petal_scope` → `require_scope` → per-DID rate limit
  (`limits::IOT_INGEST_RATE_PER_SEC`) → batch cap
  (`limits::IOT_INGEST_MAX_READINGS`, 413 past it).
- **The write goes straight to `db_reader`** via
  `fe_database::handlers::iot_reading::insert_readings_with_replication`, not
  through a `DbCommand` round-trip: readings are append-only facts with no
  derived counters, so the DB-thread single-writer invariant doesn't apply
  (rationale in fe-database `src/AGENTS.md` §iot-readings), and IoT-frequency
  batches must not queue behind the render loop's channel.
- **DB-thread fallback when `db_reader` is absent (F24)**: the SurrealKV
  per-handle file lock (os error 33 on Windows, M1/F4-documented) rejects the
  API read connection while the DB-thread writer lives, so `db_reader` is
  `None` on the deployment platform — the handler used to 503 there, killing
  REST ingest live. It now falls back to the DB-thread seam
  (`DbCommand::InsertIotReadings`, F7) over the SAME `ApiCommand::DbRequest`
  channel every channel-fallback read uses: the write happens ON the DB
  thread (MORE aligned with the single-writer rule than the `db_reader`
  append-only exception), and the DB-thread arm rides
  `insert_readings_with_replication`, so the per-row `ReplicationEvent`s fire
  exactly as on the direct path. No guard is weakened or reordered: Editor+
  role → ULID → petal scope → token containment → per-DID rate limit → batch
  caps all run on this path BEFORE the command is sent, and `claims.sub`
  rides the command as `source_did` so the acting caller's identity reaches
  the DB-thread context. The fallback fires ONLY when `db_reader` is `None`;
  with a reader configured the direct path is byte-identical to before.
  Bounded by a 10s round-trip timeout (504 on timeout, 502 on
  transport/DB failure).
- **Replication emit seam (A11, F5)**: the same call publishes one
  `ReplicationEvent` per accepted row, so readings ingested over HTTP reach
  peers exactly like DB-thread writes do. It needs `ApiState.replication_tx`
  (the DB→sync sender) and `ApiState.blob_store` (row bytes → content hash) —
  `fractalengine/src/main.rs` must wire both for the GUI binary to replicate
  ingested readings; a `None` in either field degrades to
  durable-but-unpublished, never to a failed ingest. The verse id comes from
  `parse_scope(resolved_petal_scope)`, never from the request body.
- **Validation failures map to real statuses**: unknown/foreign-petal anchor,
  bad RFC-3339 timestamp, or empty metric → 422 (typed `IotIngestError`, no
  string-sniffing); DB failure → 502; empty batch → 400. **On the F24
  fallback the typed detail crosses the DB-thread seam as
  `DbResult::IotReadingsRejected` carrying
  `fe_runtime::messages::IotIngestRejection`** (same reply family as
  `IotReadingsInserted`): validation failures stay 422 with byte-identical
  wording to the direct path (parity pinned by a fe-database test), and only
  DB failures degrade to the generic `DbResult::Error` → 502.
- **Egress seam (FR-5)**: `iot_reading` is whitelisted in
  `query_guard::ALLOWED_TABLES` and `inject_scope_filter` injects the petal
  filter on `FROM iot_reading` (rows carry a denormalized `petal_id`), so
  `/api/v1/query` + shared-URL redemption serve IoT rows scope-guarded today.
  Reading-shaped `export.parquet`/`export.csv` (flat reading rows + optional
  anchor-position join) is the remaining FR-5 polish — `prepare_export` is
  still node-table-only.

## §distributed-query (M2/F7 — A15/A16/A17)

`src/timeseries_query.rs` — the ONE guarded bridge every distributed
timeseries surface shares: `POST /api/v1/query` with body
`{"distributed": true, "query": {…TsQueryKind…}}`, the analytics
endpoint's merged `iot_reading` table, and the MCP `query_timeseries`
tool. It fans a query out to the verse's fleet over the API→sync seam and
merges the per-host partials (the transport, planner, and merge live in
fe-sync `distributed_query.rs` — see fe-sync/src/AGENTS.md
§distributed-query).

- **Authorization happens HERE, once** (`run_distributed_timeseries_query`):
  Viewer+ role → valid ULID → `resolve_petal_scope` → token containment →
  per-DID rate limit. The verse id is derived from the resolved petal scope
  (never from the request body) — the same rule as §iot-ingest. All three
  surfaces go through the same call, so a guard fix lands everywhere at
  once; the A17 tests pin that a scope-denied call never even reaches the
  fan-out seam.
- **The seam is `ApiState.distributed_tx`** (the same
  `DistributedQueryCallSender` channel shape the replication bridge uses);
  the binary bridges it into `SyncCommand::SubmitComputeTask`. A dead or
  unwired seam is an honest explicit error (`TimeseriesQueryError`),
  never a silent empty result — the `/query` surface returns 200 +
  `{"ok": false}` (house style), analytics maps to real HTTP statuses,
  MCP maps to `tool_error`.
- **The spec is structured, never SQL** (`TsQueryKind`:
  window aggregate / readings-in-window / latest-per-anchor / all-readings).
  Remote hosts render their own partial SQL from the spec via the fe-query
  builders, so a distributed request can only ever read one petal's
  readings through the sanctioned shapes — there is no SQL string to
  guard because no SQL crosses the seam.
- **Answers carry the A16 honesty metadata** (`covered_shards` /
  `missing_shards` / `answered_hosts` in the outcome meta): the `/query`
  surface echoes it verbatim so a caller can tell a genuinely empty
  window from a departed host's shard; the analytics merged table is only
  registered when the caller's SQL references `iot_reading`
  (`sql_references_table`, whole-identifier word-boundary scan) — a
  nodes-only analytics query never pays a fan-out.
- Row caps: the API requests `DISTRIBUTED_QUERY_ROW_CAP` (0 = transport
  default); the executing host clamps anything it receives anyway, so the
  API layer never widens a cap. Answer deadline
  `DISTRIBUTED_QUERY_TIMEOUT_MS` (8 s) is bounded by the transport's own
  `MAX_QUERY_TIMEOUT_MS` and generous over the sync-side 3 s default so a
  slow fleet still answers within the HTTP budget.
- Tests: `tests/distributed_query_test.rs` (11) — the three surfaces'
  happy paths, role/scope denials (seam untouched), ULID/arg validation,
  dead-seam and no-seam explicit errors, the honest-empty vs failed
  analytics table, and nodes-only analytics unaffected.
