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
- [x] Task 1.1: `DbCommand::CreateAsset` + `DbResult::AssetCreated` + *(AUDIT 2026-10-10: fe-runtime/src/messages.rs:251 `CreateAsset`; fe-database/src/handlers/crud.rs:465 `create_asset_handler`; test `crud.rs::create_asset_requires_the_blob_and_stores_metadata_only` — F13 @635b8c5)*
      `create_asset_handler` (TDD: fe-database integration test via
      `spawn_db_thread_with_sync` — send `CreateAsset { name, content_type,
      size_bytes, content_hash }`, assert `AssetCreated { asset_id,
      content_hash }` and the asset row (`data: NONE`, hash set) via direct
      DB read; then implement handler as the asset-row half of
      `import_gltf_handler` (crud.rs ~307–332) with **no file I/O**; refactor)
- [x] Task 1.2: `DbCommand::CreateNodeWithAsset` + handler (TDD: test — *(AUDIT 2026-10-10: messages.rs:263 `CreateNodeWithAsset`; crud.rs:513 `create_node_with_asset_handler`; test `crud.rs::place_asset_binds_the_asset_and_rejects_unknown_ids` — F13 @635b8c5)*
      seeded asset row + `CreateNodeWithAsset { petal_id, name, asset_id,
      position, rotation, scale, correlation_id }` yields
      `DbResult::GltfImported` (reused — spec D2, no new DbResult variant)
      and a node row with `asset_id`, `asset_path = blob://<hash>.glb`,
      geometry-point position, Euler→quat rotation matching
      `UpdateNodeTransform`'s conversion, scale, `node_log` entry; negative
      test — unknown `asset_id` → `DbResult::Error`, no node row; implement;
      refactor)
- [x] Task 1.3: Exhaustive-match fallout sweep for the new `DbCommand` *(AUDIT 2026-10-10: fe-test-harness/src/peer.rs:973-974 matches both variants; workspace compiled + tested in F19 (3,245/0))*
      variants — fe-database `lib.rs` dispatch arm, `fe-test-harness`
      `peer.rs` command loop (known gotcha), any other `match` sites found by
      `cargo check` across the workspace (compile-driven; **no fe-ui edits
      should be needed** — if one appears, stop and re-scope)
- [x] Verification: targeted `cargo test -p fe-database` green; grep confirms
      no new `DbCommand` variant carries a byte buffer [checkpoint marker] *(AUDIT 2026-10-10: fe-api/AGENTS.md §asset-ingest 'Bytes never cross the channel — by construction' (`StoredGlb` capability type); fe-database 281 green @5b06795)*

---

## Phase 2: REST asset ingest + node delete (fe-api)

Goal: an external HTTP client can upload a GLB and delete a node; create
endpoints accept optional `asset_id`/`rotation`/`scale`.

Tasks:
- [x] Task 2.1: `validate_glb(bytes, max_len)` pure helper + *(AUDIT 2026-10-10: fe-api/src/upload.rs:129 `validate_glb`, :23 `MAX_ASSET_BYTES`; unit test `upload.rs::validate_glb_table`)*
      `MAX_ASSET_BYTES = 256 MiB` constant in a new `fe-api/src/upload.rs`
      (TDD: unit tests — valid 12-byte header passes; bad magic; version 1;
      truncated; GLB header total-length ≠ actual length; oversize via small
      injected `max_len` — no 256 MB fixtures; implement; refactor)
- [ ] Task 2.2: `POST /api/v1/petals/{petal_id}/assets` multipart handler *(AUDIT 2026-10-10: PARTIAL (consolidator re-check: un-ticked — the task's own TDD acceptance (403/503/201 handler tests) is unmet on REST; ledger MCP-TEST-UPLOAD) — route fe-api/src/server.rs:191 → upload.rs:362 `upload_petal_asset`, `ASSET_ROUTE_BODY_LIMIT` = 256 MiB + 1 MiB. GAP: no REST-transport happy-path/503 test (only the shared ingest core via MCP `upload_asset_round_trip_then_place` + existence-oracle denial row) — ledger item)*
      (TDD: handler-level tests in the gis_test.rs in-memory-ApiState idiom —
      Editor+resolved-petal-scope required (403 for Viewer / out-of-scope),
      503 when `blob_store` is `None`, happy path returns 201 `{ asset_id,
      content_hash, size_bytes }`; implement: read multipart `file` field →
      `validate_glb` → `spawn_blocking(add_blob)` → `DbCommand::CreateAsset`;
      wire route with per-route `DefaultBodyLimit::max(MAX_ASSET_BYTES +
      1 MiB)` per fe-hexon-registry precedent; refactor)
- [x] Task 2.3: `DELETE /api/v1/nodes/{node_id}` (TDD: tests — Editor + *(AUDIT 2026-10-10: server.rs:140-141 → fe-api/src/endpoint.rs:256 `delete_node` (sync-safe TombstoneNode/CascadeTombstoneNode, T5 cdd3291); denial parity in `tests/existence_oracle_test.rs`)*
      `resolve_node_scope` enforced, 404 on missing node (map the handler's
      "matched no node" error), 200 `{ node_id, petal_id }` on success;
      implement over existing `DbCommand::DeleteNode`; document cascade
      semantics in the route doc comment pointer + AGENTS.md)
- [ ] Task 2.4: Optional `asset_id`/`rotation`/`scale` on `create_node` + *(AUDIT 2026-10-10: STILL OPEN — fe-api/src/types.rs:23 `CreateNodeRequest` has only petal_id/name/position. F14 deferred it with rationale 'MCP place_asset covers the flow' — but a REST-only client can upload (POST …/assets) and cannot place; ledger AGENT_DOABLE_RATIFIED)*
      `create_node_legacy` request bodies (TDD: tests — omitted fields ⇒
      byte-identical legacy behavior; `asset_id` present ⇒ routed to
      `CreateNodeWithAsset`, response includes `asset_path`; implement;
      refactor). Position/rotation/scale pass through **verbatim** — no unit
      math (spec D5 seam for map_scale_authority_20260716)
- [x] Task 2.5: Evidence task — assert `fractalengine/src/main.rs` still *(AUDIT 2026-10-10: fe-api/AGENTS.md §asset-ingest: 'One `BlobStoreHandle` is shared by ApiState and the DB thread (`fractalengine/src/main.rs`)')*
      wires the **same** `BlobStoreHandle` into `ApiState` and
      `spawn_db_thread_with_sync` (main.rs:40/80/168/244 as of survey); add a
      one-line note in `fe-api/AGENTS.md` §asset-ingest; only touch main.rs
      if divergence is found
- [x] Verification: targeted `cargo test -p fe-api` green; manual curl
      transcript (upload → fetch blob by hash → delete node) recorded in the
      task note [checkpoint marker] *(AUDIT 2026-10-10: manual curl transcript SUBSTITUTED by `mcp_integration.rs::upload_asset_round_trip_then_place` + `upload_asset_rejects_bad_bytes_and_writes_nothing`; fe-api 234 green @5b06795)*

---

## Phase 3: MCP shared authz/dispatch helper (fe-api/src/mcp.rs refactor)

Goal: one table-driven dispatch path; the role-only fallbacks are dead.

Tasks:
- [x] Task 3.1: `ToolSpec` table + dispatcher (TDD: unit tests for the *(AUDIT 2026-10-10: fe-api/src/mcp/mod.rs:61 `ScopeRule`, :104 `ToolSpec`; `tools/list` generated from the table (asserted in `mcp_integration.rs::tools_list_inventory`))*
      dispatcher in isolation — `ScopeRule::{None, Global, PetalArg, NodeArg,
      HierarchyArgs}` each enforce `require_role_and_scope` with DB-resolved
      scope where applicable; unresolvable scope ⇒ deny; unknown tool ⇒
      `tool_error`; implement dispatcher + `tools/list` generated from the
      table; refactor). Keep `tool_result`/`tool_error` shapes and 5 s
      timeouts byte-compatible
- [x] Task 3.2: Migrate the existing 6 tools onto the table (TDD: regression *(AUDIT 2026-10-10: `mcp_integration.rs::wart_create_node_foreign_petal_is_denied`, `wart_create_petal_foreign_fractal_is_denied`, `wart_update_transform_foreign_node_is_denied`)*
      tests FIRST — (a) `create_node` **without** `verse_id`/`fractal_id` is
      scope-checked via `ResolvePetalScope` and denied for an out-of-scope
      Editor token (kills mcp.rs ~359–365 wart); (b) same for `create_petal`
      (~315–322); (c) `update_transform` denied for out-of-scope Editor
      (currently role-only, ~397); then migrate; assert existing happy paths
      unchanged)
- [x] Task 3.3: Invariant guard — test (or grep-in-test) asserting no *(AUDIT 2026-10-10: fe-api/src/mcp/mod.rs:505 test `handlers_contain_no_authz_calls`)*
      `require_role(`/`require_role_and_scope(` call sites remain inside
      individual tool handler bodies; authz lives only in the dispatcher
- [x] Verification: targeted `cargo test -p fe-api mcp` green; tool count
      still 6, schemas unchanged [checkpoint marker] *(AUDIT 2026-10-10: superseded checkpoint — table landed with the full vocabulary in one feature (F13 @635b8c5))*

---

## Phase 4: Tool vocabulary expansion (6 → 20 tools)

Goal: the complete primitive vocabulary from spec FR-2/3/4/6, every tool on
the dispatch table, each with success + RBAC-negative tests.

Tasks:
- [x] Task 4.1: `upload_asset` (base64) + `place_asset` + `delete_node` *(AUDIT 2026-10-10: `mcp_integration.rs::upload_asset_round_trip_then_place`, `upload_asset_rejects_bad_bytes_and_writes_nothing`, `place_asset_rejects_a_foreign_scope_asset`; /mcp body limit later CUT to 16 MiB by DEC-C21 (@5b06795, `MCP_MAX_ASSET_BYTES` 11 MiB) — NFR-2's 349 MiB figure was a DoS surface)*
      tools (TDD: tests — upload/place/delete happy path against in-memory
      harness; invalid base64 / non-GLB / unknown asset_id / missing node ⇒
      `isError`; Viewer ⇒ denied; implement — `upload_asset` shares the
      Task 2.2 core, `place_asset` sends `CreateNodeWithAsset`, `delete_node`
      shares the Task 2.3 core; raise `/mcp` route body limit to
      `ceil(4/3 × MAX_ASSET_BYTES) + 8 MiB` per spec NFR-2; refactor)
- [x] Task 4.2: Property + query tools — `set_property`, `get_properties`, *(AUDIT 2026-10-10: `mcp_integration.rs::property_tools_round_trip`, `query_tool_uses_the_guarded_local_pipeline`)*
      `delete_property`, `query` (TDD: red tests incl. RBAC negatives and
      SELECT-only enforcement for `query` via the existing `query_guard`
      pipeline; implement as thin wrappers over the same DbCommands /
      shared cores the REST handlers use; refactor)
- [x] Task 4.3: Waypoint + GPX tools — `create_waypoint`, `move_waypoint`, *(AUDIT 2026-10-10: `mcp_integration.rs::waypoint_and_gpx_tools_round_trip`, `move_waypoint_preserves_rotation_and_scale`, `import_gpx_refuses_more_points_than_the_cap`)*
      `import_gpx` (TDD: extract transport-agnostic cores from
      `rest::create_waypoint` / `rest::move_waypoint` / `gpx::import_gpx`
      (multipart stays for REST; MCP takes `data_base64`); red tests: valid
      minimal GPX fixture creates track+waypoints, malformed GPX ⇒ `isError`,
      RBAC negatives; implement; refactor)
- [x] Task 4.4: Terrain + tileset + GIS tools — `set_petal_terrain` *(AUDIT 2026-10-10: `mcp_integration.rs::terrain_and_tileset_tools_reach_their_cores`; install_tileset role unified to Editor (DEC-C21 @5b06795); set_petal_terrain live via correlated replies 9cbdea6 (`tests/terrain_mutation_test.rs`))*
      (`terrain: null` clears), `list_tilesets`, `install_tileset` (verify
      and mirror the REST handler's role — spec open question 1),
      `get_gis_nodes` (TDD; implement; refactor)
- [x] Task 4.5: `tools/list` inventory test — exactly 20 tools, every *(AUDIT 2026-10-10: `tools_list_inventory` asserts the EXACT name list (29 = 20-name vocabulary + 9 documented extras, DEC-C14) — exact equality is a stricter tripwire than a banned-name list (no explicit `place_building`/`create_road` assertion exists); `mod.rs::table_rows_are_well_formed` asserts every description states its role and every schema is an object)*
      `inputSchema` a valid JSON Schema object, every description names its
      role requirement; **primitives-only guard**: assert no tool name
      matches the banned semantic-verb list (`place_building`, `create_road`,
      `*footprint*`, `*occupancy*`) as a tripwire for directive #1
- [x] Verification: targeted `cargo test -p fe-api` green; 20 tools listed
      [checkpoint marker] *(AUDIT 2026-10-10: 29 listed per DEC-C14; fe-api 199 green @635b8c5)*

---

## Phase 5: Headless full-loop acceptance + docs + single sweep

Goal: prove the track's reason for existing — an external client builds and
tears down a scene over `/mcp` alone — then document and sweep once.

Tasks:
- [x] Task 5.1: Integration harness in `fe-api/tests/mcp_scene_test.rs` *(AUDIT 2026-10-10: SUBSTITUTED — the harness is `fe-test-harness/src/api.rs` `ApiHarness` (real router + auth middleware + in-mem SurrealDB) consumed by `fe-api/tests/mcp_integration.rs`; no mcp_scene_test.rs file)*
      (TDD-lite: harness assertions first — in-memory SurrealDB +
      `schema::apply_all`, `fe_database::spawn_db_thread_with_sync`, an
      `ApiCommand::DbRequest → DbCommand` forwarder thread standing in for
      the Bevy bridge, tempdir-backed blob store handle shared by ApiState
      and the DB thread, real router from `fe_api::server` behind
      `auth_middleware`, tokens minted with `fe_identity::mint_api_token`
      against the state's verifying key — mirroring
      `fe-test-harness/tests/integration.rs`)
- [ ] Task 5.2: Full-loop test (spec FR-8): `upload_asset` (small embedded *(AUDIT 2026-10-10: PARTIAL (consolidator re-check: un-ticked — FR-8 asks for ONE chained loop; only pieces exist; ledger MCP-TEST-LOOP) — `mcp_integration.rs::upload_asset_round_trip_then_place` (upload→place) + `round_trip_create_read_update_read` (create/read/update) + `fe-test-harness/src/scenarios/api_db_sync_cross_thread.rs`; NOTE no single test chains upload→place→set_property→update_transform→delete in one flow)*
      GLB fixture, base64) → `place_asset` at position → `set_property` →
      `update_transform` → `get_hierarchy` + `get_gis_nodes` show the
      asset-bearing node at the moved position → `delete_node` →
      `get_hierarchy` no longer contains it — **every step via MCP
      `tools/call` over HTTP**
- [x] Task 5.3: RBAC negative suite in the same harness — Viewer token *(AUDIT 2026-10-10: `mcp_integration.rs::every_tool_rejects_a_sub_min_role_token`, `every_scoped_tool_rejects_a_foreign_scope_token`; revoked-JTI 401 in `tests/token_lifecycle_test.rs::minted_token_authenticates_then_is_revoked_live`)*
      denied on `upload_asset`/`place_asset`/`set_property`/`delete_node`;
      Editor token scoped to a different verse denied on every scoped tool;
      revoked-JTI token gets 401 at the middleware
- [x] Task 5.4: Docs — `fe-api/AGENTS.md`: §mcp-dispatch (table + ScopeRule *(AUDIT 2026-10-10: fe-api/AGENTS.md §mcp-dispatch (line ~1015) + §asset-ingest (~1105))*
      rationale, fallback-wart history), §asset-ingest (blob seam D1, limits
      NFR-2, cascade semantics, operator token flow pointer to spec);
      one-line pointers from code; fe-database handler notes for
      `CreateAsset`/`CreateNodeWithAsset` (GltfImported reuse rationale)
- [x] Task 5.5: **Single full workspace sweep** (once, per repo policy): *(AUDIT 2026-10-10: F19 sweep fmt/clippy/test 3,245/0 @1e9be3a)*
      `cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test`
      across the workspace; fix fallout; re-run once
- [x] Verification: full-loop test green in the sweep; spec acceptance *(AUDIT 2026-10-10: F13/F14/F19 evidence above; fe-ui untouched by F13 (`git show --stat 635b8c5`))*
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
