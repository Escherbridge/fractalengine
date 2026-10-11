---
type: Implementation Plan
title: "Implementation Plan: Per-Endpoint Read/Write API Surface"
tags: [endpoint_api_surface_20260725]
resource: ./spec.md
---

# Implementation Plan: Per-Endpoint Read/Write API Surface

Six phases: addressing → read → write → MCP → egress seam → new-type queries.
Read before write to de-risk the mutate path (ratified Q-3). TDD on the address
projection + query composition; single sweep at the end (N-6). The cleanest-
isolated Wave-1 track (own crates).

## Phase 1: Public endpoint addressing (FR-1) [P0]

- [x] Task: project T1's stable address → public URI; resolve URI→object and *(AUDIT 2026-10-10: cdd3291 — `GET /api/v1/nodes/{id}/address` + `GET /api/v1/address?uri=` (fe-api/src/endpoint.rs); fe-renderer/src/addressing.rs tests `render_uri_matches_t1_format`, `stable_across_rename_and_move`)*
      back; reconcile `fe-renderer/src/addressing.rs`; stability-across-move test.

## Phase 2: Read endpoint per object (FR-2) [P1]

- [x] Task: GET an object's data through `fe-query` (common + type-specific); *(AUDIT 2026-10-10: `GET /api/v1/nodes/{id}` → `EndpointNodeDto` via tombstone-filtered `load_node`; MCP `read_node`; tests `endpoint.rs::addressable_id_rejects_junk`, existence-oracle parity in `tests/existence_oracle_test.rs`. GAP: no per-type (stamp/earthwork/path) payload test)*
      typed error on unknown/unauthorized (no panic); per-type read tests
      (props/geometry/children).

## Phase 3: Write / mutate endpoint (FR-3) [P1]

- [ ] Task: mutations routed through T1's sync-safe ops (move/scale/rotate/ *(AUDIT 2026-10-10: PARTIAL — delete/cascade (`DELETE /api/v1/nodes/{id}[?cascade=true]`) and promote route through T1 ops; move/scale/rotate/props use the same DbCommands the UI uses (UpdateNodeTransform, property cmds). OPEN: no 'same op-log entry as UI' parity test; no trigger-op (e.g. re-sculpt an earthwork region) endpoint)*
      edit-props/delete/trigger); same op-log entry as the UI path (test).
- [ ] Task: fe-policy authorization on writes (reject test); position-locked *(AUDIT 2026-10-10: PARTIAL — authz: CallerAuth::Identified + fe-policy gate at the DB thread, `endpoint.rs::caller_auth_is_identified_never_local`, MCP `every_tool_rejects_a_sub_min_role_token`. OPEN: position lock — fe-database/src/handlers/transform.rs `update_node_transform_handler` has no node_kind=stamp check, so REST/MCP update_transform can free-translate a promoted stamp (FR-3 acceptance unmet))*
      objects reject illegal free-translate (test).

## Phase 4: MCP per-endpoint CRUD (FR-4) [P1]

- [x] Task: extend `fe-api/src/mcp.rs` so an MCP client can address/read/mutate/ *(AUDIT 2026-10-10: MCP `read_node`, `node_address`, `delete_node` (cascade), `promote_instance`, `update_transform` in the ToolSpec table (fe-api/src/mcp/mod.rs, F13 635b8c5); primitives-only preserved by the exact `tools_list_inventory` assertion)*
      delete an endpoint; authz + sync-safety preserved; primitives-only
      guarantees preserved or explicitly extended.

## Phase 5: Copy-API-string / report seam (FR-5) [P1]

- [x] Task: extend `gis/egress_strings.rs` — per-object string + report; expose *(AUDIT 2026-10-10: fe-ui/src/gis/egress_strings.rs:214 `api_string_for`, :223 `report_for` (tests `api_string_for_returns_node_scoped_sql`, `report_for_contains_endpoint_and_query_and_rejects_empty`); called by fe-ui/src/dialogs/context_menu.rs:673-674 and actions/node.rs. GAP: no paste→same-read round-trip test against a live API)*
      the seam T4 calls; round-trip test (paste → same read).

## Phase 6: New-type queries + docs + sweep (FR-6) [P1]

- [ ] Task: make promoted stamp nodes + earthwork regions first-class in *(AUDIT 2026-10-10: PARTIAL — fe-query/src/spatial_nodes.rs (`nodes_of_kind_sql`, `stamps_on_path_sql`, `earthwork_volume_sql`, unit tests incl. `ids_are_bound_not_interpolated`); routes `GET /petals/{p}/nodes?kind=stamp&path_id=` + `GET /petals/{p}/earthwork/summary`; `existence_oracle_test.rs::own_petal_path_still_promotes_and_lists_its_stamps` lists real promoted stamps. OPEN: no integration test of `earthwork/summary` against real T3-baked region rows)*
      `fe-query` (filter/GIS/egress) via the generic node abstraction + type
      tags; integration test against real T2/T3 data at join time ("all stamps
      on path X", "total cut/fill in petal P").
- [x] Task: `fe-api/AGENTS.md` + `fe-query` notes — URI scheme, read/write *(AUDIT 2026-10-10: fe-api/AGENTS.md §endpoint-surface; fe-query/src/AGENTS.md §spatial-nodes (cdd3291))*
      contract, MCP surface, auth model (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests *(AUDIT 2026-10-10: sweep part DONE (F19 3,245/0 @1e9be3a); retro not written; in-app/endpoint verify user-gated)*
      (N-6). Retro; in-app/endpoint verify user-gated.
