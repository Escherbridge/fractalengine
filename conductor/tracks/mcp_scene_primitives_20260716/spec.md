---
type: Track Spec
title: MCP Scene-Construction Primitives
description: >
  Close the asset-ingestion gap and expand the /mcp endpoint from 6 tools to a
  complete 20-tool primitive vocabulary so an external AI client can build
  scenes (model homes, properties, city layouts) headlessly over HTTP.
tags: [feature, mcp_scene_primitives_20260716, pending]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Track Spec: MCP Scene-Construction Primitives

## Overview

FractalEngine exposes an MCP endpoint (`POST /mcp`, JSON-RPC 2.0, protocol
`2025-03-26`) with 6 tools. An external AI client can create hierarchy and
empty nodes — but **cannot get a 3D asset into the system, cannot attach an
asset to a node, and cannot delete a node**. Everything an AI would need to
build a real scene headlessly is blocked on asset ingestion, and most of the
existing REST surface (properties, waypoints, GPX, terrain, tilesets, GIS,
query) has no MCP wrapper.

This track delivers the **primitive vocabulary**: asset upload, asset-bearing
node placement, node deletion, and MCP wrappers over the existing REST
handlers — all routed through **one shared authz/dispatch helper** that also
fixes the documented weaker-scope-check fallbacks in the current handler.

**Binding directive — primitives only.** The AI composes scenes from
low-level tools. No semantic verbs (`place_building`, `create_road`,
footprint/occupancy queries) — see Out of Scope.

## Background / Evidence (verified 2026-07-16)

- **The blocker:** MCP/REST `create_node` always yields asset-less nodes
  (`asset_id: NONE`). The only GLB-ingest op, `DbCommand::ImportGltf`
  (`fe-runtime/src/messages.rs`), takes a **local file path** and has no HTTP
  surface (UI dialog only). External clients cannot ingest or attach a GLB.
- **No node deletion over HTTP:** `DbCommand::DeleteNode` exists
  (`fe-database/src/handlers/crud.rs:220`, cascades to `gpx_track_id`
  waypoint children) but the only external path is Manager+ elevated SQL.
- **Existing REST already covers** property set/get/delete, waypoint
  create/move, GPX import/export, petal terrain get/put/delete, hexon tileset
  install/list, GIS reads, scope-guarded query, hierarchy — none wrapped as
  MCP tools (`fe-api/src/server.rs` route table).
- **Auth wart:** `create_node` in `fe-api/src/mcp.rs` (~359–365) and
  `create_petal` (~315–322) fall back to a **role-only** check when hierarchy
  IDs are omitted. Expanding to 20 tools multiplies this pattern unless a
  single dispatch/authz helper is enforced. `update_transform` (mcp.rs ~397)
  is role-only with **no scope check at all**.
- **Blob store seam:** `ApiState.blob_store: Option<BlobStoreHandle>`
  (`Arc<dyn BlobStore>`, sync methods) already exists on the API thread, and
  `fractalengine/src/main.rs:40` clones **one** handle into both the DB
  thread and `ApiState` — so the API side can write blobs directly and hand
  only hash/metadata to the DB thread.
- **Channel constraint:** all inter-thread traffic rides
  `crossbeam::channel::bounded(256)`; multi-MB payloads through it are
  unacceptable.
- **Transport:** hand-rolled request/response JSON-RPC, no SSE — acceptable,
  keep as-is.

## Functional Requirements

### FR-1: Asset upload over REST (Must)

`POST /api/v1/petals/{petal_id}/assets` — multipart upload of a GLB.

- Auth: Editor+ at the petal's resolved scope (`petal_id` is the authz
  anchor; asset rows remain node-global, content-addressed).
- Flow: validate (FR-7) → `blob_store.add_blob` **on the API side** (wrapped
  in `spawn_blocking`; the fs+hash work is synchronous) → send new
  `DbCommand::CreateAsset { name, content_type, size_bytes, content_hash }`
  (small metadata only) → `DbResult::AssetCreated { asset_id, content_hash }`.
- Response: `201 { asset_id, content_hash, size_bytes }`.
- `503` when `ApiState.blob_store` is `None` (unconfigured deploys).
- Re-uploading identical bytes is idempotent at the blob layer (same BLAKE3
  hash) but creates a new asset row; dedupe-by-hash upsert is out of scope
  (documented).

**Acceptance criteria**
- Valid GLB → 201 with non-empty `asset_id` and 64-hex `content_hash`; blob
  retrievable via existing `GET /api/v1/assets/{content_hash}`.
- Viewer token → 403. Out-of-scope Editor token → 403.
- No `DbCommand` variant carries the asset bytes.

### FR-2: MCP tool `upload_asset` (Must)

Same semantics as FR-1 with base64 transport: arguments
`{ petal_id, name, data_base64 }` → `{ asset_id, content_hash, size_bytes }`.

- Decoded bytes go through the identical validate → blob → `CreateAsset`
  core as FR-1 (one shared core function, two transports).
- Enforce the 256 MB limit on **decoded** length; `/mcp` route body limit
  raised accordingly (see NFR-2).

**Acceptance criteria**
- `tools/call upload_asset` with a valid base64 GLB succeeds headlessly.
- Invalid base64, non-GLB bytes, oversize payload → `isError: true` with a
  actionable message; nothing written to blob store or DB.

### FR-3: Asset-bearing node placement (Must)

New `DbCommand::CreateNodeWithAsset { petal_id, name, asset_id, position,
rotation, scale, correlation_id }` + handler; surfaced as:

- MCP tool `place_asset` — `asset_id` **required**; `position`, `rotation`
  (Euler XYZ radians, matching `update_transform`'s convention), `scale`
  optional with identity defaults.
- Existing `create_node` (MCP tool + REST `POST .../nodes` and legacy
  `POST /api/v1/nodes`) gains **optional** `asset_id` / `rotation` / `scale`
  body fields — non-breaking; when `asset_id` present, route to
  `CreateNodeWithAsset`, otherwise the legacy `CreateNode` path unchanged.
- Handler validates the asset row exists (fail with a clear error if not),
  mirrors `import_gltf_handler`'s node CREATE (`asset_id`, `asset_path =
  blob://<hash>.<ext>`, geometry-point position per AGENTS.md
  §geometry-inserts), converts Euler→quaternion consistently with
  `UpdateNodeTransform`, and appends `node_log`.
- Result reuses `DbResult::GltfImported` (exact field match: `node_id`,
  `asset_id`, `petal_id`, `name`, `asset_path`, `position`) — **no new
  DbResult variant**, so no exhaustive-match fallout and the running GUI's
  existing `GltfImported` listener spawns the placed asset live in-viewport.

**Acceptance criteria**
- `upload_asset` → `place_asset` yields a node whose hierarchy entry has
  `has_asset: true` and a `blob://` asset path; `GET /nodes/{id}/asset`
  serves the bytes.
- `place_asset` with unknown `asset_id` → `isError: true`, no node row.
- Auth: Editor+ at petal scope (args or DB-resolved, per FR-5).

### FR-4: Node deletion (Must)

- REST: `DELETE /api/v1/nodes/{node_id}` — new route over existing
  `DbCommand::DeleteNode`.
- MCP tool: `delete_node { node_id }`.
- Auth: Editor+ at the node's DB-resolved scope (`resolve_node_scope`).
- **Cascade semantics (documented in tool description + AGENTS.md):**
  deleting a node atomically deletes its waypoint children
  (`properties.gpx_track_id == node_id`); the DB thread emits
  `SceneChange::NodeRemoved` alongside `DbResult::NodeDeleted` so WS
  subscribers and the live GUI converge.
- Missing node ("matched no node" from the handler) → REST 404 / MCP
  `isError: true`.

### FR-5: One shared authz/dispatch helper (Must)

All MCP tools dispatch through a single table-driven helper:

- `ToolSpec { name, description, input_schema, min_role, scope_rule,
  handler }` with `ScopeRule ∈ { None (self-filtering reads), Global,
  PetalArg, NodeArg, HierarchyArgs }`.
- The dispatcher resolves the resource scope (via
  `ResolvePetalScope`/`ResolveNodeScope` DB round-trips when the rule is
  `PetalArg`/`NodeArg`, or when `HierarchyArgs` are incomplete) **before**
  invoking the handler, then enforces `require_role_and_scope`.
- **Fixes the warts:** `create_node` and `create_petal` fallbacks resolve
  scope from the DB instead of degrading to role-only; unresolvable scope →
  deny. `update_transform` gains a node-scope check. **No role-only write
  path may remain.**
- Existing 6 tools migrate onto the table; behavior otherwise unchanged
  (5 s timeouts, `tool_result`/`tool_error` shapes, safe error messages
  without internal detail).

**Acceptance criteria**
- Unit tests prove: role ladder enforced per tool; out-of-scope token denied
  per tool; `create_node` without `verse_id`/`fractal_id` is **scope-checked
  via DB resolution** (regression test for the old fallback); grep-level
  invariant — no `require_role(` call sites remain inside individual MCP tool
  bodies (only inside the dispatcher).

### FR-6: MCP wrappers over existing handlers (Must; tileset/terrain Should)

New tools, each a thin wrapper sharing a transport-agnostic core with its
REST handler (extract the core where the REST handler carries non-trivial
logic — waypoints, GPX parse loop, query guard):

| Tool | Role | Scope rule | Wraps |
|---|---|---|---|
| `set_property` | editor | NodeArg | `SetNodeProperty` |
| `get_properties` | viewer | NodeArg | `GetNodeProperties` |
| `delete_property` | editor | NodeArg | `DeleteNodeProperty` |
| `create_waypoint` | editor | PetalArg | `rest::create_waypoint` core |
| `move_waypoint` | editor | NodeArg | `rest::move_waypoint` core |
| `import_gpx` | editor | PetalArg | `gpx::import_gpx` core, `data_base64` body |
| `set_petal_terrain` | editor | PetalArg | `SetPetalTerrain` (null clears) |
| `list_tilesets` | viewer | None | `terrain::list_available_tilesets` |
| `install_tileset` | match REST handler (verify; expected Manager+) | None/Global | `terrain::install_hexon_tileset` |
| `get_gis_nodes` | viewer | PetalArg | `gis::list_gis_nodes` core |
| `query` | viewer | token-scope via existing `query_guard` pipeline | `rest::execute_query` core (SELECT/RETURN only) |

Plus FR-2/3/4 tools (`upload_asset`, `place_asset`, `delete_node`) and the
existing 6 — **20 tools total** in `tools/list`.

**Acceptance criteria**
- `tools/list` returns exactly the 20-tool inventory; every `inputSchema` is
  a valid JSON Schema object; every description states its role requirement.
- Each wrapper has at least one success test and one RBAC-negative test.

### FR-7: GLB validation + size limit (Must)

- Pure helper `validate_glb(bytes, max_len) -> Result<(), UploadError>`:
  length ≥ 12, magic `glTF` (0x46546C67 LE), version == 2, GLB header total-
  length field consistent with actual byte length, `bytes.len() ≤ max_len`.
- `MAX_ASSET_BYTES = 256 MiB` per tech-stack.md ("configurable per Node" —
  keep a named constant now; per-node config is a documented seam, not built).
- GLB **only** — reject JSON `.gltf` (tech stack: textures must be embedded
  in the GLB binary to avoid Bevy hot-reload bug #18267). Embedded-texture
  presence is **documented as an uploader requirement**, not parsed/enforced.
- Both transports (multipart, base64-decoded) share this helper.

**Acceptance criteria**
- Unit tests: valid header passes; bad magic / version 1 / truncated header /
  header-length mismatch / oversize (via small injected `max_len`) all fail
  with distinct errors. Oversize enforced without allocating a 256 MB fixture.

### FR-8: Headless full-loop acceptance test (Must)

An integration test (`fe-api/tests/mcp_scene_test.rs`) drives the complete
loop **over HTTP via MCP `tools/call` only**, against the real router with
real auth middleware and a real minted token:

1. `upload_asset` (small valid GLB fixture, base64)
2. `place_asset` at a position
3. `set_property`
4. `update_transform` (move)
5. `get_hierarchy` + `get_gis_nodes` (observe the node with asset)
6. `delete_node` → hierarchy no longer contains it

RBAC negatives in the same harness: Viewer token cannot `upload_asset` /
`place_asset` / `delete_node`; an Editor token scoped to a **different
verse** is rejected on every scoped tool.

Harness: in-memory SurrealDB + `fe_database::schema::apply_all` +
`fe_database::spawn_db_thread_with_sync` + a small `ApiCommand → DbCommand`
forwarder thread (stands in for the Bevy bridge) + tempdir-backed blob store
+ `mint_api_token` from `fe-identity` against the state's verifying key.

## Non-Functional Requirements

### NFR-1: Channel discipline (Must)
Asset bytes never traverse the crossbeam channel. Only hash/metadata-sized
`DbCommand`s are introduced. Blob I/O and BLAKE3 hashing run under
`spawn_blocking` on the API runtime (never on the Bevy thread; never
`block_on` in handlers).

### NFR-2: Body limits (Must)
- Asset REST route: per-route `DefaultBodyLimit::max(MAX_ASSET_BYTES + 1 MiB)`
  (precedent: `fe-hexon-registry/src/routes.rs:80`).
- `/mcp` route: raised to `ceil(4/3 × MAX_ASSET_BYTES) + 8 MiB` to admit a
  max-size base64 upload; decoded-size check remains authoritative.
- All other routes keep the axum default (2 MB).
- Documented implication: a max-size MCP upload transiently holds ~600 MB
  (body + decode); acceptable for a desktop node, streaming ingest is a
  non-goal.

### NFR-3: Security (Must)
- Deny-by-default: every tool passes through the FR-5 dispatcher; no
  role-only writes. Scope resolution failures deny, never degrade.
- Error messages to clients stay generic ("operation failed"); details go to
  `tracing` (existing pattern).
- Security-relevant denials logged via `tracing` per workflow.md quality
  gates.

### NFR-4: Compatibility (Must)
- MCP protocol version, request/response transport, `tool_result` /
  `tool_error` content shapes unchanged. Existing 6 tools keep their names
  and argument schemas (only *stricter* authz + additive optional args).
- REST changes are additive (new routes; optional body fields).
- `fe-ui` is untouched (parallel-track constraint). `DbCommand::CreateNode`
  and `DbCommand::ImportGltf` are untouched.

### NFR-5: Performance (Should)
- Non-upload tool calls preserve the existing 5 s reply timeout.
- Scope resolution adds at most one DB round-trip per call (matching current
  REST handler behavior).

## User Stories

- **As an external AI client** (Claude/other MCP client) with an
  Editor-scoped token, **I want** to upload GLBs and compose them into a
  petal with positions, rotations, properties, and waypoints, **so that** I
  can build a model home or city layout headlessly.
  - Given a valid token and GLB, when I call `upload_asset` then
    `place_asset`, then the node appears in `get_hierarchy` with
    `has_asset: true` at my position.
- **As a node operator**, **I want** every MCP write gated by role AND scope,
  **so that** a leaked Viewer or off-scope token cannot mutate my worlds.
  - Given a Viewer token, when it calls any write tool, then the call returns
    an error and nothing changes.
- **As an operator running headless**, **I want** a documented token flow,
  **so that** I can hand a scoped credential to an external client (see
  Operator Token Flow).

## Technical Considerations (key design decisions)

- **D1 — API-side blob write.** Upload handlers write bytes via
  `ApiState.blob_store` and send only `CreateAsset` metadata to the DB
  thread. Verified: `main.rs:40` shares one `BlobStoreHandle` between
  `ApiState` (lines 168/244) and `spawn_db_thread_with_sync` (80/109), so
  hashes written API-side are immediately servable and DB-referencable.
  Plan includes an evidence task asserting this single-handle wiring stays
  true.
- **D2 — New DbCommand variants, not `CreateNode` extension.**
  `DbCommand::CreateNode` has ~15 construction sites including
  `fe-ui/src/dialogs/create_entity.rs` and `fractalengine/src/gpx_bridge.rs`;
  extending it violates the no-fe-ui constraint. `CreateAsset` +
  `CreateNodeWithAsset` are additive; `CreateNodeWithAsset` reuses
  `DbResult::GltfImported` to avoid DbResult exhaustive-match fallout
  (known fe-test-harness gotcha) and to light up the GUI's existing listener.
- **D3 — Table-driven dispatch** (FR-5) is the multiplication-proof against
  the fallback wart; it is a refactor of `mcp.rs`, not a new transport.
- **D4 — Shared cores, two transports.** Upload/GPX/waypoint/query logic is
  extracted once and called from both REST handlers and MCP tools; REST
  handlers keep their existing routes and behavior.
- **D5 — Scale seam (binding directive #2).** `position`/`rotation`/`scale`
  pass through **verbatim in petal-local world units**. This track invents no
  per-asset scale metadata and does no unit conversion. The
  `CreateNode`/`CreateNodeWithAsset` position write path is the seam
  `map_scale_authority_20260716` will touch (world_scale-derived placement);
  coupling is limited to that pass-through.
- **D6 — Crate boundary.** Changes land in `fe-api`, `fe-runtime`
  (messages), `fe-database` (handlers/dispatch), plus `fractalengine`
  wiring only if the blob-handle evidence task finds divergence. **No fe-ui
  edits** (parallel track `road_builder_ux_20260716` owns fe-ui).
- **Repo conventions:** terse one-line doc comments; the "why" (dispatch
  table rationale, blob seam, limits, cascade semantics) goes in
  `fe-api/AGENTS.md`; TDD-ordered tasks; **single full workspace sweep at
  the very end only** (user's standing test-execution policy).

## Operator Token Flow (documented, not built)

Headless token minting is a non-goal. The supported flow:

1. Operator launches the FractalEngine GUI on the host node.
2. In-app API-token UI mints a token via `fe-identity`'s `mint_api_token`
   (`DbCommand::MintApiToken`, Manager+ at scope enforced server-side;
   30-day ApiClaims: `sub`, `scope`, `max_role`, `jti`).
3. Operator copies the JWT into the external MCP client's config as
   `Authorization: Bearer <token>` for `POST /mcp`.
4. Revocation: in-app revoke by JTI (revocation cache honored by
   `auth_middleware`).

For scene construction, mint **Editor** at the target verse scope (e.g.
`VERSE#v1`). Manager is required only for `create_verse` and (expected)
`install_tileset`.

## Out of Scope (explicit non-goals)

- **City-semantic tools** — no `place_building`, `create_road`,
  footprint/occupancy queries. Primitives only; do not sneak semantic verbs
  in (binding directive #1).
- SSE / streamable-HTTP MCP transport (request/response stays).
- OpenAPI generation.
- Headless/API-driven token minting (operator flow documented instead).
- fe-plugin integration.
- Per-asset scale metadata or unit conversion (owned by
  `map_scale_authority_20260716`).
- Streaming/chunked asset upload; asset dedupe-by-hash upsert; asset
  deletion/GC.
- Any fe-ui change.
- `export_gpx`, petal `.hexon` export/import, elevation-profile/stats,
  field-defs, IoT ingest, analytics, crates/publish as MCP tools (REST-only
  remains fine for v1 of this vocabulary; revisit on demand).

## Open Questions

1. `install_tileset` minimum role: match whatever
   `terrain::install_hexon_tileset` enforces today (verify during Phase 4;
   expected Manager+). Default: mirror REST exactly.
2. Should `upload_asset` MCP tool cap decoded size below 256 MB for
   practicality (JSON-RPC memory)? Default: no — one limit everywhere,
   documented memory implication (NFR-2).
3. `set_petal_terrain` clearing convention: `terrain: null` clears (mirrors
   REST DELETE). Default: yes, single tool with nullable arg.
