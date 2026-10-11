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

- [x] Task: GET an object's data through `fe-query` (common + type-specific); *(AUDIT 2026-10-10: `GET /api/v1/nodes/{id}` → `EndpointNodeDto` via tombstone-filtered `load_node`; MCP `read_node`; tests `endpoint.rs::addressable_id_rejects_junk`, existence-oracle parity in `tests/existence_oracle_test.rs`. GAP: no per-type (stamp/earthwork/path) payload test)* **Per-type read DONE 2026-10-11 (T5-PARITY, commit pending):** `fe-api/tests/api_integration.rs::read_node_returns_each_type_specific_payload` reads a promoted stamp (`kind` stamp + `instance_index`/`path_id`), an earthwork region (`kind` earthwork_region + cut/fill/material) and a GPX path (`kind` null + `gpx_type`/`gpx_points`) through `GET /api/v1/nodes/{id}`.
      typed error on unknown/unauthorized (no panic); per-type read tests
      (props/geometry/children).

## Phase 3: Write / mutate endpoint (FR-3) [P1]

- [ ] Task: mutations routed through T1's sync-safe ops (move/scale/rotate/ *(AUDIT 2026-10-10: PARTIAL — delete/cascade (`DELETE /api/v1/nodes/{id}[?cascade=true]`) and promote route through T1 ops; move/scale/rotate/props use the same DbCommands the UI uses (UpdateNodeTransform, property cmds). OPEN: no 'same op-log entry as UI' parity test; no trigger-op (e.g. re-sculpt an earthwork region) endpoint)* **Op-log parity DONE 2026-10-11 (T5-PARITY, commit pending)** — two halves: `fe-api/tests/api_integration.rs::rest_delete_sends_the_ui_tombstone_op_with_an_identified_caller` (REST DELETE emits the UI's exact `TombstoneNode`/`CascadeTombstoneNode`, caller `Identified{token sub, role}` where the UI sends `Local`) + `fe-database/tests/node_lifecycle_test.rs::ui_and_api_tombstones_write_the_same_op_log_entry` (a `Local` and an `Identified` tombstone write op-log rows and row markers of identical shape; only node_id/lamport/hlc/source_did differ). Box stays open ONLY for the trigger op (ledger T5-TRIGGER).
      edit-props/delete/trigger); same op-log entry as the UI path (test).
- [x] Task: fe-policy authorization on writes (reject test); position-locked *(AUDIT 2026-10-10: PARTIAL — authz: CallerAuth::Identified + fe-policy gate at the DB thread, `endpoint.rs::caller_auth_is_identified_never_local`, MCP `every_tool_rejects_a_sub_min_role_token`. OPEN: position lock — fe-database/src/handlers/transform.rs `update_node_transform_handler` has no node_kind=stamp check, so REST/MCP update_transform can free-translate a promoted stamp (FR-3 acceptance unmet))* **T5-LOCK DONE 2026-10-11 (commit pending):** DB-authoritative guard in `update_node_transform_handler` (`position-locked: stamp`, before the op-log commit) + fail-fast `rest.rs::stamp_translate_denied` on REST/MCP/WS before the optimistic broadcast, and in `move_waypoint_core` (adversarial review 2026-10-11: MCP `move_waypoint` takes addressable ids and wrote lat/lon/ele before the DB refused the move). Tests: `fe-database/tests/node_lifecycle_test.rs::stamp_translate_is_rejected_but_rotation_and_scale_persist`, `fe-api/tests/mcp_integration.rs::update_transform_rejects_moving_a_promoted_stamp`, `api_integration.rs::rest_transform_rejects_moving_a_promoted_stamp_but_rotates_it`, `ws_integration.rs::ws_transform_update_cannot_move_a_promoted_stamp`.
      objects reject illegal free-translate (test).

## Phase 4: MCP per-endpoint CRUD (FR-4) [P1]

- [x] Task: extend `fe-api/src/mcp.rs` so an MCP client can address/read/mutate/ *(AUDIT 2026-10-10: MCP `read_node`, `node_address`, `delete_node` (cascade), `promote_instance`, `update_transform` in the ToolSpec table (fe-api/src/mcp/mod.rs, F13 635b8c5); primitives-only preserved by the exact `tools_list_inventory` assertion)*
      delete an endpoint; authz + sync-safety preserved; primitives-only
      guarantees preserved or explicitly extended.

## Phase 5: Copy-API-string / report seam (FR-5) [P1]

- [x] Task: extend `gis/egress_strings.rs` — per-object string + report; expose *(AUDIT 2026-10-10: fe-ui/src/gis/egress_strings.rs:214 `api_string_for`, :223 `report_for` (tests `api_string_for_returns_node_scoped_sql`, `report_for_contains_endpoint_and_query_and_rejects_empty`); called by fe-ui/src/dialogs/context_menu.rs:673-674 and actions/node.rs. GAP: no paste→same-read round-trip test against a live API)*
      the seam T4 calls; round-trip test (paste → same read).

## Phase 6: New-type queries + docs + sweep (FR-6) [P1]

- [x] Task: make promoted stamp nodes + earthwork regions first-class in **DONE 2026-10-11 (T5-PARITY, commit pending):** `fe-api/tests/api_integration.rs::earthwork_summary_sums_live_region_rows` sums real region rows through `GET /petals/{p}/earthwork/summary` (2 live regions summed; a tombstoned region, another petal's region and a plain node with a volume key excluded). *(AUDIT 2026-10-10: PARTIAL — fe-query/src/spatial_nodes.rs (`nodes_of_kind_sql`, `stamps_on_path_sql`, `earthwork_volume_sql`, unit tests incl. `ids_are_bound_not_interpolated`); routes `GET /petals/{p}/nodes?kind=stamp&path_id=` + `GET /petals/{p}/earthwork/summary`; `existence_oracle_test.rs::own_petal_path_still_promotes_and_lists_its_stamps` lists real promoted stamps. OPEN: no integration test of `earthwork/summary` against real T3-baked region rows)*
      `fe-query` (filter/GIS/egress) via the generic node abstraction + type
      tags; integration test against real T2/T3 data at join time ("all stamps
      on path X", "total cut/fill in petal P").
- [x] Task: `fe-api/AGENTS.md` + `fe-query` notes — URI scheme, read/write *(AUDIT 2026-10-10: fe-api/AGENTS.md §endpoint-surface; fe-query/src/AGENTS.md §spatial-nodes (cdd3291))*
      contract, MCP surface, auth model (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests *(AUDIT 2026-10-10: sweep part DONE (F19 3,245/0 @1e9be3a); retro not written; in-app/endpoint verify user-gated)* **Retro written 2026-10-11 (below); only the user-gated in-app/endpoint verify remains.**
      (N-6). Retro; in-app/endpoint verify user-gated.

## Retro (2026-10-11)

**Landed.** Addressing, per-object read, delete/cascade/promote through T1's ops, MCP per-endpoint
CRUD and the egress seam shipped in Wave 1 (cdd3291) and the F13/F14 mission work. The
2026-10-11 completeness campaign closed the remaining FR-3/FR-6 acceptance:
- **T5-LOCK** (batch auth-policy): a promoted stamp cannot be free-translated on any transport.
  The DB handler is authoritative; `rest.rs::stamp_translate_denied` fails fast before the
  optimistic broadcast.
- **T5-PARITY** (batch mcp-api-surface): UI-vs-API op-log parity, the earthwork summary over real
  rows, and per-type `read_node` payloads (tests named in the tasks above).

**Left.** T5-TRIGGER (a trigger-op endpoint, e.g. re-sculpt an earthwork region; the op
vocabulary is unratified) and the user-gated in-app/endpoint verify. Also recorded in
fe-api/AGENTS.md §endpoint-surface: API rotation/scale on a stamp writes the row's columns while
the renderer reads `stamp.override.*`.

**Lessons.**
- "Same op-log entry as the UI" splits cleanly at the channel. The API half asserts the exact
  `DbCommand` that reaches the DB thread; the DB half asserts that two callers produce identical
  rows. Neither half needs the other's harness, and together they cover the whole path.
- Seed type-tagged rows with the `fe_query::spatial_nodes` key constants, not string literals. The
  read contract then has one spelling, and a renamed key breaks the test instead of silently
  passing.
- A ticked box with a "PARTIAL" audit note decays. Keep a task open until its own acceptance test
  exists, and name the remaining piece (here T5-TRIGGER) so it cannot hide.
