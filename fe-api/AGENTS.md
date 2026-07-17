# fe-api — API gateway (REST + WS + MCP)

Runs on its own multi-thread tokio runtime (`spawn_api_thread`), talking to the
Bevy/DB threads over `crossbeam::channel` (`ApiCommand` -> `DbCommand` ->
`DbResult`, matched via `tokio::sync::oneshot` per request — see
`fe-runtime/src/app.rs` `drain_api_commands` + `PendingApiRequests`, read-only
from here). Where a `db_reader: Option<Arc<Surreal<Db>>>` is configured,
handlers may bypass that round-trip entirely with a direct SurrealDB query
(`direct_*` helpers in `rest.rs`/`assets.rs`) — this is the established escape
hatch for reads that don't have (or don't need) a dedicated `DbCommand`.

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
| GET | `/api/v1/tiles/elevation/{tileset_id}/{z}/{x}/{y_png}` | `terrain::get_elevation_tile` |
| GET | `/api/v1/tiles/satellite/{tileset_id}/{z}/{x}/{y_jpg}` | `terrain::get_satellite_tile` |
| GET | `/api/v1/tilesets` | `terrain::list_available_tilesets` |
| GET | `/api/v1/tilesets/{tileset_id}/meta` | `terrain::get_tileset_meta` |
| GET | `/api/v1/shared/{token}` | `share::redeem_share_url` (§share — the signature is the credential) |

**Authenticated** (Bearer JWT via `auth::auth_middleware`):

| Area | Routes |
|------|--------|
| Hierarchy CRUD | `GET /api/v1/hierarchy`; `POST /api/v1/verses`, `POST …/verses/{v}/fractals`, `POST …/fractals/{f}/petals`, `POST …/petals/{p}/nodes`; legacy flat `POST /api/v1/nodes` |
| Node ops | `PATCH\|GET /api/v1/nodes/{id}/transform`, `PATCH\|GET …/properties`, `DELETE …/properties/{key}`, `PATCH /api/v1/nodes/{waypoint_id}/move`, `GET /api/v1/nodes/{track_id}/elevation-profile`, `GET …/stats` |
| Waypoints | `POST /api/v1/petals/{p}/waypoints` |
| GIS reads (§gis) | `GET /api/v1/petals/{p}/gis/nodes`, `GET …/gis/tracks` |
| Assets (§assets) | `GET /api/v1/assets/{content_hash}`, `GET /api/v1/assets/by-id/{asset_id}`, `GET /api/v1/nodes/{id}/asset` |
| Petal archive / GPX | `GET /api/v1/petals/{p}/export`, `POST …/import`, `POST …/import/gpx`, `GET …/export/gpx` |
| Terrain config | `GET\|PUT\|DELETE /api/v1/petals/{p}/terrain` |
| Field defs | `POST /api/v1/field-defs`, `GET /api/v1/field-defs/{scope}`, `PATCH\|DELETE /api/v1/field-defs/by-id/{id}` |
| Query / BI egress (§query-guard, §export, §share) | `POST /api/v1/query`, `POST /api/v1/query/elevated`, `POST /api/v1/query/share`, `GET /api/v1/petals/{p}/export.parquet`, `GET …/export.csv`, `POST /api/v1/analytics/query` |
| IoT ingest (§iot-ingest) | `POST /api/v1/petals/{p}/iot/readings` |
| Hexon tilesets | `POST /api/v1/hexons/tilesets/install`, `DELETE /api/v1/hexons/tilesets/{id}`, `PATCH …/{id}/seeding`, `GET /api/v1/hexons/tilesets`, `GET /api/v1/hexons/storage` |
| Hexon crate registry | `POST /api/v1/crates/publish`, `POST /api/v1/crates/{uri}/install`, `DELETE …/{uri}/uninstall`, `GET /api/v1/crates/search`, `GET /api/v1/crates/installed`, `GET /api/v1/crates/{uri}`, `GET …/{uri}/entries`, `GET …/{uri}/entries/{entry_id}/asset`, `GET /api/v1/crates/available` |
| MCP | `POST /mcp` (`mcp::mcp_handler`) |

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
- `fractalengine/src/main.rs` currently passes `blob_store: None` into
  `fe_api::ApiConfig` (a separate `FsBlobStore` handle is only wired to Bevy,
  not the API thread) — so on the actual GUI binary today, every asset
  endpoint (old and new) hits the `blob_store` `None` branch and returns 503,
  even though `db_reader` is wired and asset *metadata* queries would
  succeed. Wiring `blob_store: Some(blob_store.clone())` in `main.rs` (the
  handle is already `Clone`) closes this gap.
- A `DbCommand::GetNodeAsset { node_id }` / `DbCommand::GetAssetMeta
  { asset_id }` variant (returning name/content_type/size_bytes/content_hash)
  would let these endpoints work over the crossbeam channel when no
  `db_reader` is configured (e.g. a future relay-only deployment). Today they
  return 503 in that case, same as `blob_store` being absent.
- Share-URL signing key persistence (§share): pass the node's `NodeKeypair`
  into `ApiConfig`/`run_server` from `fractalengine/src/main.rs` so shareable
  links survive a restart; today the key is ephemeral per process.

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
  `fe_database::handlers::iot_reading::insert_readings`, not through a
  `DbCommand` round-trip: readings are append-only facts with no derived
  counters, so the DB-thread single-writer invariant doesn't apply (rationale
  in fe-database `src/AGENTS.md` §iot-readings), and IoT-frequency batches
  must not queue behind the render loop's channel.
- **Validation failures map to real statuses**: unknown/foreign-petal anchor,
  bad RFC-3339 timestamp, or empty metric → 422 (typed `IotIngestError`, no
  string-sniffing); DB failure → 502; empty batch → 400.
- **Egress seam (FR-5)**: `iot_reading` is whitelisted in
  `query_guard::ALLOWED_TABLES` and `inject_scope_filter` injects the petal
  filter on `FROM iot_reading` (rows carry a denormalized `petal_id`), so
  `/api/v1/query` + shared-URL redemption serve IoT rows scope-guarded today.
  Reading-shaped `export.parquet`/`export.csv` (flat reading rows + optional
  anchor-position join) is the remaining FR-5 polish — `prepare_export` is
  still node-table-only.
