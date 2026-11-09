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
hierarchical scope, require token containment, then subscribe or mutate (an
unknown and an out-of-scope target answer the same `not_found` /
`NOT_FOUND_OR_DENIED` error — §existence-oracle). A
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
| Terrain config (§terrain-mutation) | `GET\|PUT\|DELETE /api/v1/petals/{p}/terrain` |
| Render distance (§render-distance) | `GET\|PUT\|DELETE /api/v1/petals/{p}/render-distance` |
| Visibility Groups (§visibility-groups) | `GET\|PUT /api/v1/petals/{p}/visibility-groups`, `PATCH\|DELETE …/visibility-groups/{group_id}` |
| Tile data plane | `GET /api/v1/tiles/elevation/{id}/{z}/{x}/{y}.png?petal_id=...`, `GET /api/v1/tiles/satellite/{id}/{z}/{x}/{y}.jpg?petal_id=...`, `GET /api/v1/tilesets?petal_id=...`, `GET /api/v1/tilesets/{id}/meta?petal_id=...` |
| Field defs | `POST /api/v1/field-defs`, `GET /api/v1/field-defs/{scope}`, `PATCH\|DELETE /api/v1/field-defs/by-id/{id}` |
| Query / BI egress (§query-guard, §export, §share) | `POST /api/v1/query` (body `distributed: {…TsQueryKind…}` → §distributed-query), `POST /api/v1/query/elevated` (§elevated-query — OFF unless `FE_API_ELEVATED_ALLOW=1`; Owner; NOT scope-confined), `POST /api/v1/query/share`, `GET /api/v1/petals/{p}/export.parquet`, `GET …/export.csv`, `POST /api/v1/analytics/query` (merged `iot_reading` table via §distributed-query) |
| IoT ingest (§iot-ingest) | `POST /api/v1/petals/{p}/iot/readings` |
| R5 device formats / payloads (§device-ingest) | `POST\|GET /api/v1/verses/{v}/device-formats`; `POST\|GET /api/v1/petals/{p}/iot/payloads` (multipart ingest / list) |
| Hexon tilesets | `POST /api/v1/hexons/tilesets/install?petal_id=…`, `DELETE /api/v1/hexons/tilesets/{id}?petal_id=…`, `PATCH …/{id}/seeding?petal_id=…`, `GET /api/v1/hexons/tilesets?petal_id=…`, `GET /api/v1/hexons/storage?petal_id=…` |
| Hexon crate registry | `POST /api/v1/crates/publish`, `POST /api/v1/crates/{uri}/install?petal_id=…`, `DELETE …/{uri}/uninstall?petal_id=…`, `GET /api/v1/crates/search?petal_id=…`, `GET /api/v1/crates/installed?petal_id=…`, `GET /api/v1/crates/{uri}?petal_id=…`, `GET …/{uri}/entries?petal_id=…`, `GET …/{uri}/entries/{entry_id}/asset?petal_id=…`, `GET /api/v1/crates/available?petal_id=…` |
| Sim control (§sim-control) | `POST /api/v1/sim/start`, `POST /api/v1/sim/stop`, `GET /api/v1/sim/status`, `POST /api/v1/sim/step`, `POST /api/v1/sim/inject-fault` |
| Agent screenshot (§screenshot) | `GET /api/v1/debug/screenshot` → `image/png` (byte-identical, unchanged); `POST` same path → dynamic posed capture (window/scene, 5 pose kinds), PNG + `X-FE-Capture-Pose` header — ON unless `FE_API_SCREENSHOT_ALLOW=0`; Owner; 1/s per DID; scope-confined to the ACTIVE verse (uniform 404 otherwise) |
| Agent camera read-back (§camera-state) | `GET /api/v1/debug/camera` — live pose/active hierarchy/projection; same opt-in as screenshot; Owner; 10/s per DID |
| Agent input injection (§agent-input-surface) | `POST /api/v1/debug/input` — synthetic pointer/keyboard/scroll, optional `capture_after` PNG (which must also clear the screenshot gate); OFF unless `FE_API_INPUT_ALLOW=1` (mutates state, unlike the two rows above); Owner; 5/s per DID; scope-confined to the ACTIVE verse; refused 409 while a posed capture holds the camera |
| Agent token bootstrap (§qa-dev-token) | Not a route: the GUI writes loopback-only API tokens (Owner where the node owns the verse) for every local verse to `FE_API_DEV_TOKEN_OUT` once the API listens; unset = off |
| MCP | `POST /mcp` (`mcp::mcp_handler`) — 49-row `ToolSpec` table: 36 always advertised, `screenshot`/`get_camera` only on a GUI host (either capture bridge wired), `inject_input` only when its own input bridge is wired, the 5 QA-debug tools (`ui_tree`, `ui_state`, `ui_logs`, `dialog_answers`, `window`) only when the QA-debug seam is wired (GUI host), the 5 `sim_*` only on a sim lab host (§mcp-dispatch; the table's `requires` column and the length assert in `mcp/mod.rs` are the source of truth); 16 MiB body cap + 8-way concurrency limit (§asset-ingest) |

## §existence-oracle (REST/WS denial unification, 2026-10-10)

**Contract.** Any handler that resolves a resource's scope FROM THE DB and then
checks token containment must make "unresolvable" and "out of scope"
indistinguishable — identical body text, `rest::NOT_FOUND_OR_DENIED` ("not
found or not permitted"), AND identical status. Otherwise a verse-A token can
probe whether any ULID exists in verse B (the old split: "could not resolve
petal scope"/404 vs "insufficient scope"/403). Status follows the handler's
convention: house-style handlers answer 200 + `{ok:false, error}`; real-status
handlers (export, gis, gpx export, iot, upload, `.hexon` export, tile plane)
answer 404 for both (the §assets precedent). MCP had this already (DEC-C21).

**One core, no hand-rolled pairs.** `rest::covered_scope(claims, resolved)`
is the only containment-after-resolution step; `rest::authorized_petal_scope`
/ `authorized_node_scope` = resolve + `covered_scope`, returning the scope or
`None` (= the uniform denial). REST handlers, `hexon::require_hexon_petal_access`
(tileset mgmt, crate registry, tile plane), `timeseries_query`, the analytics
query, WS subscribe/scene-subscribe/transform/entity commands and the MCP
dispatcher all go through it — a new route that pairs `resolve_*_scope` with
its own `require_scope` and a second message is the regression to look for.
A resolver transport failure (DB error, channel closed, timeout) is also
`None`: it answers the same denial whether or not the id exists, so a flaky
backend never becomes an oracle (post-authz transport errors keep their
502/503/504 — only an authorized caller reaches them).

**Kept distinct** (they reveal nothing stored): role failures ("insufficient
permissions" / 403 — checked before resolution), input shape ("invalid
petal_id", bad params, 400), and the tile plane's "tileset not bound to YOUR
petal" 404 (only reachable after the caller's own petal is authorized).
Caller-supplied scopes with no DB lookup (`POST /field-defs`, `GET
/field-defs/{scope}`, `GET /address?uri=`) carry no oracle but use the same
text for consistency. `POST /query/share` checks only the token's own scope.

**Second ids inside an authorized petal** (adversarial-review pass,
2026-10-10). Authorizing the URL petal is not enough when the request carries
ANOTHER id the handler then acts on. `promote_instance` (REST + MCP) takes a
`path_id`: the DB handler tolerates an absent path but refuses a foreign one,
so "promoted" vs "failed" told a verse-A editor whether any ULID was a node in
verse B. `endpoint::path_in_petal` now requires the path to be a node OF the
authorized petal — unknown and foreign both deny with the one text before any
`DbCommand`. `GET /petals/{p}/nodes?kind=stamp&path_id=` used
`fe_query::stamps_on_path_sql(path_id)` with no petal filter, which RETURNED
another verse's stamp rows (a cross-tenant read, not just an oracle); the
builder now takes `(petal_id, path_id)` and filters on both.

**Test.** `tests/existence_oracle_test.rs` drives every affected route family
through the real router with a verse-A Owner token, asking for (i) a
never-existing ULID and (ii) a real verse-B resource, and requires
byte-identical (status, body) that is a denial. Reverting any one route to a
split message turns it red. Second-id rows put the probe inside the caller's
OWN petal (`{pa}`): promote must deny both alike; the `?path_id=` listing must
answer both alike (an empty list, `SAME_ANSWER_ROUTES`).
`own_petal_path_still_promotes_and_lists_its_stamps` guards against
over-denial; `tests/ws_integration.rs::ws_unknown_and_foreign_ids_get_identical_denials`
covers the WS messages; `mcp_integration.rs::promote_instance_unknown_and_foreign_paths_are_indistinguishable`
the MCP tool. `scripts/hexon-serve-verify.ps1` asserts the same on a live
relay for the tile plane (foreign = unknown petal = 404).

**Residuals (not closed here).** Timing: the scope lookups are now indexed
point reads (`idx_petal_id` / `idx_fractal_id` / `idx_node_id` /
`idx_verse_id`, fe-database AGENTS §schema-macro), so latency no longer grows
with table size. What remains is the second-hop asymmetry: an unknown petal
does 1 indexed lookup, a foreign petal 2 (petal → fractal), a sub-ms
difference. The fixed-cost resolver that would erase it is still deferred. Tileset store
mutation still says "tileset remains assigned to another petal" — inherent to
refusing a shared-store delete (the caller can read its own binding), and a
tileset-id fact, not a hierarchy-ULID one. The crate registry is a node-wide
catalog: `install_crate` reveals whether a crate URI is published at all.
**Open follow-up (recorded 2026-10-11, SYNC-NSID-CAP):** `/query` still serves
`verse` and `fractal` rows across ALL verses on the node (names, `created_by`,
`created_at`, `default_access`) — only `namespace_id` and the verse record id
are hidden (§query-guard). Verse/fractal scope filtering (a verse-scoped
FROM-substitution mirroring the petal one, keyed on the token's verse) is a
separate existence-oracle item, not done here.

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
  `POST …/paths/{path}/instances/{i}/promote` through `PromoteInstance` (the
  path must be a node of the URL petal — `path_in_petal`, §existence-oracle). The API
  is a *client* of the same ops the UI uses.
- **Auth model (FR-3/FR-4, Q-2).** API/MCP callers are
  `CallerAuth::Identified { did, role }` — the token's already-resolved role
  ceiling (never `CallerAuth::Local`, that is the desktop UI's). The handler does
  the same scope-containment check every node op does (`require_scope` on the
  resolved node/petal scope); **fe-policy is the authoritative Editor+ gate** at
  the DB thread (N-5 — the API layer carries identity, does not decide policy).
- **Stamp position lock (FR-3 / D-A5, T5-LOCK).** A promoted stamp's position
  is path-derived, so a transform write that moves it is refused on every
  transport (REST `PATCH …/transform`, MCP `update_transform`, WS
  `transform_update`, and `move_waypoint_core` — MCP `move_waypoint` takes
  addressable ids, and is refused before its lat/lon/ele property writes too)
  while rotation/scale-only writes pass. Two layers:
  `rest.rs::stamp_translate_denied` is the cheap fail-fast check run BEFORE the
  optimistic broadcast (the persist is fire-and-forget, so a later DB denial
  would arrive after subscribers already saw the move) — it only reads the row
  for ids containing `#inst-`, because `promote_instance_handler` mints exactly
  `<path>#inst-<n>` and `DuplicateNode` strips stamp identity, so plain nodes
  pay no I/O; an unreadable stamp row fails closed. The **authoritative** guard
  is fe-database `update_node_transform_handler` (any `node_kind = stamp` row,
  checked before the op-log commit; fe-database/src/AGENTS.md §lifecycle). REST
  and WS accept `is_addressable_node_id` ids on the transform route so a
  promoted stamp can be rotated/scaled at all (MCP already did). Deferred: API
  rotation/scale on a stamp writes the row's `rotation`/`scale` columns, but the
  renderer reads the `stamp.override.*` properties (`fe-ui/src/actions/asset.rs`
  `STAMP_SCALE_KEY`/`STAMP_ROTATION_KEY`) — mapping API rotation/scale onto the
  overrides is a follow-up, not part of T5-LOCK.
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

## §auth-policy (AP-3.1 / AP-3.4, auth_policy_pattern_20260710)

Every REST/WS/MCP role and scope check evaluates through fe-policy — no
hand-rolled role or scope math in this crate. `auth.rs::api_token_context` is
the ONE `AuthContext` builder for an API token (`ApiToken { subject: sub,
scopes: [scope], role: max_role }`); hexon routes reuse it. The helper
signatures and `StatusCode::FORBIDDEN` errors are unchanged, so callers did not
move: `require_role_level` evaluates `RoleFloorPolicy(required)` (keeps the
"a `None` floor admits everyone" semantics of a typo'd `require_role` string),
`require_scope` evaluates `TokenScopePolicy` against `Scope::new(resource)`,
and `require_role_and_scope` is one `AllOf` of both evaluated through a
`PolicyEngine` (one warn-logged decision per denial). The two WS Editor gates
(`transform_update`, `entity_command`) call `require_role_level(Editor)`.
Policy semantics: `fe-policy/AGENTS.md` §policies.

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
mirroring `§assets`: 400 (bad ULID / bad filter params), 403 (role), 404
(unknown OR out-of-scope petal — §existence-oracle), 502 (query transport
failure). This is stricter than the
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
| GET | `/api/v1/assets/{content_hash}` | raw blob by BLAKE3 hash, `application/octet-stream`, `private, max-age=31536000, immutable`. Viewer+, and the hash must belong to an `asset` row the token reaches — its anchor petal or a referencing node is in scope (`asset_reach` — DEC-C21; a hash is no longer a capability). |
| GET | `/api/v1/assets/by-id/{asset_id}` | resolves `asset` row -> blob, serves with the **real** `content_type`/`name`/`size` from the DB. Granted iff the asset's anchor petal OR ANY node referencing it is in the token's scope (`asset_reach`). |
| GET | `/api/v1/nodes/{node_id}/asset` | resolves `node.asset_id` -> `asset` row -> blob. RBAC scope resolved from the node's parent chain (same `resolve_node_scope` helper as `/nodes/:id/transform`). |

All three require Viewer+ role and that the node's/asset's scope be covered
by the token; not-found and out-of-scope deny identically (404, `not found or
not permitted` — no existence oracle). The id routes require a **valid ULID** path param (reuses
`types::is_valid_ulid` — length + charset check, no separate validator), and
never build a filesystem path from user input: the blob path always comes
from `BlobStore::get_blob_path(hash)`, keyed by the DB's `content_hash`
column, never from the request. Every error path returns a structured JSON
body (`{"ok": false, "error": "..."}`) with a real HTTP status
(400/403/404/500/502/503/501/413) and a `tracing` log line — this deliberately
differs from the rest of `rest.rs`, where most handlers return HTTP 200 with
`{"ok": false, ...}` for historical reasons; asset delivery is byte-stream
territory, so real status codes matter for HTTP caches/CDNs/downloaders.

**`X-Content-Type-Options: nosniff` on every blob response (CDL-SPEC10 A4,
security).** R5 device payloads register an `asset` row with the declared
format descriptor's MIME — any MIME, including browser-sniffable types like
`text/html` or `image/svg+xml`. `get_asset`, `get_node_asset` and
`get_asset_by_id` (via the shared `serve_asset_by_id`/`insert_nosniff`) all
set the header alongside the existing `Content-Disposition: attachment` /
fixed `application/octet-stream` — a browser must not reinterpret the bytes
against a sniffed type (the stored-XSS surface a generic-MIME asset model
opens up). Test: `html_payload_is_served_as_attachment_with_nosniff`
(`tests/device_ingest_test.rs`).

**Asset scope = anchor petal + referencing nodes (DEC-C21; FEAPI-ASSET-OWNER
2026-10-11, `assets::asset_reach`)**: an API upload writes two ownership
columns on its `asset` row — `owner_did` (the token's **owner key**, below) and `petal_id`
(the **anchor petal** the upload was authorized against: the REST path petal
/ MCP `upload_asset`'s `PetalArg`). Legacy and GUI-imported rows leave both
`NONE` (additive schema; no backfill). Two nodes (even in different petals)
may still share one `asset_id`. `asset_reach` classifies the asset(s) for a
token, fail-closed on any lookup failure:

1. **Anchor first.** If any requested row the token may own-read has an
   anchor petal the token covers → `Covered`, with no reference scan — the
   uploader can read its upload before it is placed and after it is placed
   elsewhere. A row is own-readable when its `owner_did` is absent (legacy /
   peer-replicated rows), equals `claims.sub` (a **node-level** upload: every
   token of the node shares it), or equals the token's `owner_key` (A5).
2. **Reference scan** (legacy rows, assets shared into other petals): EVERY
   referencing node's petal AND every referencing `device_payload` row's
   petal (capped: 10 000 rows per scan / 256 distinct petals, fail-closed
   beyond) — any in scope → `Covered`; all outside → `Foreign`. The payload
   leg (R5 sweep 2026-10-25) exists because a device-payload asset is never
   placeable: without it, a dashboard token of another client could list a
   payload (Viewer+) yet never fetch its blobs. Test:
   `another_clients_viewer_reads_a_device_payloads_blobs_through_its_petal`.
3. **Unplaced**: an owned row whose anchor is not covered, or that belongs
   to another client → `Foreign`; a legacy row (no owner/petal) →
   `Unreferenced` (unchanged behaviour).

Reads (by-hash, by-id) require `Covered`; MCP `place_asset` refuses `Foreign`
(so a token can neither launder another tenant's placed asset into its own
petal nor claim another tenant's still-unplaced upload) with the same text as
an unknown id. **`owner_did` never widens reach:** the tenant boundary is
the token scope, hence the anchor petal; the owner clause only *narrows*
unplaced reach inside it. Consequences, all deliberate: a legacy
never-placed asset is not readable until placed; a deny-by-default miss (row
cap) is a false 404, never a leak; the `asset` table is not in the `/query`
read whitelist (§query-guard). Indexes: `idx_asset_owner` (quota sums),
`idx_asset_content_hash` (by-hash reads, GC).

**Owner key (TOKEN-CLIENT-CLAIM, 2026-10-25, `upload::owner_key`).** Every
token this node accepts is minted by the node keypair, so `sub` is the node
DID for every caller. A token minted through the production door
(`DbCommand::MintApiToken`) also carries a `client` claim — an ed25519
`did:key`, requested or freshly generated (fe-identity `src/AGENTS.md`
§api-token "client claim"). `owner_key(claims) = client.unwrap_or(sub)` is
the ONE place fe-api reads `claims.client`, and `UploadOwner::new` is its one
caller on the write side (and `UploadOwner`'s only constructor — its fields
are private, refine R2), so every upload door inherits it (REST multipart,
MCP `upload_asset`, REST/MCP device ingest). It keys three things only:
`asset.owner_did`, the per-owner quota (§asset-ingest) and the unplaced-reach
own-read clause above. It is **never an authorization subject**:
`endpoint::caller_auth` and `auth::api_token_context` keep `claims.sub`, the
role ceiling stays `max_role`, W5's AC-46 cap resolves the node — so a
`client` naming a Manager's DID gains nothing (threat TC-1,
`a_client_claim_naming_a_manager_did_gains_no_role`). Check order on an
upload: JWT verify (incl. the `client` grammar) → `jti` revocation → role +
anchor-petal scope → AC-46 cap where replicated → reach/quota by owner key.
The upload doors' AC-46 cap runs on the DB thread at `CreateAsset` (TC-7,
refine R2): a stale token's bytes may already be in the blob store when the
registration is refused — an unregistered blob that no asset row reaches.
`owner_did` stays local: the device-payload emission and the scene publisher
strip it from outbound asset rows and peers strip it inbound (fe-database
`handlers/device_payload.rs` `outbound_asset_row_bytes`, `replicated_row.rs`).
An anchored GLB/glTF upload is published at registration, before any placement
(QA-0158, fe-database `handlers/scene_publication/AGENTS.md` §uploaded-assets);
peers read it as an ownerless row, so their tokens reach it through its anchor. Rotation caveat: a re-minted
client cannot read its predecessor's *unplaced* uploads (placed ones keep
reference reach) — re-mint with the same client DID (GUI "Client DID" field,
CLI `--client`) for continuity. Tests (`tests/asset_ownership_test.rs`):
`two_clients_of_one_node_have_separate_quota_buckets`,
`a_legacy_token_uploads_into_the_node_level_bucket`,
`unplaced_upload_reach_follows_the_owner_key`,
`a_client_claim_naming_a_manager_did_gains_no_role`.

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

## §db-reader (DB-READER-RO, CDL-SPEC10 A3b — closes ledger precondition before A4/A5)

`ApiState.db_reader` is `Option<Arc<db_reader::ReadDb>>`, not the raw
`Surreal<engine::local::Db>` handle. `ReadDb` (`db_reader.rs`) is a guarded
facade: it wraps the connection and exposes exactly one method,
`ReadDb::query`, mirroring `Db::query`'s own signature. It never implements
`Deref`/`DerefMut`, so none of SurrealDB's typed CRUD builders
(`create`/`update`/`delete`/`insert`/`upsert`) are reachable through it.

**Why not the usual "`Deref`, no `DerefMut`" guard (correction to the
ledger's literal sketch).** That idiom blocks mutation only when the
mutating methods require `&mut self`. SurrealDB's `Surreal<C>` is an async
client handle over a channel (`inner: Arc<Inner>`), so `query`/`create`/
`update`/`delete`/`insert`/`upsert` all take `&self` (verified against the
vendored `surrealdb-3.0.5` source, `src/method/mod.rs`). A `Deref<Target =
Db>` would make `.delete()` resolve through ordinary autoderef regardless of
whether `DerefMut` exists — so this type guards by omitting `Deref`
entirely and re-exposing only `query` as an inherent method. The
`compile_fail` doctest on `ReadDb` proves this (E0599, no method and no
`Deref` chain to one), which is the literal acceptance the ledger asked for.

**What the guard does and does not cover.** `ReadDb::query` still executes
whatever SurrealQL string it is given, including a hand-written `CREATE`/
`DELETE` statement — the existing SELECT-only validator in `query_guard.rs`
is the (unchanged) layer responsible for that, and a few in-crate tests
seed fixtures with a raw `db.query("CREATE ...")` exactly as before. What
`ReadDb` removes is the *typed* CRUD surface: grep confirms no `db_reader`
call site anywhere (src or tests) ever called `.create()`/`.update()`/
`.delete()`/`.insert()`/`.upsert()` — only `.query()` — so this costs zero
behaviour and closes the surface a future handler could otherwise reach by
habit (the typed API is the one most code in this crate uses against the
DB-thread's own handle elsewhere).

**The one write exception.** `iot.rs`'s `ingest_readings` writes directly
through `db_reader` (§iot-ingest's append-only fast path, bypassing the DB
thread). `ReadDb::writable(&self) -> &Db` is the single named escape hatch
for that path — `insert_readings_with_replication` still takes the raw
`&Db` the fe-database handler expects. It is `pub(crate)`: code outside
fe-api (the harness, binaries, integration tests) can hold a `ReadDb` but can
never turn it back into a writer. Grep for `.writable()` before adding
a second caller; a new write surface on `db_reader` should be a deliberate,
reviewed decision, not an accidental one.

**Migration (zero behaviour change).** All ~59 existing read call sites
compile unchanged against `ReadDb` because every one of them already only
called `.query(...)`. A handful of in-crate helper functions that took
`db: &Surreal<engine::local::Db>` (the `direct_resolve_*_scope` family in
`rest.rs`, `serve_asset_by_id`/`direct_load_asset_meta` in `assets.rs`,
`load_export_nodes`/`load_field_defs` in `format.rs`,
`load_gpx_export_nodes` in `gpx.rs`, `load_petal_nodes_direct` in `ws.rs`,
`run_guarded_query` in `query_guard.rs`) now take `&ReadDb` — a few even
via a local `type Db = crate::db_reader::ReadDb;` alias so the diff is a
one-line retarget. Construction sites (`ApiState`/`ApiConfig`, the two
binaries' `api_db_reader` hydration block, `fe-test-harness::ApiHarness`,
and the in-crate/integration tests that build an `ApiState` directly) wrap
the owned `Db` with `ReadDb::new(db)` before the `Arc`.

## §query-guard + §limits

## §analytics-query

`POST /api/v1/analytics/query` is a deliberately narrower DataFusion surface
than `/api/v1/query`: its body requires both `sql` and a concrete `petal_id`.
The handler requires Viewer+, validates and directly resolves that petal's
hierarchical scope, then requires token containment *before* registering the
`EntityStore` table. The table is always registered with the authorized petal
filter; caller SQL can never select an all-petals table. Analytics has no
crossbeam scope-resolution fallback: without a direct DB reader it returns an
explicit unavailable error; an unresolvable or out-of-scope petal is the one
`NOT_FOUND_OR_DENIED` denial (§existence-oracle).

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
closed (`entity_store: None`) rather than serving a partial nodes table.
The GUI (`fractalengine/src/main.rs`) calls these shared fns directly — its
pre-extraction local copies and their duplicate tests were deleted
(FEAPI-GUI-HYDRATE, 2026-10-11), so the module's own tests are the only copy.

**Honest-unavailable posture on per-handle-lock platforms (Windows).** (Superseded by OF-2, 2026-10-13: both binaries now pass a clone of the DB thread's own handle as `db_reader`, so the reader is present on every platform — fe-database `src/AGENTS.md` §shared-handle. The fallbacks below remain for a `None` reader, e.g. a failed GUI hydration.) The
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
read whitelist (RBAC data is not BI egress; since 2026-10-10 the elevated
endpoint excludes them too — §elevated-query). The elevated endpoint's DDL
screen moved from bypassable multi-word substring matching ("DEFINE TABLE"
vs "DEFINE  TABLE") to whole-word keyword bans + all-occurrence target
checks in `validate_elevated_sql`. Fail-closed tradeoff accepted: keywords inside
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
   constructors (`type::thing|record|table|field(s)`, `record::`,
   `<record>`, and the `.to_record()` method alias), `r"…"` record
   strings — every way to name a record without its table name;
   **Record-id literals (2026-10-10):** `reject_foreign_record_literals`
   then refuses any `table:key` literal whose table is not in
   `ALLOWED_TABLES`. Before this, a Viewer of verse A could read
   `SELECT role:victim.* AS l FROM verse` (role rows by id) and
   `SELECT $v.to_record().* … FROM verse` with `vars: {v: "<node rid of
   verse B>"}` (any verse's node — `.to_record()` slipped past the
   constructor needles, and a bound var never shows the word `node` to
   step 5). Both proven against SurrealDB 3.0.5 and pinned by
   `tests/record_id_read_test.rs`. The scan is lexer-free (strings count)
   but sound and narrow because SurrealQL glues `:` to both sides of a
   record id (`parse_record_id_key` bails on whitespace): spaced object
   keys `{k: v}`, `::` function paths and digit-led runs (`…T00:00:00Z`
   datetimes) pass; `{k:v}`, `'did:key:…'` and other `word:word` literals
   are refused — bind them through `vars`. String-aware skipping was
   rejected on purpose: SurrealQL regex literals (`/'/`) would desync a
   quote lexer;
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

*Row-scoped tables (corrected 2026-10-09, DEC-C19 N1; `device_payload` added
CDL-SPEC10 A5):* `node`, `iot_reading`, `petal`, `room`, `model`,
`crate_registry`, `device_payload` are all in `PETAL_SCOPED_TABLES` and get
the identical FROM-substitution row filter — each has its own `petal_id`
column (fe-database/src/schema.rs). `device_format` is NOT in
`ALLOWED_TABLES` at all: it is verse-scoped (not petal-scoped), and already
has its own dedicated read route (`list_device_formats`, REST + MCP) rather
than an ad-hoc BI query. The claim previously here —
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
`iot_reading`) if a real consumer appears. `ASSET` was removed the same way
(DEC-C21): its rows carry no owner/scope column, so allow-listing it exposed
every uploaded `asset_id`/`content_hash` to any token; no `/query`/export
consumer existed (fe-ui reads assets via the DB thread; the rest.rs whitelist
test now pins the rejection). `crate_entry` carries no
`petal_id` column and stays deliberately unscoped. Function namespaces
outside the banned constructors (e.g. a future `fn::`/`api::`) are not
enumerated.

*`verse.namespace_id` is a capability, not a column (SYNC-NSID-CAP,
2026-10-11).* It is the iroh-docs `NamespaceId`, which IS
`Capability::Read` — any peer holding it plus a serving node's address can
sync the whole verse replica (fe-sync/src/AGENTS.md §namespace-id-capability).
`VERSE` is allow-listed but not petal-scoped, so before this any token could
`SELECT namespace_id FROM verse` across every verse on the node. Now, in every
`GuardMode` (Query, Egress share, Export — all reach VERSE only through
`prepare_scoped_sql`): (1) any SQL whose uppercased canonical text contains
the word `NAMESPACE_ID` (projection, `WHERE` probe, even a string literal) is
refused with `NAMESPACE_ID_REFUSAL` ("namespace_id is not queryable (replica
read capability)"); (2) every `verse` FROM target is rewritten to
`(SELECT * OMIT id, namespace_id FROM verse)` — `id` is omitted too because
`SELECT id.* FROM verse` would re-fetch the whole record, capability
included (`verse_id` still identifies the row); (3) `verse`, like the
petal-scoped tables, may only appear as a FROM target, which kills the
`verse:<key>.*` record-literal dereference. OMIT parses on the embedded
SurrealDB 3.0 (proven by `tests/namespace_capability_test.rs`). Unit tests:
`verse_targets_never_expose_the_replica_capability`,
`namespace_id_and_verse_record_literals_are_refused_in_every_mode`.
Behaviour change for clients: `/query` verse rows no longer carry `id` or
`namespace_id`, and a query mentioning `verse` outside FROM is refused.

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

## §elevated-query (`rest.rs::execute_elevated_query`, 2026-10-10; DB-thread routing 2026-10-11)

`POST /api/v1/query/elevated` runs caller-supplied multi-statement SurrealQL
— including writes — on the DB thread (`DbCommand::ElevatedQuery`, ratified
2026-10-10: route, do not remove). It **cannot be scope-confined**:
raw SQL is not honestly filterable by string (the M4 review broke the old
`/query` scope filter four ways — §query-guard), so there is no token-scope
check at all. Until 2026-10-10 the only gate was Manager+, which let a
Manager token for verse A rewrite rows of any verse on the node, including
`role`/`verse_member` (privilege escalation). The posture is now
deny-by-default (fe-policy's ratified stance), mirroring `FE_SIM_ALLOW`
(DEC-C13):

- **Operator opt-in.** `ApiState.elevated_query_allowed` is `false` unless the
  binary read `FE_API_ELEVATED_ALLOW=1` at startup
  (`fe_api::elevated_query_allowed_from_env`, called once by both the relay
  and the GUI; loud WARN when enabled, quiet when unset, a warning when set
  to anything but `1`). Off → every call gets `elevated query endpoint is
  disabled on this node` before role, rate limit, parsing or DB. A field,
  not a per-request env read, so tests set it
  (`ApiHarness::spawn_with_elevated_query_allowed`).
- **Guard order: opt-in → Owner (typed `RoleLevel` — a role-string typo
  would parse to `RoleLevel::None` = admit-all) → 5/s per DID →
  `validate_elevated_sql` → DB thread.** The first three steps are the shared
  `ELEVATED_QUERY_GATE` (§operator-gate). Owner of ANY scope passes, which is
  exactly why the endpoint is only for single-tenant nodes.
- **Table whitelist `query_guard::ELEVATED_TABLES`** = scene content only:
  `NODE`, `FRACTAL`, `PETAL`, `FIELD_DEF`, `MODEL`, `ROOM`. Removed:
  `ROLE`, `VERSE_MEMBER` (privilege data changes only through the RBAC
  handlers); `VERSE` (no secret on the row — the namespace secret lives in
  the keystore — but `created_by` and `default_access` are
  `role_manager::resolve_role` inputs, so `UPDATE verse SET created_by = …`
  is an Owner takeover and bypasses `set_default_access`'s viewer/none pin);
  `NODE_LOG` (append-only history); `ASSET`, `CRATE_REGISTRY`,
  `CRATE_ENTRY` (content-addressed hashes / signature-checked install path —
  a raw rewrite swaps blobs or forges `signature_valid`).
- **Target parsing fails closed.** The old screen inspected only
  `FROM`/`INTO`/`UPDATE` and only the first identifier, so `DELETE role`,
  `CREATE role …`, `UPSERT role …` and `UPDATE node, role …` all passed the
  whitelist. Now every `FROM`/`UPDATE`/`UPSERT`/`CREATE`/`DELETE`/`INTO`
  target list is checked element by element; write targets must be plain
  table names (record ids, `$params`, escaped identifiers, subqueries,
  `->` graph paths fail — filter with `WHERE`); `INSERT` must use `INTO`;
  `USE` (would re-point the DB thread's session), `ALTER`,
  `REBUILD`, `ACCESS`, `SHOW`, `OPTION`, `RELATE`, `LIVE` join the
  DDL/`INFO`/`SLEEP`/`KILL` bans. Keywords inside string literals reject the
  query — bind literals through `vars`.
- **DB-side `TIMEOUT 5s` on every DML statement (FEAPI-ELEV-TIMEOUT).**
  `validate_elevated_sql` returns an `ElevatedPlan`: the script split into
  top-level statements by `split_elevated_statements` (`;` splits only
  outside strings, escaped identifiers and `()`/`[]`/`{}` nesting), each
  statement starting with `SELECT`/`CREATE`/`UPDATE`/`UPSERT`/`DELETE`/`INSERT`
  suffixed ` TIMEOUT 5s`, plus the lowercase write tables. `TIMEOUT`,
  `PARALLEL`, `EXPLAIN` (a caller budget could raise ours; the other two
  must follow it) and `BEGIN`/`COMMIT`/`CANCEL` (would break the DB thread's
  transaction wrapper) are banned words. The API awaits
  `ELEVATED_REPLY_TIMEOUT_SECS` (7 s) for the reply; `query timed out (5s)`
  means only that backstop fired — the DB thread still finishes the script
  and its late reply is dropped by the entry's pop-and-drop (fe-runtime
  §api-reply-correlation), so a timed-out write MAY have landed.
- **32-statement cap (FEAPI-ELEV).** `MAX_ELEVATED_STATEMENTS` (fe-database
  `handlers/elevated_query.rs`) is the single definition, enforced twice:
  `validate_elevated_sql` fast-fails on the same count right after
  `split_elevated_statements` — before the `TIMEOUT` suffix map, so a
  trailing run of empty statements never counts — and `elevated_query_reply`
  repeats the check as the authority, before any snapshot, query or op-log
  work (a caller that reached the DB thread without going through the API
  gets the same refusal). The cap counts top-level statements only: a
  `FOR`/`IF` block is one statement however many inner statements it holds,
  and a `;` inside a string, an escaped identifier or `()`/`[]`/`{}` nesting
  is not a split point. Over the cap: `elevated script has {n} statements;
  the limit is 32`, the same `ok:false` envelope as every other refusal.
  The splitter does not model SurrealQL regex literals (`/…/`), so a `/'/`
  could make it read a "string" across real `;` boundaries (EL-1). The DB
  side therefore re-counts the joined script with SurrealQL's own parser and
  refuses any mismatch (`statement boundaries are ambiguous`) — refine R2,
  fe-database `src/AGENTS.md` §handlers "elevated_query". EL-2 verdict (kept):
  `FOR` stays allowed — a keyword ban is lexer-free here and would refuse any
  string containing the word "for"; the endpoint is Owner-only and opt-in.
- **Single writer, exactly like the typed handlers** (fe-database
  `handlers/elevated_query.rs`, `src/AGENTS.md` §handlers). Every script —
  reads too — runs on the DB thread; `db_reader` is never used, so the
  endpoint works on a channel-only (Windows) node. A script with no write
  target runs as-is and is not op-logged. A write script:
  1. refuses unless every write table is in the DB-side mirror
     `ELEVATED_WRITABLE_TABLES` (the API parse is authoritative; this is a
     second check) and the write tables hold ≤ `ELEVATED_DIFF_MAX_ROWS`
     (20 000) rows in total — larger is refused before anything runs;
  2. snapshots those tables (blake3 per row; nodes keep their
     id/petal/tombstone state);
  3. commits through `op_log::commit_operation_atomic`: ONE
     `OpType::ElevatedQuery` entry (`node_id` = token `sub`, payload
     `{sql, vars, actor_did, write_tables}`) in the SAME transaction as the
     caller's statements — a failing or timed-out statement leaves neither
     the log row nor any write;
  4. diffs a post-snapshot (exact: the serial DB thread is the only writer
     between the two), bumps `edit_seq` on updated nodes, and appends one
     `node_log` fact (op `elevated_write`, `source_did` = token `sub`,
     payload `{change: created|updated|deleted, row}`) per changed node —
     the typed node handlers' best-effort history;
  5. then emits: `SceneChange::NodeAdded` for a created/updated live node,
     `NodeRemoved` for a newly tombstoned or hard-deleted live node, and one
     `ReplicationEvent` per created/updated `node`/`fractal`/`petal` row (the
     tables the inbound applier handles; `id` stripped, verse resolved
     through the hierarchy). The reply carries `changed_rows`; the desktop
     reloads its tree when it is > 0.
- **Residuals (documented, not bugs):** `field_def`/`model`/`room` writes are
  op-logged and local only — no inbound applier, so not replicated. Hard
  deletes are not replicated (same as typed `DeleteNode`) — tombstone for a
  sync-safe delete. Statements that do not start with a DML verb (`LET`,
  `RETURN`, `IF`, `FOR`, a bare `(SELECT …)`) carry no DB-side `TIMEOUT`;
  they are bounded only by the API's 7 s backstop, and the DB thread stays
  busy until they finish — a dropped transaction future does NOT cancel the
  embedded engine's spawned query task (verified 2026-10-11: the write
  committed after the future was dropped), so no DB-thread wall-clock
  budget can be enforced safely. A top-level `RETURN` inside a write
  script ends the transaction's statement list early (SurrealDB
  semantics): later statements do not run.
- **Reads that name no FROM target are closed too (2026-10-10 adversarial
  review).** `RETURN role:x.*`, `SELECT *, role:x.* AS l FROM node`,
  `$v.to_record().*` (with `vars: {v: "role:x"}`), `type::record(…)`,
  `r'role:x'` and `⟨role⟩:x` all dereferenced `role` rows (proven against
  SurrealDB 3.0.5). `validate_elevated_sql` now also runs the shared
  `reject_comments` + `reject_record_constructors` (needles incl.
  `TO_RECORD`, `TYPE::FIELD`) + `reject_foreign_record_literals(…,
  ELEVATED_TABLES)` — see §query-guard "Record-id literals". Cost: object
  keys must be spaced (`{key: value}`), comments are refused, and string
  literals holding `word:word` (DIDs, `a:b`) must be bound through `vars`.
  The `ON DUPLICATE KEY UPDATE` target-check exemption now requires both
  `DUPLICATE KEY`, not just a preceding `KEY` (`$key UPDATE role` no longer
  skips the check).
- **Writes cannot be redirected through `id`:** `CREATE/UPSERT node CONTENT
  {id: role:x}` and `INSERT INTO node {id: …}` re-table the key onto `node`
  (surrealdb-core `val/value/generate.rs`, `insert.rs`), and `SET a.b = …`
  through a record link overwrites the link instead of writing the linked
  row (`val/value/set.rs`) — so the write-target check is the whole write
  boundary.
- **Practically unreachable today:** `handlers::api_token` refuses to mint
  `max_role: owner`, so no issued API token passes the Owner gate even on an
  opted-in node. Keep it that way unless the endpoint gains a real scope
  model.
- Tests: `tests/elevated_query_test.rs` (router + real reply router + the
  production `elevated_query_reply` via `serve_db_thread`: off → refused;
  Manager → refused; Owner write → one op-log row, one `node_log` fact, one replication event
  carrying the bumped `edit_seq`, one `NodeAdded`; a failing second
  statement rolls back the first and the log; a long scan hits the DB-side
  TIMEOUT in < 7 s and the next script still runs; works with
  `db_reader: None`; privilege rewrites refused and rows unchanged; a script
  over the 32-statement cap is refused with nothing op-logged —
  `an_elevated_script_over_the_statement_cap_is_refused_and_writes_nothing`),
  `tests/record_id_read_test.rs` (Owner cannot read a seeded `role:victim`
  by id; the same vectors on `/query`) + `query_guard` units
  `elevated_write_targets_must_be_plain_whitelisted_tables`,
  `elevated_plan_bounds_every_dml_and_names_its_write_tables` and
  `elevated_statement_cap_counts_top_level_statements` + fe-database
  `handlers::elevated_query::tests::the_db_thread_refuses_an_elevated_request_over_the_statement_cap`.

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
  non-nullable) rather than fabricating `[0,0,0]`. `value` and the anchor
  position are both `Float64` end to end (EGR-F32) — rows, the Arrow column,
  and CSV — so BI consumers get both back bit-exact.
- Parquet responses ship `Content-Type: application/vnd.apache.parquet`,
  `Content-Length` (axum), and `Accept-Ranges: bytes` so DuckDB httpfs can
  `read_parquet('<url>')` (plan D1).
- **Windows `db_reader` fallback (F10, 2026-10-09).** (Superseded by OF-2, 2026-10-13: both binaries now pass a clone of the DB thread's own handle as `db_reader`, so the reader is present on every platform — fe-database `src/AGENTS.md` §shared-handle. The fallbacks below remain for a `None` reader, e.g. a failed GUI hydration.) `prepare_export` and
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
  depth. **CLOSED 2026-10-11 (EGR-DBREADER):** `/api/v1/query`
  (`rest.rs::run_local_query`, shared by MCP `query`) and the `fmt=json`
  branch of share redemption now use the same
  `run_guarded_query_via_state` (correlated, `TIMEOUT 5s`, the
  `EGRESS_FALLBACK` semaphore → "egress busy" 503). A gone DB bridge is an
  honest error: `/query` answers an error envelope, share 503
  (`query_guard::is_db_bridge_error`). Note the DB thread's `RawQuery`
  keyword blocklist (`fe-database/src/handlers/raw_query.rs`: `IF`, `FOR`,
  `RETURN`, …) is stricter than the API guard and can reject SQL the
  direct `db_reader` path would run — that error is honest, not a leak.
  Tests: `tests/export_share_test.rs` `*_channel_fallback_*` (real router +
  production `raw_query_reply` over Mem SurrealDB, with GUI and foreign
  correlated rows raced in).
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
  the label in both. **Precision (EGR-F32, closed):** positions — node and
  reading-anchor, parquet and CSV, local and latlon — are `f64` end to end
  through `EntitySnapshot`/`ReadingSnapshot` (`fe-entity-store/src/AGENTS.md`
  §egress-precision); the old `f32` narrowing (≈1 m at mid-latitudes) is
  gone. Rotation/scale stay `f32` (D-54) — no BI consumer computes distance
  over them.
- **EGR-WKB: the parquet WKB geometry column is east-north-up for
  petal-local output.** `parquet_response` passes the already-existing
  `out.coords == Coords::LatLon` flag into fe-query (`fe-query/src/AGENTS.md`
  §geoparquet), which reorders `[x, y, z]` (engine, y up) to
  `[east, north, up] = [x, -z, y]` in the WKB bytes only; lon/lat output is
  identity, because it is already `[lon, lat, ele]` here in `row_to_snapshot`.
  **CSV and the analytics `pos_*` columns are unaffected (A7)** — they stay
  engine-frame, `x_m,y_m,z_m` / `lon,lat,ele_m` headers unchanged. A real GIS
  reader (DuckDB's `spatial` extension, `ST_X/ST_Y/ST_Z`) sees ENU directly;
  `scripts/bi-egress-verify.ps1`'s "ENU axis order" section proves it live.
- Status mapping: 400 bad query/coords/table, 403 role, 404 unknown or
  out-of-scope petal (§existence-oracle), 413 row-cap/byte-ceiling (enforced on the readings path too — the
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
- **Windows (no `db_reader`):** every format redeems through the correlated
  channel fallback (`fmt=json` since 2026-10-11, EGR-DBREADER — §export);
  a saturated fallback or a gone DB bridge answers 503.

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
scope and permits only IDs bound in that petal's terrain configuration. An
unknown or foreign `petal_id` is 404 (§existence-oracle; only a role failure
is 403), and unbound or unknown tileset IDs on an authorized petal are 404 too, and byte responses are `private, no-store` so a
shared cache cannot replay a scoped tile to another caller.

## §terrain-mutation (SetPetalTerrain correlation, 2026-10-10)

`terrain.rs::set_petal_terrain_core` backs REST `PUT|DELETE
/api/v1/petals/{p}/terrain` and MCP `set_petal_terrain` (which replaced the
former 503 `TERRAIN_MUTATION_UNAVAILABLE` refusal). Order: RBAC Editor+ →
ULID check → petal scope resolution + `require_scope` (MCP: the dispatcher's
`PetalArg` rule) → `TerrainConfig` validation → correlated write.

- **Validation (DEC-C22).** A non-null value must deserialize as
  `fe_terrain::config::TerrainConfig` — the strictest reader of the column
  (`terrain_config_from_petal_json`) — else 400 / MCP `invalid terrain config`.
  The RAW object is stored, not a re-serialization: additive keys fe-terrain
  reads raw (`view_mode`) and GUI-owned keys survive a GET→edit→PUT round
  trip. PUT requires an object; `null` (MCP) / DELETE clears. Because the body
  is taken as `Json<Value>`, auth runs before config validation.
- **Write path.** Always `api_cmd_tx` (`DbRequest`), never `db_reader` — the
  DB thread is the single writer. A fresh ULID rides `correlation_id`; the
  reply is accepted only if the echoed id AND `petal_id` match (502 on a
  mismatch, never another caller's terrain). Success returns the terrain the
  DB thread READ BACK after the write (GET's `{petal_id, terrain}` shape).
- **Failures.** Correlated `PetalTerrainSetFailed` / closed channel → 502
  `terrain write failed` (detail logged, not returned); no reply in
  `TERRAIN_WRITE_TIMEOUT_SECS` (10, the IoT write precedent) → 504. Routing:
  fe-runtime `src/AGENTS.md` §api-reply-correlation (family `PetalTerrainSet`).
- **Validation also requires a JSON object**: serde would accept a
  positional array as a `TerrainConfig`, but the column is `option<object>`
  and lenient readers call `terrain.get(..)`.
- **Host fan-out.** The write's echo also drives the desktop Bevy consumers
  (map picker, `terrain_bridge`) — but ONLY when it is the desktop's active
  petal: both drop other petals' echoes, so an API write to petal B never
  replaces the terrain of the petal the user is viewing (fe-runtime
  §api-reply-correlation). The tileset-list GET fallback
  (`assigned_tilesets`, no `db_reader`) sends a CORRELATED `GetPetalTerrain`
  (2026-10-11) and accepts only a `PetalTerrainFetched` echoing its id AND
  petal; `RequestFailed` or anything else is refused. That reply is private
  to the API caller — it no longer re-applies the read to the desktop's
  terrain runtime, and a GUI load for the same petal cannot satisfy it. Tests: `tests/terrain_mutation_test.rs` (full router,
  real reply router + production DB reply builder against Mem SurrealDB).

## §render-distance (SetPetalRenderDistance, p2p_asset_streaming_20260718 S6)

`render_distance.rs::set_render_distance_core` backs REST `PUT|DELETE
/api/v1/petals/{p}/render-distance` and MCP `set_petal_render_distance` —
same shape as `set_petal_terrain_core` (§terrain-mutation), minus the GUI
fan-out (render distance has no desktop live-apply consumer the way terrain
does; the fe-ui side picks it up via `ReplicatedRowApplied`/`HierarchyLoaded`,
not an API echo). Order: RBAC Editor+ (PUT/DELETE) or Viewer+ (GET) → ULID
check → `authorized_petal_scope` (404 unknown-or-foreign, §existence-oracle)
→ range validation → correlated write.

- **Validation.** `render_distance` must be a JSON number or `null`
  (`null`/absent clears — inherit the global setting); a non-finite or
  negative number is 400. The DB thread's own `ASSERT` is the authoritative
  ceiling (`[0, 50000]`) — the API only screens for the cheap, obviously-wrong
  cases before the round trip; the DB thread's refusal still surfaces as 502.
- **Write path.** Always `api_cmd_tx`, never `db_reader`. `CallerAuth::Identified
  { did: "api", role: "editor" }` carries the role this route ALREADY
  validated (`require_role(&claims, "editor")`) — same trust boundary as
  `terrain.rs`/`visibility.rs`'s REST routes, never a role the DB thread
  resolves itself for an API caller. A fresh ULID rides `correlation_id`; the
  reply is accepted only if the echoed id AND `petal_id` match.
  `PetalRenderDistanceLoaded` is the one reply shape for both set and get.
- **Failures.** `RequestFailed` / closed channel → 502; no reply in 10 s → 504
  (`RENDER_DISTANCE_WRITE_TIMEOUT_SECS`, mirrors the IoT/terrain precedent).

## §visibility-groups (VIS-PH6, hierarchy_visibility_groups Phase 6)

`visibility.rs` serves a petal's visibility-Group registry
(`[{id, name, visible, order}]`, fe-database §lifecycle "Visibility groups"):
`GET` (Viewer+) and `PUT` (Editor+, whole-registry replace, body
`{"groups": [...]}`) on `/api/v1/petals/{p}/visibility-groups`, and `PATCH`
(upsert one Group; a NEW id needs `name`, `visible` defaults true, `order`
to last+1) / `DELETE` (unknown id → 404) on `…/{group_id}`. Gate order
mirrors `terrain.rs`: `require_role_level` (fe-policy `RoleFloorPolicy` over
`api_token_context`) → ULID → `rest::authorized_petal_scope` (missing and
foreign petals are the SAME `NOT_FOUND_OR_DENIED` body,
`tests/visibility_groups_api_test.rs`) → the DB thread, which re-checks
Editor+ at the petal scope from `endpoint::caller_auth(&claims)`. Every call
is a correlated `ApiCommand::DbRequest` (fresh ULID; reply must echo the id
AND petal) with a 10 s timeout; a malformed registry is a 400 BEFORE the
round trip (`fe_database::handlers::visibility_groups::validate_groups`, the
same validator the DB thread runs); a DB refusal/failure is 502 "failed or
not permitted" (detail logged). **PATCH/DELETE use read-modify-CAS** under a correlated Get/Compare pair
(`modify_group_core`): a registry write between capture and use refuses instead of losing that edit.
PUT remains an explicit whole-registry replacement. All registry introductions reject IDs already
referenced by stored live or tombstoned memberships, preventing legacy deleted-ID resurrection.
New native Groups use ULIDs while existing API IDs remain stable. MCP `list_visibility_groups` (Viewer) and
`set_visibility_group` (Editor; `group_id` + `name`/`visible`/`order`, or
`delete: true`) share these cores.

## §iot-ingest (iot_spatial_reporting_20260714)

`iot.rs` — `POST /api/v1/petals/:petal_id/iot/readings`, the minimal-v1
external ingestion hook (batch JSON; webhook-able; a poll connector catalog is
future work). Design notes:

- **Guard order mirrors export.rs**: `require_role("editor")` (writes need
  Editor+, unlike the Viewer+ read endpoints) → ULID check →
  `authorized_petal_scope` (404 unknown-or-foreign, §existence-oracle) → per-DID rate limit
  (`limits::IOT_INGEST_RATE_PER_SEC`) → batch cap
  (`limits::IOT_INGEST_MAX_READINGS`, 413 past it).
- **The write goes straight to `db_reader`** via
  `fe_database::handlers::iot_reading::insert_readings_with_replication`, not
  through a `DbCommand` round-trip: readings are append-only facts with no
  derived counters, so the DB-thread single-writer invariant doesn't apply
  (rationale in fe-database `src/AGENTS.md` §iot-readings), and IoT-frequency
  batches must not queue behind the render loop's channel.
- **DB-thread fallback when `db_reader` is absent (F24)**: (Superseded by OF-2, 2026-10-13: both binaries now pass a clone of the DB thread's own handle as `db_reader`, so the reader is present on every platform — fe-database `src/AGENTS.md` §shared-handle. The fallbacks below remain for a `None` reader, e.g. a failed GUI hydration.) the SurrealKV
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

## §device-ingest (`device.rs`, CDL-SPEC10 A4 — security review-and-fix)

R5's generic-asset + device-payload REST surface, built on A3's DB layer
(`fe-database::handlers::device_payload`) and A2's `fe-sdk::source_class` /
`fe-sdk::device_format` shapes. No second validation path: every guard below
front-loads a cheap REST-layer check purely to fail before a byte moves or
to pick the right `content_type`; the DB-thread handler is still the actual
authority and re-validates everything (A5's MCP tools reuse these same core
functions for the same reason).

- **`POST /api/v1/verses/{v}/device-formats`** — Manager+ at verse scope (R5
  assumption 2: schema-defining, like branch controls — the same floor the
  A3 inbound-replication gate uses for `device_format`, so the local and P2P
  gates agree). Body deserializes straight into `fe_sdk::device_format::
  DeviceFormatDescriptor` (no separate request DTO — one shape, one grammar
  check). `DbCommand::RegisterDeviceFormat` → `DeviceFormatRegistered` (201)
  or the typed `DeviceFormatRejected` (422: bad `format_id` grammar, blank
  `mime`, or a conflicting `mime` on an existing `(verse_id, format_id)` —
  re-declaring the SAME mime is an idempotent 201, per the DB handler).
- **`GET /api/v1/verses/{v}/device-formats`** — Viewer+, same verse-scope
  resolution (`rest::resolve_verse_scope` + `write_target_authorized` with
  no ancestry — the `create_fractal`/`create_verse` idiom, reused for a read).
- **`POST /api/v1/petals/{p}/iot/payloads`** — Editor+, petal scope
  (`authorized_petal_scope`, §existence-oracle). Multipart fields:
  `format_id`, `node_id`, optional `recorded_at` / `payload_id`, and 1..=
  `limits::DEVICE_PAYLOAD_MAX_ASSETS` (32) ordered `file` parts. Route body
  limit: `upload::DEVICE_PAYLOAD_ROUTE_BODY_LIMIT` (= `ASSET_ROUTE_BODY_LIMIT`,
  256 MiB + 1 MiB — one 256 MiB-capped blob's worth of bytes per payload is
  already generous; `DEVICE_PAYLOAD_MAX_ASSETS × MAX_ASSET_BYTES` would be an
  absurd per-route ceiling). Order:
  1. Resolve the declared format's MIME (`device.rs::declared_format_mime`,
     a `gis::run_select` read, `pub(crate)` so MCP's `ingest_device_payload`
     (A5) reuses it directly) — unknown `format_id` is a 422 **before** any
     blob is stored.
  2. Store the files through `upload::store_payload_files`: the same
     name/store-presence guards as `ingest_glb_with_limit`, one owner-quota
     check against the payload's total, and `store_blob` runs the FR-7 GLB
     header + KTX2 checks only when `content_type == GLB_CONTENT_TYPE` — a
     device payload's bytes are opaque by design (R5: "bytes stored opaque")
     and are registered with the **declared descriptor's MIME**, never sniffed
     from the multipart part or the client's `Content-Type`. Each file is
     carried unregistered (`PayloadFile`) in the payload's one DB command,
     which makes it its own `asset` row (owner = token `sub`, anchor = this
     petal, same as a GLB upload — §assets) before the payload row. QA-0161
     (2026-11-08): a `CreateAsset` round trip per file plus the payload's own
     cost two extra reply frames and lane slots per file, the drone flight's
     payload ticks overran the 0.5 s slot (p50 753 ms).
  3. Send `DbCommand::InsertDevicePayloads` (Device `SourceClass`, fixed —
     SPEC-10 §7.1 rule 6 forbids inferring the class from `source_did`, not a
     sound fixed default for a single-route caller; MCP's `ingest_device_payload`
     (A5) is a separate route fixed to `Manual` for the same reason). The DB handler
     re-validates anchor/format/per-asset MIME/timestamp and is the only
     place idempotent retry and referential confinement are enforced; a
     rejection here is 422 (`DevicePayloadIngestRejection`'s `Display`).
     **Known accepted gap**: by this step some `asset` rows from step 2 may
     already be durable even though the payload itself is then refused
     (e.g. an unknown `node_id`) — orphan GC (`asset_gc.rs`, A3) reclaims
     them after the grace window; re-validating per-file MIME/anchor in
     REST before storing would duplicate the DB handler's own checks for a
     narrow win, so it is not done (§5 risk 1 in the campaign plan notes the
     same tradeoff for the pre-existing IoT/asset path).
  4. 201 `{payload_id, asset_ids}` (asset ids keep ingest order — the DB
     handler returns them verbatim, including on an idempotent retry).
- **Client `payload_id` (Phase A security review, 2026-10-15).** REST and
  MCP refuse a non-ULID id (400 / tool error) before any byte is stored; the
  DB door re-checks it. A taken id is a retry only for the same petal and the
  same principal; anyone else gets **409** (`PayloadIdConflict`, mapped by
  `device::payload_rejection_status`) with a body that names nothing about the
  existing payload.
- **MCP device tools are correlated.** `register_device_format`,
  `list_device_formats`, `ingest_device_payload` and `list_device_payloads`
  send a correlation id and accept only the reply that echoes it, and the DB
  arms answer failures with `RequestFailed` (`handlers::correlated_failure`).
  An uncorrelated list reply can otherwise be paired with another caller's
  waiter after a wildcard `Error` skews the queue, handing it another petal's
  rows (fe-runtime §api-reply-correlation).
- **`GET /api/v1/petals/{p}/iot/payloads?node_id&format_id&from&to`** —
  Viewer+, petal scope. Thin `ListDevicePayloadsQuery` → `DbCommand::
  ListDevicePayloads` → `{payloads}`.
- **Placement refusal (renderer protection).** `upload::place_asset_core` —
  shared by REST `create_node`'s `asset_id` path and MCP `place_asset` — now
  checks `asset.content_type == GLB_CONTENT_TYPE` (a `gis::run_select` read)
  AFTER the existing `asset_reach` check and BEFORE `CreateNodeWithAsset`: a
  reachable, existing device-payload asset is refused with
  `upload::NOT_A_GLB_ASSET`, distinct from `UNKNOWN_ASSET` (the caller
  already proved reach, so naming the real reason leaks nothing an
  existence-oracle rule must hide). One refusal point covers both REST and
  MCP — never duplicated per caller. Test:
  `device_asset_cannot_be_placed_as_a_scene_node`.
- **Reading↔payload link** (`iot.rs`, A3 wiring; no A4 code change —
  `IotReadingInput.payload_id` already round-trips through the existing
  `IotIngestRequest` deserialize). Test:
  `readings_posted_with_payload_id_join_to_the_payload`.
- **Correlation discipline**: every DB-thread round trip here follows the
  `upload::register_asset` precedent — a fresh `ulid` `correlation_id`, and
  the reply arm matches on `correlation_id: Some(echoed) if echoed ==
  correlation_id`; a mismatched or family-blind reply is 502, never
  attributed to this caller (fe-runtime `src/AGENTS.md` §api-reply-correlation).

**Known open item (not this slice's owned files — flagged for the Phase A
review checkpoint, §6 of the campaign plan).** The plan's MANDATORY KNOWN
FINDINGS (automated security review of A3, `65f39ec`) name two issues in
`fe-database/src/handlers/{iot_reading.rs,device_payload.rs}`: `
fetch_known_payloads` has no petal scope (a reading in petal P can
reference petal Q's `device_payload`), and the idempotent-retry branch of
`insert_device_payloads` returns `existing.asset_ids` for ANY matching
`payload_id` before validating the caller's petal/principal match the
existing row's. Neither file is in A4's owned-files list (fe-api only); the
review-and-fix is accordingly left for whichever slice/pass owns those
fe-database files next, rather than edited out-of-scope here.

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
  `--features sim-control`) → REST 503 "sim control not configured". MCP
  goes further (SIM-GUI-TOOLS, 2026-10-11): the five `sim_*` rows carry
  `requires: Some(ToolSeam::SimControl)`, so off a lab host they are absent
  from `tools/list` and `tools/call` answers `unknown tool: sim_start` —
  byte-identical to a name that never existed, so a non-lab host neither
  advertises nor confirms the lab surface (§mcp-dispatch).
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
  are `{Global, Owner, requires SimControl}` rows in the §mcp-dispatch table
  (F13); the per-verb guard (and its 503) stays for REST and as defense in
  depth.

## §mcp-dispatch (F13/A26 — DEC-C14, DEC-C15; 2026-10-09)

`src/mcp/mod.rs` owns ONE table, `TOOLS: &[ToolSpec]`
(`{name, description, input_schema, min_role: RoleLevel, scope_rule, requires, handler}`);
`tools/list` and `tools/call` both derive from it. `requires:
Option<ToolSeam>` names a host seam the row depends on (today only
`SimControl` = `ApiState.sim_control_tx`); `tool_available(spec, state)`
filters BOTH `tools/list` and the `tools/call` lookup, so an unwired row is
indistinguishable from an unknown name (`unknown tool: {name}`) and is
checked before authz. `tools/call` =
lookup (available rows only) → `authorize` → schema-`required` check → handler. `authorize` is the
**only** MCP authz code: typed role floor (`require_role_level` — a string
role typo would parse to `RoleLevel::None` = admit-all, so the table is
typed and a test forbids a `None` floor), then the row's `ScopeRule`
resolved **from the DB**, then token containment, then (HierarchyArgs only)
the claimed-ancestry match. Unresolvable, out-of-scope and
ancestry-mismatched calls ALL deny with one text, `rest::NOT_FOUND_OR_DENIED`
("not found or not permitted" — DEC-C21: the old three messages were an
existence oracle). Role failures stay "insufficient permissions"; malformed
ids ("invalid petal_id") and depth escalation ("unexpected petal_id") stay
distinct — they are input-shape errors that reveal nothing stored.
Handlers (`src/mcp/tools.rs`) take
the resolved scope and contain no `require_*`/`resolve_*_scope` calls —
grep-tested (`handlers_contain_no_authz_calls`). Shared cores they call
(sim verbs, hexon/tileset cores, query_guard) keep their own guards as
defense in depth.

| ScopeRule | Meaning | Rows |
|---|---|---|
| `None` | self-filtering read (token scope narrows output) | get_hierarchy, query |
| `Global` | no resource scope exists — role-only by design | create_verse (Manager), sim_* (Owner) |
| `PetalArg(k)` | `args[k]` petal → `resolve_petal_scope` | promote_instance, query_timeseries, upload_asset, place_asset, create_waypoint, import_gpx, set_petal_terrain, list_visibility_groups, set_visibility_group, list_tilesets, install_tileset, get_gis_nodes |
| `NodeArg(k)` | `args[k]` node → `resolve_node_scope` | update_transform, read_node, node_address, delete_node, set/get/delete_property, move_waypoint |
| `VerseArg(k)` | `args[k]` verse → `resolve_verse_scope` (CDL-SPEC10 A5) | register_device_format, list_device_formats |
| `HierarchyArgs(target)` | the id AT `target` (Verse/Fractal/Petal) is DB-resolved; shallower ids sent must match the stored chain; a DEEPER id is rejected outright | create_fractal→Verse, create_petal→Fractal, create_node→Petal |

Role floors: Viewer for reads, Editor for writes (incl. install_tileset),
Manager for create_verse, Owner for sim_*. install_tileset was Manager under
DEC-C14 ("deliberately stricter"); DEC-C21 M5 aligned it to Editor — REST
install and fe-policy `Install` are both Editor, and a stricter MCP row only
split the authority model. Its archive decode is capped at
`terrain::TILESET_ARCHIVE_MAX_BYTES` (11 MiB — the `/mcp` envelope
arithmetic of §asset-ingest; the REST route shares the cap, see there) and
archive parsing + the registry write run on `spawn_blocking` (both
transports).

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

**REST twins (DEC-C21 Mandatory 1).** `POST …/verses/{v}/fractals/{f}/petals`
and `POST …/petals/{p}/nodes` used to build the authz scope from the URL ids
(`build_scope(v, f, p)` + prefix containment), so an Editor of `VERSE#A`
could write into ANY petal by putting `A` in front of it. They now apply the
same discipline as `HierarchyArgs` (`rest::write_target_authorized`): resolve
the WRITE TARGET (fractal / petal) from the DB, require the token to cover
it, then require the URL ancestors to match its stored chain — all denials
404 + `NOT_FOUND_OR_DENIED`, role 403. `create_fractal` resolves its verse
too (existence), and the legacy flat `POST /api/v1/nodes` uses the same
helper. Sweep of rest.rs: every other node/petal handler already DB-resolves
its scope; the field-def by-id routes (`PATCH|DELETE
/api/v1/field-defs/by-id/{id}`) had NO scope check at all (any Manager could
rewrite any verse's schema) and now require the field def's stored scope;
`GET /api/v1/field-defs/{scope}` requires the token to cover `{scope}`.

**Honest arithmetic (DEC-C14; counts updated CDL-SPEC10 A5).** A26's "20
tools" = the mcp_scene_primitives 20-name vocabulary, now fully present. The
table has 36 rows: 31 non-sim + 5 sim — an intentional, documented superset
(read_node, node_address, promote_instance, query_timeseries,
list_visibility_groups, set_visibility_group, sim_* shipped legitimately,
plus A5's 4 R5 device rows below); none are deleted or renamed. `tools/list`
returns 36 on a sim lab host and 31 everywhere else (the GUI always; the
relay without `sim-control`) — `tools_list_inventory` pins both
configurations.

**R5 MCP parity (CDL-SPEC10 A5).** `register_device_format` (Manager,
`VerseArg`), `list_device_formats` (Viewer, `VerseArg`), `ingest_device_payload`
(Editor, `PetalArg`), `list_device_payloads` (Viewer, `PetalArg`) mirror the
REST surface (`device.rs`, §device-ingest) with no second validation path:
`ingest_device_payload` reuses `device.rs::declared_format_mime` (now
`pub(crate)`) and `upload::ingest_blob_with_limit` directly, the same core
REST's multipart route calls. Its `files` arg is base64 (MCP has no
multipart), decoded and SUMMED before any byte is stored — the cap is on the
TOTAL decoded size across the whole payload (`upload::MCP_MAX_ASSET_BYTES`,
the same §asset-ingest envelope arithmetic `upload_asset` uses), not per
file; larger payloads use REST multipart. Its `DbCommand::InsertDevicePayloads
source_class` is fixed to `SourceClass::Manual` (an MCP caller drives ingest
by hand, distinct from REST's fixed `Device` — SPEC-10 §7.1 rule 6 forbids
inferring either from `source_did`). `query_guard::{ALLOWED_TABLES,
PETAL_SCOPED_TABLES}` also gained `DEVICE_PAYLOAD` (§query-guard);
`device_format` stays non-queryable by design (verse-scoped, already has its
own list route).

**Known gaps (F14 / follow-ups).** (`set_petal_terrain` is no longer a gap —
it shares REST PUT/DELETE's correlated core, §terrain-mutation.) `query` inherits
`query_guard`'s verse/fractal-scoped tokens getting NO row filter
(petal-scoped tokens are filtered). `place_asset` refuses an asset
referenced only outside the token's scope, or an unplaced upload anchored in
a petal the token does not cover (§assets — DEC-C21).

## §asset-ingest (F13 — mcp_scene_primitives FR-1/2/3/7)

`src/upload.rs`. Both transports (REST multipart
`POST /api/v1/petals/{p}/assets`, MCP `upload_asset` base64) share
`ingest_glb_with_limit`: name check → `blob_store` present (else fail
closed: REST 503 / MCP error) → owner quota check (below) →
`spawn_blocking { base64 decode (length pre-checked so oversize never
allocates) → validate_glb → BLAKE3 + add_blob }` → `DbCommand::CreateAsset
{name, content_type, size_bytes, content_hash, owner_did, petal_id,
correlation_id}` (an `UploadOwner` = the token's `owner_key` — `client`, else
`sub` — + authorized anchor petal; §assets) → correlated `DbResult::AssetCreated`. The DB side is the public
`handlers::crud::create_asset_reply`, which tests reuse to service the
channel.

- **REST place (MCP-2.4, 2026-10-11).** `POST …/petals/{p}/nodes` and the legacy
  `POST /api/v1/nodes` accept optional `asset_id` / `rotation` / `scale`. With `asset_id`,
  `rest.rs::create_node_in_petal` calls the same `upload::place_asset_core` as MCP
  `place_asset`, AFTER the route's own write-target authz. The response is the house 200
  envelope with `{id, name, asset_id, asset_path}`. `UNKNOWN_ASSET` (unknown OR foreign-scope
  asset) is 404 with MCP's exact text, a malformed id is 400, and values pass through verbatim
  with no unit math (spec D5). With rotation or scale but no `asset_id` the call is 400: plain
  `CreateNode` has no columns for them, and dropping them silently would be worse. With all
  three absent, the legacy path and its response are byte-identical. Tests:
  `tests/mcp_integration.rs` REST-twins lane, on the same emulator and foreign-asset fixture
  as the MCP tests.
- **Bytes never cross the channel — by construction.** `store_glb` consumes
  the `Vec<u8>` and returns `StoredGlb` (private fields: hash + size only);
  `register_asset`, the sole `CreateAsset` sender, accepts only `StoredGlb`.
  The DB thread (`create_asset_handler`) re-verifies `has_blob` and writes
  `data: NONE`. One `BlobStoreHandle` is shared by ApiState and the DB thread
  (`fractalengine/src/main.rs`), so the hash is immediately servable.
- **Validation (FR-7):** GLB only — `glTF` magic, version 2, header length ==
  byte length, ≤ `MAX_ASSET_BYTES` (256 MiB, per-node config is a seam).
  Then the KTX2 intake guard (`fe_runtime::ktx2_guard::check_glb`, fe-runtime
  `src/AGENTS.md` §ktx2-guard, 2026-10-13): a GLB carrying a KTX2/Basis
  texture is `UploadError::Ktx2Texture` (HTTP 422; RUSTSEC-2026-0344 interim
  mitigation), and malformed chunks or glTF JSON are `MalformedGlb` (400).
  Both are refused before `add_blob`, so nothing is stored. Other texture
  formats are not parsed. Test: `tests/asset_ownership_test.rs`
  `ktx2_texture_upload_is_refused_and_writes_nothing`.
- **Limits (NFR-2, DEC-C21):** asset route `DefaultBodyLimit` = 256 MiB +
  1 MiB — **large GLBs go through REST multipart**. `/mcp` =
  `MCP_ROUTE_BODY_LIMIT` = **16 MiB for every JSON-RPC method** (it used to
  be ceil(4/3 × 256 MiB) + 8 MiB ≈ 349 MiB on every method, a DoS surface).
  MCP `upload_asset` decodes at most `MCP_MAX_ASSET_BYTES` = 11 MiB: its
  base64 is 11 × 4/3 ≈ 14.67 MiB, leaving ~1.3 MiB for the JSON-RPC
  envelope. Honest peak per MCP call: the <= 16 MiB body (the raw bytes plus
  the parsed `serde_json::Value` — roughly 2× the body while parsing), the
  arguments MOVED out of `params` (`Value::take`, never cloned), one copy of
  the base64 string handed to the decode task (`ToolCall.args` is borrowed,
  so the handler cannot move it), and the decoded bytes (<= 11 MiB) on
  `spawn_blocking` → ~45 MiB transient per call at the cap; at most `MCP_MAX_CONCURRENT_CALLS` = 8 calls run at
  once (a semaphore middleware with tower `ConcurrencyLimit` semantics —
  excess requests wait for a permit before their body is read;
  unauthenticated requests are rejected by the auth layer first) → a few
  hundred MiB worst case, versus multiple GiB before. (The previous
  "~600 MB" figure assumed one call at the old 349 MiB cap; it was never a
  bound.) **Tileset install (FEAPI-TILESET-2MIB, 2026-10-11):** REST
  `POST /api/v1/hexons/tilesets/install` carries `DefaultBodyLimit` =
  `TILESET_ROUTE_BODY_LIMIT` (11 MiB + 1 MiB multipart slack) and refuses an
  archive over `TILESET_ARCHIVE_MAX_BYTES` = 11 MiB with an explicit
  `ok:false` message (house-style 200 envelope); MCP `install_tileset` decodes
  at most the same 11 MiB. Both import the archive through
  `fe_format::HexonArchive::import`, whose per-entry/total decompression caps
  (fe-format AGENTS.md §decompression-limits) bound what the larger archive
  can inflate to. Known inefficiency, left as is: `install_tileset_bytes`
  imports the archive twice (the ownership inspection, then
  `TilesetRegistry::install`). Other routes keep axum's 2 MiB default. GPX via MCP caps at
  `MCP_GPX_MAX_BYTES` = 11 MiB decoded (same envelope arithmetic) and BOTH
  GPX transports cap parsed points at `limits::GPX_MAX_POINTS` = 10 000
  (each trackpoint is a `CreateNode` round trip; REST answers 413).
- **Upload quota (FEAPI-BLOB-GC, 2026-10-11).** `ApiState.upload_quota_bytes`
  (env `FE_API_UPLOAD_QUOTA_BYTES`, read once by both binaries via
  `upload_quota_bytes_from_env`; default 2 GiB, an unparseable value warns
  and uses the default). `ingest_glb_with_limit` checks it right after the
  name check and BEFORE any byte is decoded or stored:
  `SELECT math::sum(size_bytes) … WHERE owner_did = $owner GROUP ALL` (via
  `gis::run_select`, `idx_asset_owner`) + the incoming size — the exact
  length for multipart, the base64-derived upper bound for MCP. Over →
  `UploadError::QuotaExceeded` (REST 413, MCP tool error) and neither a blob
  nor a row is written; a failed lookup fails closed (`RegisterFailed`).
  Keyed by `owner_did` = the token's `owner_key` (§assets): each API client
  has its own bucket, and legacy (client-less) tokens share one node-level
  bucket keyed on `sub`. Client churn per node is bounded by the per-sub
  active-token cap (50, `api_token_store::MAX_ACTIVE_TOKENS_PER_SUB`). Soft: two
  concurrent uploads can each pass the check and overshoot by one upload.
  Re-uploading identical bytes counts again (each upload adds a row).
- **Blob GC (FEAPI-BLOB-GC, 2026-10-11).** A fire-and-forget
  `DbCommand::GcUnreferencedAssets { grace_secs }` (fe-runtime src/AGENTS.md
  §asset-gc: Bevy timer, `FE_ASSET_GC_INTERVAL_SECS` default 3600, `0`
  disables; `FE_ASSET_GC_GRACE_SECS` default 86 400) runs
  `fe_database::handlers::asset_gc` on the DB thread: delete `asset` rows that
  no `node` row references (tombstoned nodes count — conservative) and whose
  `created_at` is older than the grace (unparseable timestamps are kept);
  then `BlobStore::remove_blob` each of their hashes that no remaining
  `asset` row, no `crate_entry.asset_hash` and no node path stamp
  (`properties.path_asset.asset_path = blob://{hash}.{ext}`) names. **Invariant: the blob
  directory is never scanned for orphans** — replication row blobs
  (`replicate_row`) and hexon blobs share the store and are not
  asset-referenced. The single-writer DB thread makes the reference check and
  the delete atomic with respect to placements. Residual: an upload whose
  bytes equal a just-collected blob can race the GC between the API's
  `add_blob` and `CreateAsset`; `create_asset_handler` re-verifies `has_blob`,
  so that upload fails (`RegisterFailed`) instead of leaving a dangling row —
  a retry succeeds.
- **Placement:** `place_asset` → `DbCommand::CreateNodeWithAsset` (log-first
  `NodeCreated` op, rotation stored as Euler XYZ like `UpdateNodeTransform`)
  → `DbResult::GltfImported` with an echoed `correlation_id`. Only a
  correlated `GltfImported` is an API reply (`ReplyKind::PlacedAsset`); a GUI
  `ImportGltf` result (`None`) can never satisfy an API waiter, and the GUI's
  existing listener still spawns the placed node live.
- **Typed DB-layer failure (2026-10-11, RT-CORR-EDGE).** A correlated
  `CreateAsset` / `CreateNodeWithAsset` that fails on the DB thread is
  answered with `DbResult::RequestFailed { error, correlation_id }`
  (`register_asset` → `RegisterFailed` 502; `place_asset_core` → `unknown
  asset` / `operation failed`) — immediately. It used to be the wildcard
  `Error`, which never reaches a correlated waiter, so the caller hung for
  the full 5 s `db_round_trip` timeout.
- Re-uploading identical bytes is idempotent at the blob layer but creates a
  new asset row (dedupe-by-hash is out of scope).

## §operator-gate (`operator_gate.rs`, 2026-10-31; per-gate default added agent_vision_20261031)

Every deny-by-default operator surface (§elevated-query, §screenshot,
§camera-state, §agent-input-surface) runs ONE guard chain — a **guard
chain** pattern, declared as data: `OperatorGate { opt_in_env, endpoint,
enabled_warning, disabled_message, rate_key_prefix, rate_per_sec, rate_label,
default_on }` (`fe_api::ELEVATED_QUERY_GATE`, `screenshot::SCREENSHOT_GATE`,
`vision::CAMERA_STATE_GATE`, `input::INPUT_GATE`). A new gated endpoint adds a `const` and calls
the gate — it does not copy the chain. `OperatorGate` itself is edited once
(agent_vision_20261031 V2 added `default_on`); every endpoint added after
that only adds a `const`.

- **Per-gate default (`default_on`, agent_vision_20261031 DD-1).** Each gate
  decides what an UNSET (or `""`) env value means:
  `opt_in_value_enables(value, default_on)` resolves unset/`""` to
  `default_on`, `"1"` always enables, `"0"` always disables, and any other
  value always disables (fail closed) regardless of `default_on`
  (table-tested both columns: `per_gate_default_decides_the_unset_and_garbage_cases`).
  `ELEVATED_QUERY_GATE`/`INPUT_GATE` keep `default_on: false` (the
  pre-existing opt-in-only posture); `SCREENSHOT_GATE`/`CAMERA_STATE_GATE`
  set `default_on: true` — an agent gets visual verification out of the box,
  and the operator opts OUT with `=0` instead of opting in with `=1`.
- `opted_in_from_env()` — read once at startup against the gate's
  `default_on`; loud WARN whenever the result is on (a default-on variant
  naming the env var to set to `0`, when the value was unset; the plain `=1`
  variant otherwise), a warning for any unrecognized value (fail closed
  either way), silence for an explicit `0` or an unset default-off gate. The
  result is stored on `ApiState` (a field, never a per-request env read) so
  tests flip it.
- `admit(allowed, state, claims)` — the REST chain: `check_opted_in` → typed
  `require_role_level(Owner)` → `check_rate` (the shared
  `query_guard::check_rate_limit` map, key `<prefix>:<did>`). Refusals are a
  `GateRefusal { Disabled, InsufficientRole, RateLimited }` whose `message()`
  text and `status()` (403 opt-in/role, 429 rate — the ONE mapping every
  surface's error enum delegates to) are identical on every surface; each
  surface keeps its own response shape (elevated: legacy 200 + error body;
  screenshot/camera/input: 403/429).
- **Composed gates.** A surface whose output crosses into another gate's
  territory runs that gate too, rather than widening its own: input's
  `capture_after` returns pixels, so `input::check_capture_after_gate` runs
  `SCREENSHOT_GATE.check_opted_in` + `check_rate` (§agent-input-surface).
- **MCP.** The dispatcher owns authz (FR-5: `tools.rs` may not call a
  `require_*` guard), so an MCP handler calls `check_opted_in` +
  `check_rate` only; the dispatcher refuses the opt-in BEFORE `authorize`
  (`operator_opt_in_refusal`) and enforces the Owner floor from the tool row —
  so the order matches REST.
- `FE_SIM_ALLOW` is NOT on this gate on purpose: the sim seam's opt-in decides
  whether the bridge is spawned at all (fail-closed `NotConfigured`), and it
  has no rate budget (§sim-control).

## §screenshot (`screenshot.rs` + `vision.rs`, agent visual verification; dynamic capture added agent_vision_20261031)

`GET /api/v1/debug/screenshot` returns `image/png` of the GUI's primary window
— the 3D viewport **and** every egui panel, exactly what the user sees. The MCP
`screenshot` tool returns the same capture as an MCP image content block
(`{"type":"image","mimeType":"image/png","data":<base64>}` plus a short text
block) when called with empty arguments. It exists so an agent driving the
API can SEE the result of its edits. **`POST` the same path** with a JSON
body (or MCP `screenshot` with any arguments) runs a **dynamic posed
capture** instead — the agent can tell it WHERE to look (an exact camera, an
orbit, a lat/lon, a slippy tile, or a node/petal's AABB) and whether to move
the real window camera or render off-screen; see the wire contract and the
static validation table in `fe-runtime/src/AGENTS.md` §vision-bridge. The
`GET` path's bytes are byte-for-byte unchanged by any of this (a hard
tripwire); its only behaviour change is the SEC-ADDENDUM scope check.

It is an **exfiltration surface**: a frame shows whatever verse is open, so
the host confines every capture and camera read to a token whose scope covers
that verse ("Scope confinement" below), and a posed scene capture can still
look anywhere in the active petal without the user noticing. So the posture mirrors
§elevated-query (deny-by-default), with one change:

- **Operator opt-in, ON BY DEFAULT (agent_vision_20261031 DD-1).**
  `ApiState.screenshot.allowed` is read once at startup from
  `FE_API_SCREENSHOT_ALLOW` (`screenshot::screenshot_allowed_from_env`,
  `SCREENSHOT_GATE.default_on == true`): **unset admits an Owner**; `=0`
  refuses with the disabled message; `=1` admits (and warns the same as
  before); any other value fails closed and warns. A field, not a
  per-request env read, so tests set it
  (`ApiHarness::spawn_with_screenshot`). Off → every call (GET, POST, MCP,
  and `GET /camera`) gets its gate's disabled message (REST 403) before
  role, rate limit or bridge. On MCP the dispatcher refuses BEFORE
  `authorize` (`mcp/mod.rs::operator_opt_in_refusal`), so the order is
  identical there. §operator-gate has the full default-on/default-off table.
- **Gate order (GET/legacy MCP): opt-in → Owner (typed) → 1/s per DID (key
  `screenshot:<sub>`, REST 429) → bridge.** **Gate order (POST/dynamic MCP,
  spec §5.7): opt-in → Owner → 1/s per DID → static validation (400) →
  single-flight slot (409) → bridge.** Validation runs AFTER the gate on
  purpose: an unauthenticated or un-opted caller never learns the parameter
  grammar, and a refused caller never costs a parse. Owner of any scope
  passes THIS gate; the host then confines the capture to the token's scope
  ("Scope confinement" below).
- **The plain-GET bridge.** `ApiState.screenshot.bridge` is a bounded (4)
  crossbeam `fe_runtime::screenshot::ScreenshotRequest` channel carrying a
  ULID correlation id + a oneshot reply slot (fe-runtime src/AGENTS.md
  §screenshot-bridge). The API `try_send`s (full → 503 "busy", disconnected →
  503 "render host stopped") and awaits the reply for
  `ScreenshotSurface.reply_timeout` (5 s; 504 "screenshot timed out" — never a
  hang); a reply echoing a foreign correlation id is rejected. The Bevy side
  (`fractalengine/src/screenshot_bridge.rs`) serves every queued request with
  ONE `Screenshot::primary_window()` capture and PNG-encodes it on the compute
  pool.
- **The dynamic-capture bridge (`vision.rs::request_vision`, a SECOND,
  separate bridge — fe-runtime src/AGENTS.md §vision-bridge explains why).**
  `ApiState.screenshot.vision` is the same shape (bounded(4), `try_send`,
  correlation id, reply timeout), wired via `ScreenshotSurface::with_vision`.
  A posed capture additionally holds `ApiState.screenshot.capture_slot` (a
  1-permit `tokio::sync::Semaphore`, `try_acquire_owned`) for the whole
  round trip: a second posed capture arriving while one is in flight gets
  409 `capture_in_progress` WITHOUT the slot ever touching the bridge
  (`vision::run_capture`, shared by the REST `POST` handler and the MCP
  `screenshot` handler so REST and MCP contend for the same single flight).
  The reply timeout for a posed capture is `reply_timeout + 20s` (a settle
  wall-clock cap; `GET /camera`'s `ReadCamera` op keeps the plain
  `reply_timeout`, since it never settles).
  `CaptureSpec::validate()` (fe-runtime §vision-bridge) is the only
  constructor of `ValidCaptureSpec` that reaches the bridge — a 4 KiB body
  cap and `deny_unknown_fields` both collapse into one `malformed_body`
  refusal at the REST/MCP boundary (fe-runtime's `CaptureSpec` doesn't know
  about raw bytes). An empty `POST` body is a bare default (window, no
  pose) capture, not an error.
- **Headless/unwired hosts.** `ScreenshotSurface::disabled()` (relay, harness
  default) wires neither bridge — all three routes answer 503 "no window",
  and the MCP rows (`screenshot`, `get_camera`) are not even advertised
  (`ToolSeam::Screenshot`, available iff EITHER bridge is wired).

How an agent calls it (GUI started with `FE_API_SCREENSHOT_ALLOW` unset, an
Owner API token read from the GUI's opt-in dev-token file — §qa-dev-token;
the Inspector's mint door caps at `manager`):

```bash
# Plain capture — unchanged.
curl -fsS -H "Authorization: Bearer $FE_TOKEN"   http://127.0.0.1:8765/api/v1/debug/screenshot -o frame.png

# Posed scene capture: look straight down at a slippy tile, no UI, off-screen.
curl -fsS -X POST -H "Authorization: Bearer $FE_TOKEN" -H "Content-Type: application/json" \
  -d '{"mode":"scene","pose":{"tile":{"z":14,"x":8192,"y":5461}},"width":1024,"height":1024}' \
  -D - http://127.0.0.1:8765/api/v1/debug/screenshot -o tile.png
# -D - prints the X-FE-Capture-Pose header alongside the PNG.
```

MCP: `tools/call` with `{"name":"screenshot","arguments":{}}` for the plain
capture, or `{"name":"screenshot","arguments":{"mode":"window","pose":{"orbit":{...}}}}`
for a posed one (the response's second content block is the resolved pose
JSON). Humans do not need any of this: `F12` in the app saves a timestamped
PNG to `data/screenshots/` and toasts the path (local keyboard — no API gate).

Tests: `tests/screenshot_test.rs` (the plain `GET` path: refused when not
opted in, refused below Owner, admitted capture returns the host's bytes
verbatim then rate-limits, silent host times out) over a fake capture
responder — renderer-free. `tests/vision_test.rs` (the dynamic `POST`/MCP
path + `GET /camera`) covers the static validation table, the round trip
(PNG + pose header/text, equal to what the fake vision responder received),
the single-flight 409, a dynamic Bevy-side refusal mapping to 422, the
camera-budget independence, the disabled/headless cases, and both MCP shapes
— also renderer-free.

### Scope confinement + `include_ui` (V5 addenda: SEC-ADDENDUM, UI-ADDENDUM)

- **Default-on is bounded by scope.** Every capture (plain `GET`, posed
  `POST`, MCP `screenshot` with or without arguments) and every camera read
  (`GET /api/v1/debug/camera`, MCP `get_camera`) carries the caller's token
  scope across the bridge (`ScreenshotRequest.caller_scope`,
  `VisionRequest.caller_scope`). The Bevy host refuses unless that scope covers
  the ACTIVE verse (fe-policy `scope_contains` against `VERSE#<active>`; with
  nothing open, no one is covered) AND every petal still rendered (the terrain
  binding, which outlives a verse switch, and not-yet-despawned nodes —
  fractalengine src/AGENTS.md §vision "Scope confinement"), and checks it again
  at capture time, so a verse-B Owner token never receives verse-A scene
  pixels or projection. A token scoped below a verse (fractal/petal) never
  covers one, so it cannot capture at all (fail closed). **Residual, by
  design (UI-ADDENDUM):** a window capture (`include_ui: true`, and the plain
  `GET`) is the whole window, so UI chrome that lists other verses (the
  hierarchy sidebar's verse names, an open dialog) is in the frame. Scope
  confinement covers the scene, not the chrome; on a node shared between
  tenants use `include_ui: false` or set `FE_API_SCREENSHOT_ALLOW=0`. Both doors call the same
  `request_capture(state, &claims)` / `run_capture(state, &claims, spec)` /
  `request_vision(state, &claims.scope, ..)`, so there is no MCP-only gap. `focus` targets are resolved to their verse first, and an
  out-of-scope target and a nonexistent one answer IDENTICALLY (§existence-oracle).
  `navigate: true` never leaves the active verse (`cross_verse_navigate_refused`),
  and a navigating job puts the user's navigation back on failure, timeout or
  `restore: true`. The denial is the uniform 404
  `{"ok":false,"error":"not found or not permitted","code":"not_found_or_denied"}`
  (REST), and the same text as the MCP tool error.
- **Audit (critic pass 2026-11-01).** Every capture that clears the gate (and,
  for a posed one, the single-flight slot) writes one INFO line before it
  crosses the bridge — `vision::audit_capture`: `did`, `scope`, `mode`
  (`window`/`scene`) and `pose` kind (`none (plain)`, `none`, `camera`,
  `orbit`, `geo`, `tile`, `focus`). It lives in the shared cores
  (`request_capture`, `run_capture`), so neither door can skip it. Scene
  captures stay invisible on screen by design: a per-capture toast was
  considered and deferred, because it would land in the very window
  captures an agent uses for UI QA (revisit if multi-tenant hosting lands).
- **`include_ui`** (POST body + MCP `screenshot`): include_ui (default true):
  include the app's UI panels in the image; false renders only the 3D scene
  off-screen without moving the user's view. It is the same switch as `mode`
  (`true` = `window`, `false` = `scene`). Giving both with different answers is
  the typed 400 `include_ui_conflicts_with_mode` (fail closed, no precedence).
  `width`/`height` need `include_ui: false`, because a UI capture is always the
  window's own size. The `X-FE-Capture-Pose` header and the MCP pose text carry
  `include_ui` beside `mode` (`vision::pose_metadata_json`).
- **One model, one parse path, one round trip (advocate pass, 2026-11-01).**
  REST and MCP parse into the same `CaptureSpec` through
  `vision::validate_capture_request` (serde failure = `malformed_body`, then
  `CaptureSpec::validate`'s typed table); MCP errors carry the same `code`
  prefix REST puts in its JSON (`VisionError::coded_message`). Both bridges go
  through `bridge_client::round_trip(bridge, caller_scope, op, timeout, name)`
  — the API end of fe-runtime §correlated-bridge — so the caller's scope can
  never be left off a request; each surface only maps `RoundTripError` to its
  own typed error.

## §camera-state (`vision.rs::get_camera`, agent_vision_20261031)

`GET /api/v1/debug/camera` (+ MCP `get_camera`) answers an agent's half of
the closed QA loop `get_camera -> screenshot(pose) -> get_camera`: the live
main-camera pose, whether a posed capture currently holds it (`locked`), the
active verse/fractal/petal, and the terrain projection (grounded
`origin_ele`, world scale, tileset bounds/zoom range when a tileset is
installed) plus the window's size/minimized state.

- **Same opt-in as screenshot, own rate budget.** `CAMERA_STATE_GATE` reads
  the SAME `FE_API_SCREENSHOT_ALLOW` field (`ApiState.screenshot.allowed`) —
  one operator switch covers both surfaces — but has its own rate key
  (`camera:<sub>`) and a 10/s budget, specifically so the closed loop never
  burns the screenshot surface's 1/s capture budget
  (`camera_state_round_trips_and_does_not_spend_the_capture_budget`).
- **Goes over the vision bridge's `ReadCamera` op**, answered by the Bevy
  host immediately in any state (even mid-capture, with `locked: true`) —
  never the single-flight slot, which only a `Capture` op touches.
- **MCP row sharing.** `get_camera`'s `ToolSpec.requires` is
  `ToolSeam::Screenshot` (not a separate seam) — availability only needs
  EITHER bridge wired, but `operator_opt_in_refusal` special-cases the tool
  name to report `CAMERA_STATE_GATE.disabled_message` instead of the
  screenshot one, so a disabled node's error text still names the right
  endpoint.

## §agent-input-surface (`input.rs`, folded-in `agent_input_surface_20261031`
via `agent_vision_20261031`; wiring landed plan I3)

S4 extends the existing QA core with `POST /api/v1/debug/dialog-answers` and `POST /api/v1/debug/window`,
plus MCP `dialog_answers` and `window`. The shared default-off Owner gate and active-verse check run on both doors;
host mutations additionally require a validated isolated QA root and the actual loopback listener notification.
Bodies are capped at 64 KiB. REST dialog answers accept an array or `{answers:[...]}`; MCP uses the latter.
Window actions are `size`, `ui_scale`, `maximize`, `restore`, `minimize`, and `close`. Close requests confirmation.
App UI scale changes only the application's egui scale, never system display settings.

Clipboard isolation also applies at the GUI sink: the input bridge removes egui
`CopyText` and `CopyImage` output before platform processing throughout injection,
settling/capture, and the final success or abort frame. The planner still refuses
clipboard chords. Idle user copies remain enabled; audits contain counts, not payload.

`POST /api/v1/debug/input` (+ MCP `inject_input`) synthesizes pointer,
keyboard and scroll input into the running GUI, so an agent can close the
loop `get_camera -> screenshot(pose) -> inject_input -> screenshot`. It is
the one surface in this family that **mutates** state rather than observing
it, so its posture deliberately diverges from §screenshot/§camera-state in
exactly one place:

- **Operator opt-in, OFF BY DEFAULT.** `ApiState.input.allowed` is read once
  at startup from `FE_API_INPUT_ALLOW` (`input::input_allowed_from_env`,
  `INPUT_GATE.default_on == false` — the pre-existing opt-in-only posture,
  §operator-gate): unset or anything but `1` refuses with the disabled
  message; `=1` admits and WARNs that, combined with the default-on
  screenshot/camera surfaces, an Owner token now has full remote control of
  this host's GUI. A field, not a per-request env read, so tests flip it
  (mirrors `ApiHarness::spawn_with_screenshot` — `spawn_with_input` is its
  `InputSurface` counterpart, fe-test-harness src/AGENTS.md §api-harness).
- **Gate order (mirrors §screenshot's POST, spec AIS §4.5): opt-in → Owner
  (typed) → 5/s per DID (key `input:<sub>`) → static validation (400) →
  bridge.** A whole command batch counts once against the rate budget, since
  a UI interaction is naturally multi-command, unlike a 1/s capture. On MCP
  the dispatcher refuses the opt-in BEFORE `authorize`
  (`operator_opt_in_refusal`, `ToolSeam::Input`), so the order is identical
  there; `tool_available` hides the row entirely when `ApiState.input.bridge`
  is unwired (headless/relay hosts, every harness default).
- **One parse-and-validate path, shared by both doors**
  (`input::validate_input_request`): a structural serde failure (including
  `InputSpec`'s `deny_unknown_fields`, or REST's 64 KiB body cap) is
  `malformed_body`; everything else is `InputSpec::validate`'s typed table,
  run in table order (fe-runtime src/AGENTS.md §input-bridge has the full
  command vocabulary and the per-field `non_finite`/`invalid_key`/
  `text_too_long`/`wait_frames_out_of_range` checks). `ValidInputSpec` has no
  public constructor besides `validate()` — no float or unmapped key crosses
  the bridge unchecked.
- **The bridge.** `ApiState.input.bridge` is `fe_runtime::input::InputRequest`
  — a `BridgeRequest<InputOp, InputOutcome>` alias on the shared
  §correlated-bridge envelope (fe-runtime §input-bridge explains why input
  reuses that envelope rather than mirroring screenshot's own bridge shape).
  Both doors go through `bridge_client::round_trip(bridge, caller_scope, op,
  timeout, name)`, so the caller's scope can never be left off a request —
  exactly like vision's capture/camera ops. `input::request_inject` maps
  `RoundTripError` to `InputError` (busy/host-gone/timeout/uncorrelated) —
  except a missed deadline, which it first maps through the spec's shared
  `InputProgress` (fe-runtime §input-bridge "Truthful late replies"): all
  frames applied → **202** `{ok:true, settled:false, code:"applied_unsettled"}`
  on REST and the same fields on MCP (never a 504 that invites resending a
  world edit, QA-0034/QA-0026); otherwise 504 `input_partially_applied` (k of
  n) or `input_not_started`. A settled reply is 200 with `settled:true`. And
  `input::run_inject` (shared by the REST handler and the MCP `inject_input`
  handler) audits before crossing the bridge (`input::audit_inject`: did,
  scope, command count, wait_frames, capture_after — mirrors
  `vision::audit_capture`).
- **Scope confinement is check-at-use, not an up-front resolve.** Unlike
  `PetalArg`/`NodeArg` tools, `inject_input`'s `ScopeRule` is `Global` — the
  caller's scope crosses the bridge as `caller_scope` and the Bevy host
  refuses unless it covers the active verse AT DRAIN TIME AND AT INJECTION
  TIME (`§INPUT-ADDENDUM` item 2), the same "check at use, never an
  existence oracle" rule §existence-oracle states for REST. An out-of-scope
  target and a nonexistent one answer IDENTICALLY:
  `fe_runtime::input::DENIED_MESSAGE` is byte-identical to
  `rest::NOT_FOUND_OR_DENIED` and to `fe_runtime::vision::DENIED_MESSAGE`
  (asserted in `input.rs`'s own test), so a denied injection reads exactly
  like a denied capture.
- **`capture_after` composes the screenshot gate (`§INPUT-ADDENDUM` item
  6).** The attached PNG is a capture, so it must not dodge the capture
  surface's own posture: `input::check_capture_after_gate` (inside
  `run_inject`, so REST and MCP share it) refuses `capture_after: true` with
  the screenshot gate's own refusal when `FE_API_SCREENSHOT_ALLOW=0`, and
  spends the `screenshot:<sub>` 1/s budget; the Owner floor is already the
  input gate's, and the host's per-frame view-scope check (fractalengine
  §agent-input-surface) covers the capture frame. A batch without
  `capture_after` never touches the screenshot gate.
- **Interaction with a posed capture (`§INPUT-ADDENDUM` item 4).** While
  `CameraCaptureLock` holds the main camera for a vision job, an injection is
  refused `camera_locked_by_capture` (409) rather than fighting the held
  pose — synthetic input and a posed capture never run concurrently.
- **Success body** (REST JSON, and the MCP tool's JSON-as-text payload):
  `commands_applied`, `wait_frames_used`, and an optional
  `capture_after_png_base64` (present only when `capture_after: true` asked
  for a post-action PNG, captured through V5's shared `capture_png` helper —
  the field is OMITTED, not `null`, when absent). REST is JSON-only on this
  route (unlike the posed-capture route's raw-PNG body + pose header): an
  injection's primary payload is the outcome, with the image as an
  attachment.
- **Headless/unwired hosts.** `InputSurface::disabled()` (relay, harness
  default) answers both doors 503 "no window", and the MCP `inject_input` row
  is not even advertised.

How an agent calls it (GUI started with `FE_API_INPUT_ALLOW=1`, an Owner
token):

```bash
curl -fsS -X POST -H "Authorization: Bearer $FE_TOKEN" -H "Content-Type: application/json" \
  -d '{"commands":[{"kind":"pointer_click","x":640,"y":360,"button":"left"}],"wait_frames":5,"capture_after":true}' \
  http://127.0.0.1:8765/api/v1/debug/input
```

MCP: `tools/call` with `{"name":"inject_input","arguments":{"commands":[{"kind":"key_press","key":"Enter"}]}}`.

Tests (one journey per region, 2026-11-01 test policy): the REST door's
gate/validation/error-mapping table is `input.rs`'s own journey; the wire
contract's validation table and bridge-capacity bound are
`fe-runtime/src/input.rs`'s; the full-router flow is
`tests/vision_test.rs::input_injection_disabled_role_round_trip_rate_limited_headless_and_camera_lock_journey`
(disabled, role floor, empty-text `no_effective_input` 400, `capture_after`
refused by the screenshot gate on both doors, admitted round trip with the
PNG attachment, 5/s rate limit, headless, the 409 camera lock); the Bevy
translation is `fractalengine/src/input_bridge.rs`'s journey.

## §qa-dev-token (`dev_token.rs`, headless agent bootstrap, 2026-11-01)

`FE_API_DEV_TOKEN_OUT=<path>` makes the GUI write loopback-only API tokens for
every local verse to `<path>` once the API is listening, and rewrite the file
when the verse set changes — the way a headless QA agent gets the Owner token
§screenshot/§camera-state/§agent-input-surface need (the Inspector's mint
door caps at `manager`). The host wiring, file format and security posture
live in `fractalengine/src/AGENTS.md` §qa-dev-token; this crate owns:

- `emit_dev_tokens(path, bound, db, keypair, client_did, verse_ids)` — the
  loopback check FIRST (`loopback_api_base`; a non-loopback `bound` mints and
  writes nothing), then one `fe_identity::mint_client_api_token` per verse at
  `role_manager::resolve_role` of the node's own DID (never above it; none →
  no token), 8 h TTL (`DEV_TOKEN_TTL_SECS`), written atomically owner-only via
  `fe_identity::file_fallback::replace_owner_only_file`. Returns a
  token-free `DevTokenEmission` that is safe to log.
- `spawn_dev_token_writer(DevTokenWriter)` — the thread: waits on
  `ApiConfig::listening_tx` (the actual bound address, sent by `run_server`
  right after `bind`), refuses a non-loopback bind for the life of the
  process, then re-emits per (coalesced) verse-id list.
- `ApiConfig::listening_tx` is `None` on the relay and in every harness, so
  nothing else changes.

Flow test: `tests/screenshot_test.rs::owner_capture_via_the_dev_token_file_returns_the_host_png_then_is_rate_limited`.

## §qa-debug / §ui-tree

`GET /api/v1/debug/ui-tree`, `/ui-state`, and `/logs` share one read core with
MCP `ui_tree`, `ui_state`, and `ui_logs`. All are default-OFF under the captured
`FE_API_INPUT_ALLOW=1` flag, require Owner, and spend a separate per-DID QA
budget of 20 reads/sec. Injection keeps its independent 5/sec budget. Headless
hosts have no QA seam and do not advertise these MCP tools. The GUI checks the
caller's scope against the active rendered view at use; denial has the same
`not_found_or_denied` / `not found or not permitted` contract as vision/input.

Tree filters: `name` (case-insensitive substring, max 256 bytes), `role`
(case/separator-insensitive role name, max 64 bytes), `interactive=1|0`.
Bounds are PHYSICAL screenshot pixels and can be sent directly to the existing
input endpoint. Each response includes `coordinate_space`, frame, active verse,
pixels-per-point and primary-window dimensions. Nodes retain stable ids, parent,
role, resolved accessible name, redacted value, enabled/hidden ancestor state,
focus, selection and toggle state. Password values and descendant text values
are omitted. This export does not consume or alter the OS accessibility tree.

UI state projects authoritative navigation/selection/Transform, active tool,
open surfaces, visible notices/status chips, physical viewport rect and exposed
pending operations. DB/sync pending totals are null because no authoritative
counter exists. `settled` explicitly covers UI gestures, capture lock and known
UI operation queues; it does not certify asynchronous DB/sync/terrain completion.

Logs accept `since=<unsigned sequence>` and `level=trace|debug|info|warn|error`
(minimum severity). The result includes entries, latest_seq and generation.
Historical records clear on an active-verse boundary; scope is checked before
reading. Logs are redacted at ingest and bounded; see fe-runtime §qa-debug for
attribution limits. The bridge refuses a frame whose navigation changed while
it was rendered, allowing clients to retry rather than reading a stale tree.

## Property reply identity

REST, MCP and WebSocket property operations share the property core methods. Each method supplies a fresh correlation ID and requires the expected reply variant, echoed ID, node ID and applicable key. Correlated failures must echo the same ID. The projection fallback uses the same checked read core; route-tracker background property writes also carry fresh IDs despite dropped receivers. WebSocket errors retain the client-safe operation failed envelope while logging internal detail. The API never waits for a node to become selected in the GUI.

## �property-schema

GET/PUT `/api/v1/field-defs/by-id/{id}/schema` and MCP
`get_field_schema` / `set_field_schema` share the same core. Reads require Viewer,
updates Manager, both at the definition's stored scope (foreign/unknown share one
denial). PUT body is `{version:1,schema:{type:"object",properties:{gain:{type:"number"}},required:["gain"],additionalProperties:false}}`.
This is the bounded property-schema vocabulary documented in fe-sdk/src/AGENTS.md,
not JSON Schema. Unknown keywords are errors. GET returns current descriptor and
immutable accepted revision list; legacy definitions have no enforced descriptor.
Updates dry-run existing values/defaults and require the next revision; no implicit
migration, automatic default insertion or outer required-field constraint. The
correlated committed reply is independent of GUI selection. Legacy field-def
constructors and metadata-only behavior remain compatible.

## �node-history

POST `/api/v1/petals/{p}/objects` accepts `{name,asset_id?,position?,rotation?,scale?,properties?:{...}}`.
Transform defaults are zero position/Euler XYZ radians and unit scale. Initial
properties are validated before one atomic create+baseline commit; optional
asset_id uses the same scoped reach/GLB gate as place_asset. Without an asset this
is a data-only node. No filesystem path is accepted. Existing create routes keep
their constructors and reply forms.

POST `/api/v1/nodes/{n}/mutations` accepts tagged `property_set {key,value}`,
`property_deleted {key}`, or `full_transform {position,rotation,scale}`. Actor is
the validated token subject, not a request field. Creation/mutation returns
`{node_id,history_version,state,persist:"committed"}` inside ApiResponse; state is
the authoritative normalized node-table snapshot (position XYZ). Existing
property REST/MCP/WS DTOs stay compatible while using this attributed commit core.
Existing async transform remains compatible; use mutations for a committed receipt.
There is no effective-time override on this contract; commit order is authoritative.

GET `/api/v1/nodes/{n}/history?after_version=0&limit=100` returns entries ordered by
row_version, next_after_version (null at end), journal_version, current_version,
coverage_start_version/time, supported_operations and current_state_verified.
GET `/api/v1/nodes/{n}/state?at_version=N` adds availability, exact at_version,
baseline_version and state only when replay is available. Legacy gaps are explicit.
Explicitly tagged atomic Saves add optional `atomic_save` grouping metadata to retained history
entries and state reads. An interior exact cut reports `inside_atomic_batch` and its completed Save
version with no state; unverifiable receipt groups report `incomplete_atomic_batch`. Page cursors
still enumerate every audit fact. Snapshot fields for available versions retain their existing shape.
MCP get_node_history/read_node_at_version/commit_node_mutation share the same core.
History reads require Viewer at current or retained stored node ancestry; writes
require Editor at current ancestry. Foreign/unknown share the existing denial.
The raw node_log table remains excluded from generic SQL. Page1..200 and response
byte caps apply. Per-object versions do not imply a multi-object wall-time snapshot.

The create route sends a typed NodeMaterialized GUI birth after commit, with a
fresh unknown correlation ID and its own reply family. CommittedNodeMaterialization
carries authoritative NodeDto quaternion/scale/asset_path, Euler rotation, asset_id,
properties and history_version. The GUI adapter must hydrate this current-state
materialization. NodeHistoryLoaded and NodeStateAtVersionLoaded remain read-only
previews; the UI must never apply them as live state. The separate API receipt
stays NodeMutationCommitted with its matching request ID. These shared typed DTOs
live in fe-runtime::node_history; their serialized REST shapes above are unchanged.

### Node-history request and reply identity boundaries

The API constructs NodeAccessGrant::Scoped from the validated claims.scope; scope
and actor never come from object/mutation JSON. The DB rechecks this grant at use,
fences current ancestry transactionally, and authorizes each historical fact by
its retained scope. Unknown/foreign historical intervals expose static cursor
markers only, or unavailable_scope without replay state. Current-node admission
alone never grants access to an earlier scope's object history.

Correlated success must match nonce, reply family, and requested identity. A
mutation receipt's node_id must equal its state.node_id and the requested node;
a create receipt must identify its requested petal and a valid new node id.
History page coverage and every entry identify the requested node; state previews
also match the requested version and snapshot node. Property wrappers return
success only after these checks, rather than manufacturing success from the
requested id when the DB replied for another object.


Dedicated Local object commands derive actor from the DB-thread identity and
require stored Editor permission. Local history checks stored Viewer permission
on the current scope and each past fact scope. Local schema writes/reads require
Manager/Viewer respectively. Schema GET/PUT/MCP now use grant-bound
GetFieldSchema/SetFieldSchema DB requests; API creates Scoped from validated
claims and preserves authenticated opaque-principal capability behavior. Replies
still validate exact correlation and field-definition identity.


Scoped desktop SELECT extraction: src/query_guard.rs re-exports the pure
fe_query::sql_guard implementation and retains its async transport/authorization
pipeline. Existing REST/MCP/export/share/elevated guard semantics, row/byte/rate
limits and DB timeouts are unchanged. Pure guard journeys live with the moved
module; API semaphore/transport testing remains here. The desktop's narrower
petal-only sources/function vocabulary is enforced only by the new DB handler.

Scoped-query security follow-up changes the shared guard: whole-word TABLE and
RECORD are now refused anywhere, including field/string uses, to close native
Table/Record casts (also nested literal object/array kinds) bypassing source
rewriting. This deliberate security delta follows the semantics-preserving
extraction above. Ordinary numeric less-than comparisons remain accepted; no
new token/Owner exemption or broader API authority is introduced.

## Terminal maintenance API ownership

The GUI uses `spawn_api_service`, retaining its stop/join owner. This controlled host wraps its reader in command-write mode: IoT appends take the existing correlated DB-command path even when direct reads are available. Dropping the stopped API runtime then cannot abandon a direct database mutation outside the serial DB barrier. Existing `spawn_api_thread` deployments preserve their previous reader/append contract. Dev-token publication has a separate stoppable owner; its join precedes token-file invalidation.

## durable-api-credential-lifecycle

REST middleware and legacy /ws initial admission share auth::authenticate. auth::require_current_credential is the async recheck for actions and delayed response completion. It checks strict current expiry, bounded JTI, retained revocation hints, and one bounded durable generation/exact-token snapshot through the configured shared reader. INFO FOR TABLE ensures a missing token table is not treated as an unknown credential. Missing/malformed metadata, decode/query error and the five-second deadline deny with a generic unavailable response; no credential text is logged or returned. A present token row must match the subject and have revoked=false. An absent token row remains valid only in its current generation, preserving protected dev bootstrap. The public JWT fields and signing keys are unchanged.

Configured-store startup validates durable authority before binding/listening or publishing dev credentials. Every subsequent protected request checks it again, so persisted revocations survive API process/cache restart. GUI and relay both retain the shared database reader when analytics hydration fails; only the partial analytics cache is disabled. A caller explicitly configuring no reader retains standalone signature/cache compatibility, which cannot provide reset/persisted-store semantics. Production does not downgrade to that mode on a database error.

Revocation broadcasts are hints, not a completeness claim. Lag/closure marks the feed unavailable; configured hosts recover only after a successful durable check and keep all known revoked JTIs. Every request continues querying durable authority. A DB-less standalone host cannot recover lost notifications and remains fail closed. Legacy WS rechecks idle sessions every 200ms, every incoming command, every broadcast, each protected response/snapshot egress and debounce flush; invalidation exits without flushing pending transforms. No unsigned synchronization path is introduced.

Canonical WS is a separate, unmounted capability protocol with no JWT verifier or ApiClaims admission. Its existing durable epoch/authorization-view/session-generation checks remain unchanged. The current mounted JWT surfaces are authenticated REST/MCP plus legacy /ws; public health/readiness and separately signed share redemption do not admit JWT mutation authority.

The existing token lifecycle and actual-upgrade WS journeys cover persisted revoke without hints, fresh API runtime restart, lagged/closed recovery, unavailable startup/read refusal, old Owner POST /verses denial after reset, fresh generation mint/dev bootstrap, idle/current/new WS denial and no credential disclosure. Test function count is unchanged; no tests were executed in the source author lane.

Credential completion checks take a fresh wall-clock expiry reading after all durable and cache awaits, immediately before success. The existing expiry journey polls a real signed credential admission until it blocks on a held authority read, resumes after its deadline, and requires denial. Clock errors remain unavailable; neither cached admission nor an earlier time reading substitutes for the final check.

Both revocation-hint reads apply the same JTI and standalone feed-availability predicate. A missing durable reader cannot recover notification loss between those awaits. The existing expiry journey also queues the real lagged revocation listener behind the admission's initial read, then lets its final read wait behind that writer; admission must refuse after the feed-loss marker commits. This deterministic ordering uses the pinned Tokio1.52.3 RwLock FIFO contract, without scheduler sleeps or a test-only admission hook. These assertions remain unexecuted until the campaign-end sweep.

## QA Portal evidence

Authenticated routes are GET `/api/v1/debug/portal-fixture/observe`, POST `/api/v1/debug/portal-fixture/action` with exactly a fixed link operation, and POST `/api/v1/debug/portal-fixture/capture`. All reuse the existing default-off input/Owner QA gate and budget. The dedicated bridge carries token scope and a host-owned five-second deadline. Completion rechecks input posture and Owner role, then calls the shared async `auth::require_current_credential` for durable revocation/reset generation and current expiry with no route-local cache fallback. Missing or unavailable configured auth metadata denies release. Receipt scope, deadline and exact live fixture permit are checked after that async credential read and before native bytes leave the route. Capture returns native image/png bytes plus bounded `x-fe-portal-fixture` metadata; generic diagnostic JSON never contains image base64. No general browser automation API is exposed.


## Canonical stamp reads

REST property and stored-transform reads accept the same addressable identifiers
as the generic node endpoint, including `<path ULID>#inst-<index>`. Validation is
syntax only: Viewer access and the owning node's stored ancestry still authorize
each request. Property reads share their direct/correlated core with MCP; both
read paths reject missing, ambiguous or tombstoned rows. Stored transforms describe
the node projection and are not advertised as the path-derived rendered pose.
Generic property writer validation and the stamp translation policy are unchanged.

### Scoped property and stored-transform reads

REST property and stored-transform reads retain the exact DB-resolved scope
that authorization accepted. MCP property reads pass the dispatcher's same
captured scope. One fixed SELECT binds the live node payload to that exact
Petal/Fractal/Verse chain and requires each identity to be globally unique in
the same statement. The guarded direct reader and strictly correlated RawQuery
fallback execute identical SQL. A missing, moved, duplicated or tombstoned row
is a denial; only a proven live row can return an empty property bag.

The separately named private legacy property helper serves only the internal
Petal-origin compatibility lookup and the existing typed-reply identity journey.
It is not reachable from REST/MCP node-property reads. Private GIS transform
loading remains a trusted internal seam; public transform GET uses the scoped
projection. Generic writer identity and stamp translation rules are unchanged.

## QA invitation transfer

GET/POST /api/v1/debug/invitation uses the captured default-off input gate, Owner floor,
existing rate budget and durable current-credential admission. GET returns actual host identity
and current public invitation metadata. POST accepts only that exact bounded metadata, over
a separate capacity-one bridge; no arbitrary credential/token/clipboard payload is accepted.
The host rechecks the actual pinned Owner with the existing DB source preparation handler
before one-use consumption. API completion repeats durable credential, deadline and scope
checks, then serializes the sealed credential only into a no-store response. It is never
placed in a generic diagnostic Value, Debug payload, trace field or error response.

The existing debug/input route accepts optional X-FE-Join-Lease as a constrained recipient UI
lease. It keeps the input opt-in/Owner/rate/scope checks, additionally repeats durable credential
checks around the guarded request, and delegates exact fixed-batch/frame/widget admission to
the native input host. No new direct Join, arbitrary text broker or authorization path exists.

## Scene-asset-publication-audit

GET /api/v1/debug/nodes/{node_id}/scene-asset-publication is metadata-only QA readback under the existing default-off FE_API_INPUT_ALLOW QA_GATE, Owner floor/rate budget and current host-context gate. Only an optional canonical uppercase ULID operation_id query is accepted; unknown/duplicate keys, extra arguments and malformed IDs refuse. There is no discovery enumerator, asset selector, arbitrary SQL, filesystem/provider path, MCP surface or table-permission expansion. The normal middleware's verified subject/scope/role ceiling is carried privately to DB, where both caller and actual local DID need current stored Owner over the complete same-Petal graph. Caller-supplied JSON cannot construct that authority.

API admission and host-to-DB forwarding each use bounded try_send. The host retains a single exact nonce/deadline/navigation owner and never blocks waiting in its frame. The dedicated typed DB result/failure cannot consume native review/adoption or unrelated API ownership. API return checks both bridge echo and DTO nonce/node/op shape, scope, whole response bound, deadline and current credential expiry/generation/revocation again. Responses are no-store and failures use fixed safe text, never SQL/cause/raw-row data. DB's final fenced transaction is the point-in-time stored authority/state linearization point; later credential checks do not promise continuous role ownership.

No operation_id requires native NONE owner/anchor/provenance and zero complete asset adoption ops; after adoption it refuses. A supplied operation_id requires exactly one persisted matching intention and stored provenance/metadata agreement; no fallback receipt is synthesized. The versioned DTO exposes only approved typed fields/digests. bytes_verified_by_this_read=false; actual downloads/BLAKE3/SHA256 remain separate. The node-bound endpoint cannot independently prove the pre-placement zero-reference instant. Standard credential admission retains its existing bounded durable reader; the audit itself goes through the already running DB owner and never opens another store.

## §extensions

`src/extensions.rs` holds the HTTP-facing rules of the extension surface (bus.md §4/§8, governance.md
§Local identity); the binary implements `ExtensionHost` over `fe_plugin::Bus` (fractalengine
`plugin_bridge.rs`). Every route runs `gate` first (a non-local caller's per-subject rate, never audited).

- **Local identity = node DID under an Owner token.** Every token this node mints carries the node DID as
  `sub`, so `is_local` also requires `max_role` Owner (the operator-gate rule); a delegated token is keyed
  `sub#jti` at the gate and reaches the bus with `ApiCaller.role_ceiling`, which caps its role and makes it
  remote there (fe-plugin `src/bus/AGENTS.md` §delegated-callers).
- **Ingress answers** are guest bytes on the API origin: a status outside 200-599 becomes 502, and every
  answer carries `x-content-type-options: nosniff` and `content-security-policy: sandbox`. Credential
  headers are stripped before the host sees the request (`is_credential_header`), a second layer under the
  net broker's own strip.
- **Governance (E10).** `PUT|GET /api/v1/verses/{verse_id}/extension-policy` is NOT local-only: any token
  whose scope covers the verse reads it, and a Manager+ token writes it (an Editor gets `role-denied` plus
  one `denial` audit row). The host then writes the row as its local DID, which must hold Manager+ too
  (`device_format` rule), so the effective authority is min(token, node). Host calls that touch the DB
  return an `AdminReply` receiver under `REPLY_TIMEOUT` (no blocking on a tokio worker); the trait's
  default answers `not-available` (503), so a binary that has not wired them fails closed.
  `POST /api/v1/extensions/kill-list-overrides` is `AdminVerb::KillListOverride` on the ordinary
  local-identity path; the Owner and confirmation checks live in fe-database
  (`handlers/AGENTS.md` §extension-policy). Precondition refusals map to governance.md's suggested statuses
  in `From<AdminRefusal>`; `not-local-identity` is the uniform not-found.
- **Mounting (I-API, R-API).** `ApiConfig.extension_host` / `binding_host` (`Option<Arc<dyn ..>>`; `None` in the
  relay and both harness tests) become `server::ApiHosts`. `build_router_with_hosts` ALWAYS merges
  `extensions::routes`, `bindings::routes` and `bound_routes::router` INSIDE the auth layer (`plugin_host_routes`);
  an absent host is replaced by `server::absent_hosts` (every lookup answers the uniform not-found, its
  policy reads too, where the trait default would say 503 `not-available`). So a relay answers an extension or
  binding path exactly as a GUI host answers an unknown id: 401 without a token, then the uniform 404 body,
  never axum's bare 404 (which told a caller the node has no plugin host). Body caps sit at the mount:
  `DefaultBodyLimit` of `MAX_INGRESS_BODY_BYTES` on the extension and alias routers (the admin JSON bodies;
  ingress itself reads the raw body through its own bounded `to_bytes`) and `MAX_BINDING_BODY_BYTES` on the
  binding routes. `build_router(state)` is the same with no hosts. `ApiHosts` is NOT in `ApiState` (many
  constructors) but an axum `Extension` over the whole router, which is how `/mcp` and `/ws` reach it; the
  stand-ins never go into `ApiHosts`, so MCP still hides the tools and `/ws` still answers not-found.
  `Method::PUT` joined the CORS list for `PUT /api/v1/bindings`.
- **MCP.** `list_extensions`, `extension_command` (PetalArg: the dispatcher's petal scope is the
  `petal_authorized` input), `list_bindings`, `set_binding`, `clear_binding` carry `ToolSeam::Extensions` /
  `Bindings`: `tool_available(spec, state, hosts)` hides them (and answers "unknown tool") when the host is
  absent. The binding tools use `ScopeRule::BindingTarget`: the DISPATCHER parses `verse_id` (+ `fractal_id`,
  `petal_id`, each a ULID) and resolves it with the REST routes' `bindings::authorized_target_scope` (exact
  level, claimed chain, containment), so authz stays in `authorize` like every other row and the grep test bans
  `authorized_target_scope` / `authorized_petal_scope` from `tools.rs`. `set_binding`/`clear_binding` floor at
  Manager in the dispatcher and again in the core. `mcp_integration.rs`'s role-floor and foreign-scope journeys
  wire a recording plugin host and require that no denied call reaches it.
- **WS.** `bus_subscribe {petal_id, source, params?}` -> `bus_subscribed {sub, source}`; `bus_unsubscribe {sub}`;
  events arrive as `{"type":"bus_event", ...envelope}` (bus.md WS row). The `ExtensionStreams` client opens on
  the first subscribe and closes (host client + subscriptions) when the socket ends. EVERY 404 from subscribe
  (missing host, unauthorized or unknown petal, `host/` source, unknown package, a host's `not-available`
  detail) is rewritten to `not_found` + `NOT_FOUND_OR_DENIED`, so the text never tells unknown from foreign.

## §bindings

`src/bindings.rs` is the REST/MCP face of the scoped binding plane (docs/extensions/bindings.md §9); the binary
implements `BindingHost` (writes through `DbCommand::PutBinding`, reads through `ListBindings` and its
resolution cache). Role rules: reads Viewer+, writes Manager+ at the token ceiling, the DB plane re-judges the
DID. Only the local DID (Owner token + node DID) sees `resolution` in `bindings/effective`. `bound_routes.rs`
serves `/api/v1/petals/{petal}/apis/{key}/{endpoint}` through the winning `api.routes` binding: unresolved is a
503 without an offer, no binding is the uniform not-found. Kept order: the MCP dispatcher resolves a binding
tool's scope before the core's remote-rate `gate` (one DB read per call, bounded by `/mcp`'s concurrency limit;
the answer is the uniform not-found either way, so the order reveals nothing).

## §scene-gate-doors

Every API door that creates a node, moves one or sets a property asks the `scene.*` gate
(`fe_runtime::scene_hook_gate`, bindings.md §6) through ONE function, `rest::scene_gate_verdict`, after its own
authz and before its write (R-API door parity, steer 2026-11-09 02:45):

| Door | Hook | Caller kind | Refusal |
|---|---|---|---|
| REST `POST .../nodes`, `POST /api/v1/nodes`, `PATCH .../transform`, `PATCH .../properties` | create / transform / property | `Rest` | 409 `refused` |
| MCP `create_node`, `place_asset` | create | `Mcp` | tool error "Refused by <package>: <reason>" |
| MCP `update_transform`, `set_property`, `commit_node_mutation` (`full_transform` / `property_set`) | transform / property | `Mcp` | same |
| `/ws` `entity_command` `create_node` / `set_node_property` | create / property | `Rest` | `entity_command_result.error` |
| `/ws` `transform_update` | transform | `Rest` | `error {code: "refused", message: "node <id>: ..."}` |

`mcp/scene_gate.rs` holds the MCP/WS wrapper: any verdict other than `Proceed` keeps the write unsent (so a new
refusal kind, such as R-HOOKS' over-cap `input-too-large`, fails closed here without an edit), and an allowed
augment stages only after its write was sent (`stage_created` with the new node id for a create). A transform is
gated BEFORE the optimistic broadcast, so a refused move is never previewed. `/ws` gates each `transform_update`
(REST parity, not the GUI's commit-only rule) and stages that node's latest augment after the debounced
`TransformPersist` is sent; a superseded proposal's augment is dropped with it. `/ws` reports caller kind `Rest`
because the WIT `caller-kind` has no `ws` (`ws.rs` `WS_SCENE_CALLER`, one line to change if the contract adds
one). Doors still outside the gate (HANDED-OFF in review-turn2/R-API.md): REST
`POST /nodes/{id}/mutations` and `POST /petals/{p}/objects` (`node_history.rs`), waypoint create/move, GPX import
and `promote_instance` on both REST and MCP; one shared-core gate inside those cores is the fix.
