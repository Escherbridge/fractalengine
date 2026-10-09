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
| Assets (§assets, §asset-ingest) | `GET /api/v1/assets/{content_hash}`, `GET /api/v1/assets/by-id/{asset_id}`, `GET /api/v1/nodes/{id}/asset`; upload `POST /api/v1/petals/{p}/assets` (multipart GLB) |
| Petal archive / GPX | `GET /api/v1/petals/{p}/export`, `POST …/import`, `POST …/import/gpx`, `GET …/export/gpx` |
| Terrain config | `GET\|PUT\|DELETE /api/v1/petals/{p}/terrain` |
| Tile data plane | `GET /api/v1/tiles/elevation/{id}/{z}/{x}/{y}.png?petal_id=...`, `GET /api/v1/tiles/satellite/{id}/{z}/{x}/{y}.jpg?petal_id=...`, `GET /api/v1/tilesets?petal_id=...`, `GET /api/v1/tilesets/{id}/meta?petal_id=...` |
| Field defs | `POST /api/v1/field-defs`, `GET /api/v1/field-defs/{scope}`, `PATCH\|DELETE /api/v1/field-defs/by-id/{id}` |
| Query / BI egress (§query-guard, §export, §share) | `POST /api/v1/query` (body `distributed: {…TsQueryKind…}` → §distributed-query), `POST /api/v1/query/elevated`, `POST /api/v1/query/share`, `GET /api/v1/petals/{p}/export.parquet`, `GET …/export.csv`, `POST /api/v1/analytics/query` (merged `iot_reading` table via §distributed-query) |
| IoT ingest (§iot-ingest) | `POST /api/v1/petals/{p}/iot/readings` |
| Hexon tilesets | `POST /api/v1/hexons/tilesets/install?petal_id=…`, `DELETE /api/v1/hexons/tilesets/{id}?petal_id=…`, `PATCH …/{id}/seeding?petal_id=…`, `GET /api/v1/hexons/tilesets?petal_id=…`, `GET /api/v1/hexons/storage?petal_id=…` |
| Hexon crate registry | `POST /api/v1/crates/publish`, `POST /api/v1/crates/{uri}/install?petal_id=…`, `DELETE …/{uri}/uninstall?petal_id=…`, `GET /api/v1/crates/search?petal_id=…`, `GET /api/v1/crates/installed?petal_id=…`, `GET /api/v1/crates/{uri}?petal_id=…`, `GET …/{uri}/entries?petal_id=…`, `GET …/{uri}/entries/{entry_id}/asset?petal_id=…`, `GET /api/v1/crates/available?petal_id=…` |
| Sim control (§sim-control) | `POST /api/v1/sim/start`, `POST /api/v1/sim/stop`, `GET /api/v1/sim/status`, `POST /api/v1/sim/step`, `POST /api/v1/sim/inject-fault` |
| MCP | `POST /mcp` (`mcp::mcp_handler`) — 29 tools, one `ToolSpec` table (§mcp-dispatch); body limit raised for base64 uploads (§asset-ingest) |

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
egress path: `/api/v1/query` (+ MCP `query`), both export routes, and
shared-URL redemption, so new egress handlers cannot bypass a guard by
construction. Error strings of the original checks are preserved.

**M4 fix B1 (2026-10-09) — row scoping is FROM-substitution, not a
string append.** The old `inject_scope_filter` appended `WHERE/AND petal_id
= '…'` only when the text contained the exact substring `FROM NODE` (single
space) — the M4 review broke it four ways (double space/tab/newline → no
injection; `WHERE true OR true` → OR binds looser than the appended AND;
trailing `--` swallowed the appended clause; a projection subquery `(SELECT
* FROM iot_reading)` was never filtered) — and verse/fractal-scoped tokens
got NO filter at all. It is gone. `prepare_scoped_sql(sql, petals, mode)`:

1. reject comment tokens `--` `#` `//` `/*` anywhere, in **every** mode
   including `Query` (corrected 2026-10-09, R5: `Query` used to skip this —
   a comment is inter-token trivia the SurrealQL lexer recurses through
   just like whitespace, so `type/**/::/**/thing(...)` split the step-4
   needle scan apart and reached an unscoped record dereference via an
   allow-listed-but-unscoped `FROM verse`; `reject_record_constructors` now
   ALSO excises comment spans independently before its own needle scan, as
   a second, order-independent layer);
2. `normalize_whitespace` (runs collapsed outside strings/comments — cosmetic;
   no guard decision depends on it);
3. `validate_select_sql` — semicolon, SELECT-only, keyword blocklist, and the
   table whitelist over `from_clause_targets`, which now also rejects any FROM
   target that is not a bare identifier followed by a clause keyword / `,` /
   `)` / end. SurrealQL parses the FROM target as an EXPRESSION
   (`parse_expr_table`), so `FROM verse AND role` read `role` past the
   whitelist, as did `FROM (role)` (paren must open a SELECT) and `FROM
   (SELECT …) AND role` (string-aware paren matcher, fail-closed on comments);
4. scoped-dialect bans: escaped identifiers (`` ` `` / `⟨⟩`), record/table
   constructors (`type::thing|record|table`, `record::`, `<record>`), `r"…"`
   record strings — every way to name a record without its table name;
5. **every** whole-word `node` / `iot_reading` in the text must be a FROM
   target (lexer-free: strings and comments count — fail-closed, so
   `WHERE name = 'node'` is a false-positive rejection; record literals like
   `node:x.*` die here);
6. Egress/Export: exactly one SELECT and one FROM table (no nesting);
7. rewrite each `node`/`iot_reading` target to
   `(SELECT * FROM node WHERE <filter>)`; Export also forces the projection
   to `*`.

*Soundness (why OR/whitespace/comment tricks cannot widen it):* the user's
statement can only read the petal table through the substituted subquery,
whose text is entirely server-built: `<filter>` is `petal_id = 'P'`,
`petal_id IN ['P1', …]`, or `false`, with ids restricted to
`[A-Za-z0-9_-]` (unsafe ids are dropped, never escaped). The inserted
segment has an even number of quotes, all AFTER the contiguous
`FROM node WHERE` tokens, and no comment/backtick/bracket characters — so
whatever lexer state the user's preceding text leaves, either the whole
`FROM node WHERE petal_id …` sequence is live SQL (filtered) or the `node`
token is inside a string/comment (inert). User predicates, `OR`, `LIMIT`,
projections and subqueries all sit OUTSIDE the parens and only ever see
already-scoped rows. Trade-off: the outer WHERE/LIMIT no longer push down —
the inner subquery materializes the petal's rows (bounded by TIMEOUT +
row cap).

*Scope → allow-list (FIX 1(c)) — `resolve_scope_petals`:* petal scope →
`[petal]`; fractal scope → petals whose `fractal_id` is that fractal AND the
fractal belongs to the scope's verse; verse scope → petals under any fractal
of that verse (`petal.fractal_id` → `fractal.verse_id`; `node`/`iot_reading`
carry only `petal_id`, so this is resolved first and inlined as an IN-list —
one extra fixed-shape query, skipped entirely when the SQL reads no
petal-scoped table); unparseable scope → `[]` (deny all). Petals with no
`fractal_id` belong to no verse and are denied.

*Modes:* `Query` (`/query`, MCP): scoped subqueries allowed (each FROM is
rewritten), comments banned same as every other mode (R5, 2026-10-09 — see
step 1 above; this used to say "comments + scoped subqueries allowed," which
was the bug). `Egress` (json share): flat single SELECT, no comments.
`Export` (parquet/csv export + share): `Egress` + `*` projection.

*Row-scoped tables (corrected 2026-10-09, DEC-C19 N1):* `node`, `iot_reading`,
`petal`, `room`, `model`, `crate_registry` are all in `PETAL_SCOPED_TABLES`
and get the identical FROM-substitution row filter — each has its own
`petal_id` column (fe-database/src/schema.rs). The claim previously here —
"only `node` and `iot_reading` are row-scoped … `node_log` has only
`node_id`" — was **factually wrong**: `node_log.payload` carries the node's
`petal_id`, `name`, `position`, and `asset_id` (fe-database/src/lib.rs
§node-log append; `handlers/crud.rs`), so with `NODE_LOG` allow-listed but
*not* scoped, a P1-scoped viewer could mint a public JSON share filtered on
`payload.petal_id` and read any petal's node metadata through it — a pure
BI-egress side channel the row-scoping machinery never saw. `NODE_LOG` has
no documented `/query`/export/share consumer (`fe-api/src/format.rs::
load_export_nodes` reads it directly against `db_reader` by `node_id`, never
through this guard) and is now **removed** from `ALLOWED_TABLES`; re-add it
only behind a node-scoped subquery substitution (mirroring `node`/
`iot_reading`) if a real consumer appears. `crate_entry` carries no
`petal_id` column and stays deliberately unscoped. Function namespaces
outside the banned constructors (e.g. a future `fn::`/`api::`) are not
enumerated.

Execution goes through `run_guarded_query` / `run_guarded_query_via_state`:
**M4 fix M2** appends `\nTIMEOUT 5s` to every guarded statement (the newline
is defense in depth against a trailing line comment, though R5 means there
should never be one left by the time SQL reaches here) so the DB aborts a
heavy query and frees its thread; a 6s client timer is only a backstop.
Statement errors are surfaced via `Response::check()` — a failed or
timed-out statement no longer reads as an empty success. The FR-4 **row cap**
policy is **error, not truncate** (`row cap exceeded (limit N rows…)`);
`enforce_byte_ceiling` guards serialized size the same way. **DEC-C19 N3
(2026-10-09):** the channel-fallback branch of `run_guarded_query_via_state`
(only — never the direct `db_reader` path) is additionally gated by a
process-global `tokio::sync::Semaphore` (`EGRESS_FALLBACK_PERMITS = 4`); a
caller that can't acquire a permit within `EGRESS_FALLBACK_ACQUIRE_TIMEOUT`
(500ms) gets a clean `"egress busy"` error (export/share map it to 503). The channel
fallback is CORRELATED (M4 fix B2 — fe-runtime src/AGENTS.md
§api-reply-correlation): it sends a fresh `correlation_id`, accepts only
`QueryResult`/`QueryFailed` echoing it, and treats anything else as an error,
never as its rows.

`src/limits.rs` holds every cost knob as a named constant (plan D4):
`/query` 10 000 rows / 8 MiB; exports 500 000 rows / 128 MiB; rate limits
10/s per DID (`/query`, exports) and per token (share redemption); share TTL
default 1h / max 24h. Change limits there, nowhere else.

## §export

`src/export.rs` — `GET /api/v1/petals/:petal_id/export.parquet|export.csv`
(`?query=<urlencoded SELECT>&coords=local|latlon`), the FR-2 BI egress. Flow:
Viewer+ role → valid ULID → petal scope coverage (`resolve_petal_scope`,
deny-by-default, real HTTP statuses per the §assets precedent) →
`prepare_scoped_sql(sql, [path_petal], GuardMode::Export)` (§query-guard:
the path petal is substituted into the FROM source — FR-6 — and the
projection forced to `*`) → `run_guarded_query_via_state` (TIMEOUT 5s, row
cap) → **row-level post-filter** (M4 fix B1(a), 2026-10-09): every row whose
own `petal_id` is not the authorized petal is dropped and counted
(a `warn!` with the count when non-zero — it can only fire if the source rewrite were bypassed) — an
egress-point re-check independent of the SQL rewrite; the forced `*`
projection guarantees the real `petal_id` column is present, and aliasing
(`'P' AS petal_id`) is impossible because the projection is discarded →
rows mapped to a table-specific shape → fe-query's GeoParquet writer
(`write_nodes_parquet_bytes` / `write_readings_parquet_bytes`, both
in-memory; no temp files) or local CSV serialization.

- **Export dialect (M4 fix B1(b)).** Export/share SQL must be ONE flat
  SELECT over ONE table: comment tokens (`--`, `#`, `//`, `/*`) anywhere —
  string literals included, so `'https://…'` is rejected — nested SELECTs and
  comma FROM-lists are 400s, at mint time and again at redemption (old
  tokens carrying such SQL now fail with 400).

- Export queries target **NODE or IOT_READING only** (400 otherwise, message
  shared with §share's mint-time check via `export::classify_export_table`):
  the output schema is the snapshot/GeoParquet nodes table or the flat
  readings table; every other table belongs to `/query`.
- **Readings shape (A23, F11).** `iot_reading` has no geometry column (schema
  `src/AGENTS.md` §iot-readings in fe-database), so each reading row
  (`reading_id, node_id, petal_id, metric, value, units, recorded_at,
  recorded_at_ms`) is paired with its anchor node's position via a
  **deterministic second guarded query** (`fetch_anchor_positions`): collect
  the distinct `node_id`s the page of readings touches, then resolve all of
  them in ONE `SELECT node_id, position, elevation FROM node WHERE petal_id =
  $pid AND node_id IN $ids` (bound, not string-interpolated — the ids are
  server-derived ULIDs, not caller SQL, so it bypasses `validate_select_sql`
  but still rides `run_guarded_query` for the same timeout/row-cap/error
  shape). This is Rust-side batching, never an N+1 per-row subquery (distinct
  ids via a `HashSet`, M4 minor #5). A reading
  whose anchor no longer resolves (hard-deleted node, or a row without a
  readable `position` — M4 minor #6: never defaulted to 0.0; not merely tombstoned —
  the join does NOT filter by `tombstone`, so a reading anchored to a
  tombstoned-but-still-present node keeps its last known position) maps to
  `anchor_position: None`, which the parquet writer encodes as a **null**
  geometry cell (unlike the nodes writer, whose geometry column is
  non-nullable) rather than fabricating `[0,0,0]`. `value` is `Float64` end to
  end (reading rows, the Arrow column, and CSV) — unlike node positions'
  `Float32` — so BI consumers get the sensor value back bit-exact.
- Parquet responses ship `Content-Type: application/vnd.apache.parquet`,
  `Content-Length` (axum), and `Accept-Ranges: bytes` so DuckDB httpfs can
  `read_parquet('<url>')` (plan D1).
- **Windows `db_reader` fallback (F10, 2026-10-09).** `prepare_export` and
  `fetch_anchor_positions` route through
  `query_guard::run_guarded_query_via_state`, which prefers the direct
  `db_reader` and falls back to the `DbCommand::RawQuery` gateway channel
  when it is `None` — the same fallback `gis.rs::run_select` already uses.
  This closes a real gap verified live: on the deployment platform
  (Windows), SurrealKV's per-handle file lock (`os error 33`,
  M1/F4-documented) routinely leaves `db_reader` `None` while the DB
  thread's writer connection is alive, so every export/share request used
  to 503 with `"export endpoint not available (no db_reader)"` — A22's
  e2e could not pass without this fix. The channel path re-applies the
  same TIMEOUT + row cap as the direct path and is correlated (M4 fix B2 —
  before it, a timed-out caller's late rows or a GUI Query-tab result could
  be handed to the next public share redeemer); `RawQuery`'s own SELECT-only guard rail
  (`fe-database/src/lib.rs`) re-validates independently as defense in
  depth. `/api/v1/query` (`rest.rs::execute_query`) and the `fmt=json`
  branch of share redemption still require `db_reader` directly and were
  deliberately left alone here (same root cause, larger blast radius —
  noted as a follow-up, not fixed in F10).
- **Range support (DEC-C10, F10, 2026-10-09).** `Accept-Ranges: bytes` used
  to be advertised with zero backing (grep found no Range/206 handling
  anywhere in `fe-api`). Verified live against the real `tools/duckdb`
  CLI (`scripts/bi-egress-verify.ps1` / `.log`): DuckDB's httpfs parquet
  reader DOES issue real `Range` GETs against `read_parquet()` URLs
  (observed: a near-full-file range for the initial read plus smaller
  footer/metadata sub-ranges) — this was not a cosmetic gap. `body_response`
  now parses a single `Range: bytes=a-b` (open-ended and suffix forms
  included) and answers `206` with a sliced body + `Content-Range`;
  `start` at or past EOF answers `416` with `Content-Range: bytes */total`;
  a comma-separated multi-range request is declined (falls back to the
  full `200` body, which RFC 7233 permits). Bodies are already fully
  buffered `Vec<u8>` (capped at `EXPORT_MAX_BYTES`), so the slice is free.
  Covered by `export.rs`'s `body_response_*` / `range_*` unit tests.
  `bytes=-0` (empty suffix) is also 416 (M4 minor #10).
- **Validators + caching (DEC-C18, M4 fix M1, 2026-10-09).** A22's
  "immutable cache" expectation is amended: export/share bodies are LIVE
  query output, so they carry `Cache-Control: private, no-store` (json share
  responses too) — `immutable` stays on the content-addressed assets route
  only. Every body (200, 206, 416) carries a strong `ETag` = quoted blake3
  hex of the exact full body. `If-Range` is honored: a validator equal to the
  current ETag serves the range (206); anything else (stale ETag, an
  HTTP-date — we send no `Last-Modified`) downgrades to the full 200, so a
  reader re-fetching ranges across a live write gets a consistent whole body
  instead of stitched slices of two result sets. Tests:
  `etag_is_strong_stable_and_body_derived`,
  `if_range_match_serves_range_mismatch_serves_full_body`,
  `export_etag_and_if_range_round_trip`.
- CSV is RFC-4180 with a leading `# crs=<label>` comment line (documented
  choice: comment line + `X-FE-CRS` header; a sidecar column would bloat every
  row) and **properties as one JSON-string column** (flattening arbitrary keys
  would make the header schema query-dependent). Readings CSV mirrors this:
  `reading_id,node_id,petal_id,metric,value,units,recorded_at,recorded_at_ms,`
  + the same local/latlon anchor-position column split as nodes (blank fields
  when the anchor does not resolve).
- `coords=latlon` converts through the petal `Projection` at the API layer
  (never in fe-query/fe-database); position becomes `[lon, lat, ele]`
  (GeoParquet EPSG:4326 axis order) / `lon,lat,ele_m` CSV columns — applied to
  the anchor position on the readings path too. 400 when the petal has no
  terrain origin. The GeoParquet `geo` metadata OMITS the spec `crs` key for
  latlon output (absent = OGC:CRS84, exactly our lon/lat order — M4 minor
  #7) while petal-local output keeps `crs: null` (DEC-C16); `fe:crs` carries
  the label in both. Precision note: parquet positions pass through
  `EntitySnapshot`'s `f32` (≈1 m at mid-latitudes) — acceptable v1, revisit if
  survey-grade egress is needed.
- Status mapping: 400 bad query/coords/table, 403 role/scope, 404 unknown
  petal, 413 row-cap/byte-ceiling (enforced on the readings path too — the
  anchor join is bounded by the already-capped distinct-node-id count, and
  the byte ceiling is checked against the final serialized body exactly as
  for nodes), 416 Range past EOF (DEC-C10), 429 rate limit, 502 query
  transport (now also covers a dead `api_cmd_tx` channel on the Windows
  fallback path, F10), 504 statement timeout. No more 503 "no db_reader" —
  closed by the F10 channel fallback above.

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
- **Key lifetime (A24, DEC-C9) — restated honestly 2026-10-09 (M4 fix M3).**
  `ApiState.share_signer` is a required `ApiConfig` field
  (`fe-api/src/lib.rs`); both binaries build it with
  `fe_identity::load_or_generate_keypair(&secret_store, "share_signer")`.
  What that actually guarantees differs per binary:
  - **GUI** (`fractalengine/src/main.rs`, `OsKeystoreBackend`): the generated
    seed is written to the OS keystore — share links survive restarts.
  - **Relay** (`fractalengine-relay/src/main.rs`, `EnvBackend`, slot
    `FE_SECRET_FRACTALENGINE_SHARE_SIGNER`): links survive a restart ONLY if
    the operator exports that 64-hex seed. `EnvBackend::set` stores
    in-process only, so when the var is unset the relay silently generated
    an ephemeral key (the "could not load/store" fallback arm never fires).
    The relay now `warn!`s at startup when the var is absent: every link it
    mints will 401 after a restart. Pinned by fe-identity
    `env_backend_share_signer_round_trips_across_restarts` (two fresh
    `EnvBackend`s load the same key from the var; without it, two different
    ephemeral keys). The earlier F11 "persistence" test shared one
    `Arc<NodeKeypair>` between two harnesses — it proves verify-with-same-key,
    not persistence.
  **Deliberately a dedicated slot, not derived from the node identity seed**:
  share-URL signing is a different capability domain — independent rotation,
  and a leaked share-signing key must never double as node impersonation.
  `ApiHarness` (fe-test-harness) now defaults to a DISTINCT generated
  keypair (DEC-C9 separation holds in tests too); `spawn_with_share_signer`
  lets a test share one key across two harness instances.
- **Caching:** json redemptions also send `Cache-Control: private, no-store`
  (DEC-C18); parquet/csv redemptions inherit §export's ETag/If-Range.
- **Windows (no `db_reader`):** `fmt=parquet|csv` redemptions work through the
  correlated channel fallback; `fmt=json` (like `/api/v1/query`) still
  answers 503 `shared query not available (no db_reader)` — unchanged.

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
DEC-C16: in the GeoParquet `geo` footer metadata (fe-query §geoparquet) this
same label lands in the custom `fe:crs` key, not the spec `crs` key (which is
always `null`) — the x-fe-crs header and CSV `# crs=` line are unaffected.

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
- ~~Share-URL signing key persistence (§share)~~ **CLOSED (A24, DEC-C9,
  F11).** `ApiConfig.share_signer` is now a required field both binaries
  populate from their secret store; see §share's "Key lifetime" entry.

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
- **Reply correlation (M3-review fix 3, 2026-10-09)**: `InsertIotReadings`
  and both reply variants carry a `correlation_id` (the CreateNode
  precedent); the fallback match verifies the echoed id + `petal_id` and
  answers 502 on mismatch, and `deliver_correlated` (fe-runtime app.rs) lets
  a timed-out caller's dead entry CONSUME its late reply so the next caller
  never receives another client's count or 422 detail. The family-blind
  `Error` routing + skip-closed sharp edge remains — see fe-runtime
  `src/AGENTS.md` §api-reply-correlation before adding any new
  timeout-capable write-path caller.
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
- **Egress seam (FR-5) — CLOSED (A23, F11)**: `iot_reading` is whitelisted in
  `query_guard::ALLOWED_TABLES` and `prepare_scoped_sql` substitutes the
  petal filter into every `FROM iot_reading` source (rows carry a
  denormalized `petal_id`; M4 fix B1 replaced the old `inject_scope_filter`), so
  `/api/v1/query` + shared-URL redemption serve IoT rows scope-guarded.
  Reading-shaped `export.parquet`/`export.csv` (flat reading rows + the
  anchor-position batched join) now ships too — see §export.

## §distributed-query (M2/F7 — A15/A16/A17)

`src/timeseries_query.rs` — the ONE guarded bridge every distributed
timeseries surface shares: `POST /api/v1/query` with body
`{"distributed": {…TsQueryKind…}}` — the spec object directly (not a
`{"distributed": true, "query": {...}}` wrapper); `sql` must be empty, the
two are mutually exclusive (`QueryRequest`, `types.rs:269-282`; dispatch
`rest.rs:872-882`) — the analytics
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
- Tests: `tests/distributed_query_test.rs` (12) — the three surfaces'
  happy paths, role/scope denials (seam untouched), ULID/arg validation,
  dead-seam and no-seam explicit errors, the honest-empty vs failed
  analytics table, and nodes-only analytics unaffected.

## §sim-control (F9/A20 — DEC-C7)

`src/sim.rs` — the simulation-lab control surface: REST `/api/v1/sim/*`
(start / stop / status / step / inject-fault) and the MCP `sim_*` tools.
One dispatch fn per verb (`sim::start` … `sim::inject_fault`); the REST
handlers and the MCP arm both call them, so the guard lives once.

- **Guard order: Owner role → seam presence → arguments.** Owner because a
  session is process-level (it overrides the host's HLC source — fe-sim
  §session); the check is scope-less (an Owner of ANY scope passes — the
  `hexon.rs::publish_crate` template). A non-owner always sees 403, never a
  400 that leaks validation detail; tests pin that a refused call never
  reaches the seam.
- **Fail closed off a lab host.** `ApiState.sim_control_tx` is `None` on
  every default build (GUI permanently; relay unless built with
  `--features sim-control`) → 503 "sim control not configured".
- **Seam** = `fe_runtime::sim_control` (`SimControlCall { cmd, reply_tx }`,
  JSON payloads — fe-api never depends on fe-sim). Reply-embedded call →
  `try_send` (Full → 503 busy, Disconnected → 502), reply awaited via
  `spawn_blocking(recv_timeout(SIM_CONTROL_TIMEOUT = 120s))` → 504. The
  bridge is serial: a long `step` (scripted query deadlines are real time)
  delays the next call, and an HTTP client with a shorter timeout gives up
  while the step still completes.
- **Status mapping** (REST; MCP returns `tool_error` text): bridge
  `BadRequest` 400, `Conflict` (session already running) / `NotRunning` 409,
  `Failed` (session torn down) 500. Envelope is the house `ApiResponse`.
- `step.n` is validated API-side (`1..=MAX_SIM_STEP`), so a bad count never
  queues. `start.script` accepts a ScenarioScript object, its JSON text, or a
  built-in name (`default` | `sharded_query` | `offline_degraded`).
- Tests: `tests/sim_control_test.rs` owns the receiver (a one-session
  emulator) through the full router; the real bridge/session is proven in
  fe-sim (`control.rs` / `session.rs` tests). The five MCP `sim_*` tools
  are `{Global, Owner}` rows in the §mcp-dispatch table (F13); the per-verb
  guard stays as defense in depth.

## §mcp-dispatch (F13/A26 — DEC-C14, DEC-C15; 2026-10-09)

`src/mcp/mod.rs` owns ONE table, `TOOLS: &[ToolSpec]`
(`{name, description, input_schema, min_role: RoleLevel, scope_rule, handler}`);
`tools/list` and `tools/call` both derive from it. `tools/call` =
lookup → `authorize` → schema-`required` check → handler. `authorize` is the
**only** MCP authz code: typed role floor (`require_role_level` — a string
role typo would parse to `RoleLevel::None` = admit-all, so the table is
typed and a test forbids a `None` floor), then the row's `ScopeRule`
resolved **from the DB**, then token containment. Unresolvable → deny
("petal/node/fractal/verse not found"). Handlers (`src/mcp/tools.rs`) take
the resolved scope and contain no `require_*`/`resolve_*_scope` calls —
grep-tested (`handlers_contain_no_authz_calls`). Shared cores they call
(sim verbs, hexon/tileset cores, query_guard) keep their own guards as
defense in depth.

| ScopeRule | Meaning | Rows |
|---|---|---|
| `None` | self-filtering read (token scope narrows output) | get_hierarchy, query |
| `Global` | no resource scope exists — role-only by design | create_verse (Manager), sim_* (Owner) |
| `PetalArg(k)` | `args[k]` petal → `resolve_petal_scope` | promote_instance, query_timeseries, upload_asset, place_asset, create_waypoint, import_gpx, set_petal_terrain, list_tilesets, install_tileset (Manager), get_gis_nodes |
| `NodeArg(k)` | `args[k]` node → `resolve_node_scope` | update_transform, read_node, node_address, delete_node, set/get/delete_property, move_waypoint |
| `HierarchyArgs(target)` | the id AT `target` (Verse/Fractal/Petal) is DB-resolved; shallower ids sent must match the stored chain; a DEEPER id is rejected outright | create_fractal→Verse, create_petal→Fractal, create_node→Petal |

Role floors: Viewer for reads, Editor for writes, Manager for create_verse +
install_tileset (DEC-C14; REST install is Editor + fe-policy `Install` — the
MCP row is deliberately stricter), Owner for sim_*.

**Wart history (fixed F13).** create_node / create_petal used to scope-check
only when the caller supplied ancestry ids (omit them → role-only), and
update_transform had no scope check. All three now resolve the write
target's real scope from the DB; the role-only fallbacks are deleted.
`HierarchyArgs`' match rule closes the decoy variant (authorize against an
owned petal, write into a foreign fractal). Handlers write into the ids
taken from the resolved scope, never raw args.

**Depth-escalation hardening (security review, 2026-10-09).** The ancestry
check alone did not stop a DEPTH escalation within one's own chain: a
petal-scoped Editor token could send `{verse_id, petal_id: <own petal>}` to
create_fractal — the old rule anchored on the *deepest* id present
(`petal_id`), which the token legitimately covers, then let the handler
write a verse-level fractal anyway. The fix anchors authz at the EXACT
level the tool writes into (`HierarchyArgs(HierarchyTarget)`): create_fractal
resolves only `verse_id` and rejects a present `fractal_id`/`petal_id`
outright; create_petal resolves only `fractal_id` and rejects a present
`petal_id`; create_node is unchanged (`petal_id` already was the target).
Ids ABOVE the target (e.g. `verse_id` on create_petal) still must match the
resolved chain — that ancestry check is kept — but nothing deeper than the
target is ever accepted, so the token's scope containment check
(`require_scope`) runs against the real write target, not a shallower
decoy.

**Honest arithmetic (DEC-C14).** A26's "20 tools" = the mcp_scene_primitives
20-name vocabulary, now fully present. `tools/list` returns 29: 24 non-sim +
5 sim — an intentional, documented superset (read_node, node_address,
promote_instance, query_timeseries, sim_* shipped legitimately); none are
deleted or renamed.

**Known gaps (F14 / follow-ups).** `set_petal_terrain` validates then
refuses exactly like REST PUT/DELETE terrain (`SetPetalTerrain` replies are
uncorrelated — fe-runtime §api-reply-correlation). `query` inherits
`query_guard`'s verse/fractal-scoped tokens getting NO row filter
(petal-scoped tokens are filtered). `place_asset` accepts any existing
`asset_id` (asset rows are node-global — §assets caveat).

## §asset-ingest (F13 — mcp_scene_primitives FR-1/2/3/7)

`src/upload.rs`. Both transports (REST multipart
`POST /api/v1/petals/{p}/assets`, MCP `upload_asset` base64) share
`ingest_glb_with_limit`: name check → `blob_store` present (else fail
closed: REST 503 / MCP error) → `spawn_blocking { base64 decode (length
pre-checked so oversize never allocates) → validate_glb → BLAKE3 +
add_blob }` → `DbCommand::CreateAsset {name, content_type, size_bytes,
content_hash, correlation_id}` → correlated `DbResult::AssetCreated`.

- **Bytes never cross the channel — by construction.** `store_glb` consumes
  the `Vec<u8>` and returns `StoredGlb` (private fields: hash + size only);
  `register_asset`, the sole `CreateAsset` sender, accepts only `StoredGlb`.
  The DB thread (`create_asset_handler`) re-verifies `has_blob` and writes
  `data: NONE`. One `BlobStoreHandle` is shared by ApiState and the DB thread
  (`fractalengine/src/main.rs`), so the hash is immediately servable.
- **Validation (FR-7):** GLB only — `glTF` magic, version 2, header length ==
  byte length, ≤ `MAX_ASSET_BYTES` (256 MiB, per-node config is a seam).
  Embedded textures are an uploader requirement, not parsed.
- **Limits (NFR-2):** asset route `DefaultBodyLimit` = 256 MiB + 1 MiB;
  `/mcp` = ceil(4/3 × 256 MiB) + 8 MiB. A max-size MCP upload transiently
  holds ~600 MB (body + decode) — accepted for a desktop node. Other routes
  keep axum's 2 MB default. GPX via MCP caps at 16 MiB decoded.
- **Placement:** `place_asset` → `DbCommand::CreateNodeWithAsset` (log-first
  `NodeCreated` op, rotation stored as Euler XYZ like `UpdateNodeTransform`)
  → `DbResult::GltfImported` with an echoed `correlation_id`. Only a
  correlated `GltfImported` is an API reply (`ReplyKind::PlacedAsset`); a GUI
  `ImportGltf` result (`None`) can never satisfy an API waiter, and the GUI's
  existing listener still spawns the placed node live.
- Re-uploading identical bytes is idempotent at the blob layer but creates a
  new asset row (dedupe-by-hash is out of scope).
