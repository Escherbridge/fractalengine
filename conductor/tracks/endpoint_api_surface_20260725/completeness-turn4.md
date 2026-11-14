---
type: Review
track: endpoint_api_surface_20260725
critic: C-DATA
round: 4
---

# C-DATA completeness — endpoint_api_surface_20260725 (round 4)

Checked at public HEAD `733a3256`. No cargo, no runs.

## Functional requirements

| Requirement | Status | Evidence |
|---|---|---|
| FR-1 public URI, resolve both ways, stable across move | complete | fe-api/src/endpoint.rs:87 `get_node_address`, :119 `resolve_address`; routes fe-api/src/server.rs:184, :188; fe-renderer/src/addressing.rs:169 `render_uri_matches_t1_format`, :197 `stable_across_rename_and_move` |
| FR-2 per-object read incl. type-specific payload, typed errors | complete | endpoint.rs:163 `get_node`, :547 `addressable_id_rejects_junk`; fe-api/tests/api_integration.rs:705 `read_node_returns_each_type_specific_payload` |
| FR-3 move/scale/rotate/props/delete through the UI's ops; authz; stamp lock | partial | Delete parity api_integration.rs:604 + fe-database/tests/node_lifecycle_test.rs:909; stamp lock node_lifecycle_test.rs:786, api_integration.rs:523, mcp_integration.rs:1656, ws_integration.rs:375; authz endpoint.rs:556, mcp_integration.rs:2253. Round 4 DATA-04: stamp rotate/scale write `stamp.override.*` in the same transaction (fe-database/src/handlers/transform.rs:28-30, :62; stamp_transform.rs:15 `euler_to_quat_xyzw`, :29 `mirror_overrides`, :36 scale range) asserted at node_lifecycle_test.rs:847-852; GUI fan-out handlers/mod.rs:80, lib.rs:1090/:2171. Trigger-op absent (T5-TRIGGER) |
| FR-4 MCP per-endpoint CRUD | complete (trigger excluded) | fe-api/src/mcp/mod.rs:237 `update_transform`, :239 `read_node`, :247 `node_address`, :249 `delete_node`, :251 `promote_instance`; mcp_integration.rs:1134 `tools_list_inventory` |
| FR-5 copy-API string round-trips (paste -> same read) | complete | fe-ui/src/gis/egress_strings.rs:252 `api_string_for`, :261 `report_for`, :420 `pasted_string_reads_same_node_then_nothing_after_tombstone` (real DB thread, ScopedQuery, GetNodeProperties parity, tombstone hides); tombstone predicate :86, :94, :104, :128; fe-ui/src/panels/query_tab.rs:44, :54 (QA-0127); escaping :79 (R-DATA-1 D1) |
| FR-6 stamps and regions first-class in fe-query | complete | fe-query/src/spatial_nodes.rs:75, :90, :109, :195; endpoint.rs:424, :469; api_integration.rs:643 `earthwork_summary_sums_live_region_rows` |
| N-4/N-5/N-10, docs (N-7) | complete | fe-api/AGENTS.md:152 §endpoint-surface (write-through at :193-196); fe-query/src/AGENTS.md:6 §spatial-nodes; fe-database/src/handlers/AGENTS.md:191 §stamp-transform |

## Plan tasks

| Task | Status | Evidence / class |
|---|---|---|
| Phase 1 addressing | complete | FR-1 row |
| Phase 2 per-object read | complete | FR-2 row |
| Phase 3 task 1 mutations incl. trigger-op, op-log parity | blocked-user (scoper) | everything but the trigger op is done (FR-3 row); T5-TRIGGER is §9 Q33 |
| Phase 3 task 2 authz + position lock | complete | FR-3 row |
| Phase 4 MCP CRUD | complete | FR-4 row |
| Phase 5 copy-API seam + round trip | complete | the audit GAP (no round-trip test) is closed by egress_strings.rs:420; plan text does not say so yet |
| Phase 6 new-type queries, docs | complete | FR-6 row |
| Phase 6 sweep / retro / in-app verify | blocked-user (scoper) | retro written 2026-10-11; sweep and verify user-gated |

Tests reach batches: node_lifecycle_test via conductor/test-batches.md:59; api/mcp/ws integration via :330; egress_strings via `gis::` at :112. `handlers::stamp_transform::tests` (stamp_transform.rs:82) is reached by no filter.

## §9 default Q33 (T5-TRIGGER)

Code matches default (a): no trigger-op route or MCP tool exists. Track text does not: spec.md FR-3/FR-4 still list trigger-op and the Phase 3 box stays open, where default (a) says amend FR-3 and close the box.

## HANDED-OFF (owner = next round)

1. HANDED-OFF (T-DATA / I-CONDUCTOR, next round; only if the user leaves Q33 at default a) - spec.md FR-3/FR-4: mark trigger-op out of scope (re-sculpt is Bevy-side, fe-ui/src/actions/terrain_proposal.rs, no DbCommand) and close the Phase 3 task 1 box.
2. HANDED-OFF (T-DATA, next round) - plan.md Phase 5: record that DATA-05 closed the round-trip gap (egress_strings.rs:420) and QA-0127; Retro "Left" paragraph still says API rotate/scale writes only row columns, stale since DATA-04.
3. HANDED-OFF (I-BATCH, next round) - conductor/test-batches.md R1 fe-database lib command (line 58): add `handlers::stamp_transform` (same item as rich_data HANDED-OFF 1).
4. HANDED-OFF (campaign build-and-verify phase) - Phase 6 sweep and in-app/endpoint verify.
- SEC-INBOX: no entry names a file of this track.

Verdict: COMPLETE for code (trigger-op is a pending user decision, defaulted to out of scope); track text and one batch filter lag behind.
