---
type: Implementation Plan
title: "Implementation Plan: MCP Scene-Construction Primitives"
tags: [mcp_scene_primitives_20260716]
resource: ./spec.md
---

# Implementation Plan: MCP Scene-Construction Primitives

## Overview

Five phases, bottom-up along the thread topology: DB-thread primitives first
(fe-runtime messages + fe-database handlers), then the REST ingest/delete
surface, then the MCP authz/dispatch refactor, then the 14-tool expansion,
finally the headless full-loop acceptance test + docs + the single workspace
sweep.

**Crate boundary (binding):** `fe-api`, `fe-runtime`, `fe-database` (+
`fractalengine/src/main.rs` only if task 2.5 finds blob-handle divergence).
**Never fe-ui.**

**Test policy (user standing rule):** each task runs only its targeted tests
during TDD (`cargo test -p <crate> <filter>`); the FULL workspace sweep
(`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`) runs
**once**, at the very end (task 5.4). Do not iterate full sweeps per task.

**Docs convention:** terse one-line `///` on new items; the "why" goes in
`fe-api/AGENTS.md` (new §mcp-dispatch, §asset-ingest) and the fe-database
handlers' AGENTS section (cascade + CreateNodeWithAsset notes).

---

## Phase 1: DB-thread primitives (fe-runtime + fe-database)

Goal: `CreateAsset` and `CreateNodeWithAsset` commands exist, handled on the
DB thread, carrying only metadata — asset bytes never enter the channel.

Tasks:
- [ ] Task 1.1: `DbCommand::CreateAsset` + `DbResult::AssetCreated` +
      `create_asset_handler` (TDD: fe-database integration test via
      `spawn_db_thread_with_sync` — send `CreateAsset { name, content_type,
      size_bytes, content_hash }`, assert `AssetCreated { asset_id,
      content_hash }` and the asset row (`data: NONE`, hash set) via direct
      DB read; then implement handler as the asset-row half of
      `import_gltf_handler` (crud.rs ~307–332) with **no file I/O**; refactor)
- [ ] Task 1.2: `DbCommand::CreateNodeWithAsset` + handler (TDD: test —
      seeded asset row + `CreateNodeWithAsset { petal_id, name, asset_id,
      position, rotation, scale, correlation_id }` yields
      `DbResult::GltfImported` (reused — spec D2, no new DbResult variant)
      and a node row with `asset_id`, `asset_path = blob://<hash>.glb`,
      geometry-point position, Euler→quat rotation matching
      `UpdateNodeTransform`'s conversion, scale, `node_log` entry; negative
      test — unknown `asset_id` → `DbResult::Error`, no node row; implement;
      refactor)
- [ ] Task 1.3: Exhaustive-match fallout sweep for the new `DbCommand`
      variants — fe-database `lib.rs` dispatch arm, `fe-test-harness`
      `peer.rs` command loop (known gotcha), any other `match` sites found by
      `cargo check` across the workspace (compile-driven; **no fe-ui edits
      should be needed** — if one appears, stop and re-scope)
- [ ] Verification: targeted `cargo test -p fe-database` green; grep confirms
      no new `DbCommand` variant carries a byte buffer [checkpoint marker]

---

## Phase 2: REST asset ingest + node delete (fe-api)

Goal: an external HTTP client can upload a GLB and delete a node; create
endpoints accept optional `asset_id`/`rotation`/`scale`.

Tasks:
- [ ] Task 2.1: `validate_glb(bytes, max_len)` pure helper +
      `MAX_ASSET_BYTES = 256 MiB` constant in a new `fe-api/src/upload.rs`
      (TDD: unit tests — valid 12-byte header passes; bad magic; version 1;
      truncated; GLB header total-length ≠ actual length; oversize via small
      injected `max_len` — no 256 MB fixtures; implement; refactor)
- [ ] Task 2.2: `POST /api/v1/petals/{petal_id}/assets` multipart handler
      (TDD: handler-level tests in the gis_test.rs in-memory-ApiState idiom —
      Editor+resolved-petal-scope required (403 for Viewer / out-of-scope),
      503 when `blob_store` is `None`, happy path returns 201 `{ asset_id,
      content_hash, size_bytes }`; implement: read multipart `file` field →
      `validate_glb` → `spawn_blocking(add_blob)` → `DbCommand::CreateAsset`;
      wire route with per-route `DefaultBodyLimit::max(MAX_ASSET_BYTES +
      1 MiB)` per fe-hexon-registry precedent; refactor)
- [ ] Task 2.3: `DELETE /api/v1/nodes/{node_id}` (TDD: tests — Editor +
      `resolve_node_scope` enforced, 404 on missing node (map the handler's
      "matched no node" error), 200 `{ node_id, petal_id }` on success;
      implement over existing `DbCommand::DeleteNode`; document cascade
      semantics in the route doc comment pointer + AGENTS.md)
- [ ] Task 2.4: Optional `asset_id`/`rotation`/`scale` on `create_node` +
      `create_node_legacy` request bodies (TDD: tests — omitted fields ⇒
      byte-identical legacy behavior; `asset_id` present ⇒ routed to
      `CreateNodeWithAsset`, response includes `asset_path`; implement;
      refactor). Position/rotation/scale pass through **verbatim** — no unit
      math (spec D5 seam for map_scale_authority_20260716)
- [ ] Task 2.5: Evidence task — assert `fractalengine/src/main.rs` still
      wires the **same** `BlobStoreHandle` into `ApiState` and
      `spawn_db_thread_with_sync` (main.rs:40/80/168/244 as of survey); add a
      one-line note in `fe-api/AGENTS.md` §asset-ingest; only touch main.rs
      if divergence is found
- [ ] Verification: targeted `cargo test -p fe-api` green; manual curl
      transcript (upload → fetch blob by hash → delete node) recorded in the
      task note [checkpoint marker]

---

## Phase 3: MCP shared authz/dispatch helper (fe-api/src/mcp.rs refactor)

Goal: one table-driven dispatch path; the role-only fallbacks are dead.

Tasks:
- [ ] Task 3.1: `ToolSpec` table + dispatcher (TDD: unit tests for the
      dispatcher in isolation — `ScopeRule::{None, Global, PetalArg, NodeArg,
      HierarchyArgs}` each enforce `require_role_and_scope` with DB-resolved
      scope where applicable; unresolvable scope ⇒ deny; unknown tool ⇒
      `tool_error`; implement dispatcher + `tools/list` generated from the
      table; refactor). Keep `tool_result`/`tool_error` shapes and 5 s
      timeouts byte-compatible
- [ ] Task 3.2: Migrate the existing 6 tools onto the table (TDD: regression
      tests FIRST — (a) `create_node` **without** `verse_id`/`fractal_id` is
      scope-checked via `ResolvePetalScope` and denied for an out-of-scope
      Editor token (kills mcp.rs ~359–365 wart); (b) same for `create_petal`
      (~315–322); (c) `update_transform` denied for out-of-scope Editor
      (currently role-only, ~397); then migrate; assert existing happy paths
      unchanged)
- [ ] Task 3.3: Invariant guard — test (or grep-in-test) asserting no
      `require_role(`/`require_role_and_scope(` call sites remain inside
      individual tool handler bodies; authz lives only in the dispatcher
- [ ] Verification: targeted `cargo test -p fe-api mcp` green; tool count
      still 6, schemas unchanged [checkpoint marker]

---

## Phase 4: Tool vocabulary expansion (6 → 20 tools)

Goal: the complete primitive vocabulary from spec FR-2/3/4/6, every tool on
the dispatch table, each with success + RBAC-negative tests.

Tasks:
- [ ] Task 4.1: `upload_asset` (base64) + `place_asset` + `delete_node`
      tools (TDD: tests — upload/place/delete happy path against in-memory
      harness; invalid base64 / non-GLB / unknown asset_id / missing node ⇒
      `isError`; Viewer ⇒ denied; implement — `upload_asset` shares the
      Task 2.2 core, `place_asset` sends `CreateNodeWithAsset`, `delete_node`
      shares the Task 2.3 core; raise `/mcp` route body limit to
      `ceil(4/3 × MAX_ASSET_BYTES) + 8 MiB` per spec NFR-2; refactor)
- [ ] Task 4.2: Property + query tools — `set_property`, `get_properties`,
      `delete_property`, `query` (TDD: red tests incl. RBAC negatives and
      SELECT-only enforcement for `query` via the existing `query_guard`
      pipeline; implement as thin wrappers over the same DbCommands /
      shared cores the REST handlers use; refactor)
- [ ] Task 4.3: Waypoint + GPX tools — `create_waypoint`, `move_waypoint`,
      `import_gpx` (TDD: extract transport-agnostic cores from
      `rest::create_waypoint` / `rest::move_waypoint` / `gpx::import_gpx`
      (multipart stays for REST; MCP takes `data_base64`); red tests: valid
      minimal GPX fixture creates track+waypoints, malformed GPX ⇒ `isError`,
      RBAC negatives; implement; refactor)
- [ ] Task 4.4: Terrain + tileset + GIS tools — `set_petal_terrain`
      (`terrain: null` clears), `list_tilesets`, `install_tileset` (verify
      and mirror the REST handler's role — spec open question 1),
      `get_gis_nodes` (TDD; implement; refactor)
- [ ] Task 4.5: `tools/list` inventory test — exactly 20 tools, every
      `inputSchema` a valid JSON Schema object, every description names its
      role requirement; **primitives-only guard**: assert no tool name
      matches the banned semantic-verb list (`place_building`, `create_road`,
      `*footprint*`, `*occupancy*`) as a tripwire for directive #1
- [ ] Verification: targeted `cargo test -p fe-api` green; 20 tools listed
      [checkpoint marker]

---

## Phase 5: Headless full-loop acceptance + docs + single sweep

Goal: prove the track's reason for existing — an external client builds and
tears down a scene over `/mcp` alone — then document and sweep once.

Tasks:
- [ ] Task 5.1: Integration harness in `fe-api/tests/mcp_scene_test.rs`
      (TDD-lite: harness assertions first — in-memory SurrealDB +
      `schema::apply_all`, `fe_database::spawn_db_thread_with_sync`, an
      `ApiCommand::DbRequest → DbCommand` forwarder thread standing in for
      the Bevy bridge, tempdir-backed blob store handle shared by ApiState
      and the DB thread, real router from `fe_api::server` behind
      `auth_middleware`, tokens minted with `fe_identity::mint_api_token`
      against the state's verifying key — mirroring
      `fe-test-harness/tests/integration.rs`)
- [ ] Task 5.2: Full-loop test (spec FR-8): `upload_asset` (small embedded
      GLB fixture, base64) → `place_asset` at position → `set_property` →
      `update_transform` → `get_hierarchy` + `get_gis_nodes` show the
      asset-bearing node at the moved position → `delete_node` →
      `get_hierarchy` no longer contains it — **every step via MCP
      `tools/call` over HTTP**
- [ ] Task 5.3: RBAC negative suite in the same harness — Viewer token
      denied on `upload_asset`/`place_asset`/`set_property`/`delete_node`;
      Editor token scoped to a different verse denied on every scoped tool;
      revoked-JTI token gets 401 at the middleware
- [ ] Task 5.4: Docs — `fe-api/AGENTS.md`: §mcp-dispatch (table + ScopeRule
      rationale, fallback-wart history), §asset-ingest (blob seam D1, limits
      NFR-2, cascade semantics, operator token flow pointer to spec);
      one-line pointers from code; fe-database handler notes for
      `CreateAsset`/`CreateNodeWithAsset` (GltfImported reuse rationale)
- [ ] Task 5.5: **Single full workspace sweep** (once, per repo policy):
      `cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test`
      across the workspace; fix fallout; re-run once
- [ ] Verification: full-loop test green in the sweep; spec acceptance
      criteria walked FR-by-FR with evidence links; confirm zero fe-ui diffs
      (`git diff --stat -- fe-ui/` empty) [checkpoint marker]

---

## Notes

- Parallel-track coordination: `road_builder_ux_20260716` owns fe-ui;
  `map_scale_authority_20260716` owns world_scale placement math. The only
  shared seam is the create/place position pass-through (spec D5) — keep it
  verbatim and coupling-free. If both tracks land in the same window, the
  Phase 1 messages.rs diff is the likeliest merge point; it is additive-only.
- `DbCommand::ImportGltf` (local-path UI import) is intentionally untouched.
- If SurrealDB-core recompiles poison rmeta during the sweep (phantom
  E0463/E0786/E0282), see project memory: `cargo clean -p` +
  `RUST_MIN_STACK=64MB -j4`.
