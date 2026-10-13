---
type: Implementation Plan
title: "Implementation Plan: API + MCP Integration Tests"
tags: [api_mcp_integration_tests_20260717]
resource: ./spec.md
---

# Implementation Plan: API + MCP Integration Tests

## Overview

Harness first (FR-1), then the two suites in parallel-safe order. Suites are
written against current behavior — including explicitly-marked KNOWN-WEAK authz
cases — so they are green at landing and act as a ratchet for
`mcp_scene_primitives_20260716`. Phase 4 (FR-5, user ask 2026-07-18) expands to
the remaining surface on the same harness. Full workspace sweep runs ONCE at
session end.

## Phase 1: Harness (FR-1)

- [x] Task: Shared test-support module — in-mem SurrealDB bring-up, scope/role fixture seeding, real fe-api router construction, tower `oneshot` dispatch helpers, authed/unauthed request builders, JSON assertion helpers *(AUDIT 2026-10-10: `fe-test-harness/src/api.rs` `ApiHarness` (spawn/mint_token/post_json/request), commit 4e79780.)*
- [x] Task: Wire `fractalengine-test-harness` as the fixture/assertion provider (dev-dependency from fe-api) — first downstream consumer, SG-08 gap *(AUDIT 2026-10-10: fe-api/Cargo.toml:46 `fractalengine-test-harness = { path = "../fe-test-harness" }` (dev-dep), 4e79780.)*
- [x] Verification: one smoke test (health/list endpoint) proves the harness end-to-end [checkpoint] *(AUDIT 2026-10-10: `fe-api/tests/api_harness_smoke.rs::harness_health_ready_and_authed_roundtrip`, 4e79780.)*

## Phase 2: api_integration.rs (FR-2)

- [x] Task: Query-guard tests — cost/row/timeout limits, row-cap errors (not truncates) *(AUDIT 2026-10-10: `api_integration.rs::query_happy_path_and_single_select_enforced`, `query_byte_ceiling_errors_not_truncates`; row cap in `export_share_test.rs::row_cap_produces_clear_error`.)*
- [x] Task: Injection tests — SQL-injection attempts via params + raw-SQL endpoints rejected *(AUDIT 2026-10-10: `api_integration.rs::query_injection_guard_rejections`; `tests/record_id_read_test.rs` (ba0b8d2).)*
- [x] Task: RBAC negative tests — Viewer/None write denials, cross-scope denials *(AUDIT 2026-10-10: `api_integration.rs::rbac_insufficient_role_denied`, `rest_creates_deny_a_foreign_target_behind_an_own_prefix`, `field_def_mutations_require_the_stored_scope`; `tests/existence_oracle_test.rs` (04c91f1).)*
- [x] Task: GIS tests — spatial queries round-trip against seeded geometry *(AUDIT 2026-10-10: `api_integration.rs::gis_nodes_round_trip`; `tests/gis_test.rs`.)*
- [x] Task: Egress CSV tests — well-formed CSV, expected headers, guard limits applied *(AUDIT 2026-10-10: `api_integration.rs::export_csv_returns_rows_and_headers`; `export_share_test.rs::export_csv_local_vs_latlon_landmine`.)*

## Phase 3: mcp_integration.rs (FR-3)

- [x] Task: MCP round-trip tests — list tools, call each existing tool, assert DB effect + response shape *(AUDIT 2026-10-10: `mcp_integration.rs::tools_list_inventory` (exact 29-tool list), `round_trip_create_read_update_read`, `property_tools_round_trip`, `waypoint_and_gpx_tools_round_trip`, `terrain_and_tileset_tools_reach_their_cores`, `upload_asset_round_trip_then_place`.)*
- [x] Task: Weak-authz KNOWN-WEAK markers — create_node / create_petal / update_transform current behavior pinned, named to reference mcp_scene_primitives_20260716 *(AUDIT 2026-10-10: landed as KNOWN-WEAK in 4e79780, then FLIPPED to strict by F13 @f65aaa2: `mcp_integration.rs::wart_create_node_foreign_petal_is_denied`, `wart_create_petal_foreign_fractal_is_denied`, `wart_update_transform_foreign_node_is_denied`, `wart_depth_escalation_is_denied`.)*
- [x] Task: Cross-reference note added to mcp_scene_primitives_20260716 (flip markers to strict when dispatcher lands) *(AUDIT 2026-10-10: moot — the flip itself landed (F13 @f65aaa2, the wart_* tests above); mcp_scene_primitives_20260716 metadata records the three warts fixed.)*

## Phase 4: Comprehensive expansion (FR-5, user ask 2026-07-18)

- [x] Task: Endpoint-family inventory — enumerate the full fe-api router surface vs current coverage; record covered / to-cover / N/A (incl. whether a WS/realtime surface exists) [checkpoint: inventory in track folder] *(DONE 2026-10-11, commit pending — API-INV-1: [endpoint-inventory.md](./endpoint-inventory.md) lists all 68 `server.rs` path literals (74 method rows) with handler + covering test or reason, `/ws` (live-socket tests), `/mcp` (29-row table: 24 tools off a lab host, 29 on one), and the unmounted `/ws/canonical` as N/A; cross-checked mechanically against `grep -o '"/[^"]*"' fe-api/src/server.rs | sort -u` (68). Gap summary at the end of the file names the routes covered only on their denial path.)* *(AUDIT 2026-10-10: STILL OPEN — no inventory artifact in this folder. Partial substitutes: the route table in `fe-api/tests/existence_oracle_test.rs` and the fe-api/AGENTS.md route index; ~66 routes in fe-api/src/server.rs.)*
- [x] Task: Hexon/tileset endpoint tests (`/crates/*` as shipped; annotate hexon_unification_20260716 re-point dependency) *(AUDIT 2026-10-10: `tests/hexon_tileset_test.rs::install_then_list_meta_and_tile_round_trip_with_petal_binding_authz` (F14 @917b502); `/crates/*` denial rows in `existence_oracle_test.rs`.)*
- [x] Task: IoT tests — batch ingestion semantics + guard whitelist seam + reading-shaped export *(AUDIT 2026-10-10: `tests/iot_ingest_test.rs` + `export_share_test.rs::export_readings_round_trips_scope_filtered_with_anchor_position`, `reading_row_cap_reuses_the_same_guarded_mechanism_as_nodes` (F11 @73f4777).)*
- [x] Task: Share-URL lifecycle tests — mint→redeem, scope ceiling, expired + tampered-signature negatives *(AUDIT 2026-10-10: `export_share_test.rs::share_issue_redeem_round_trip_enforces_scope_ceiling`, `share_expired_tampered_and_invalid_rejected`; `tests/share_signer_persistence_test.rs`.)*
- [x] Task: Auth token lifecycle tests — issue, use, expiry, revocation (session-cache TTL path) *(AUDIT 2026-10-10: `tests/token_lifecycle_test.rs::minted_token_authenticates_then_is_revoked_live`, `short_ttl_token_authenticates_then_expires_live` (F14 @917b502).)*
- [x] Task: Cross-thread scenarios via fractalengine-test-harness — API write observed through the DB thread channel contract; sync-facing effect asserted at the seam (mock replicator boundary) *(AUDIT 2026-10-10: `fe-test-harness/src/scenarios/api_db_sync_cross_thread.rs` (real DB thread + real loopback iroh — stronger than the mock boundary), F14 @917b502.)*
- [x] Task: MCP negative/fuzz — malformed frames, unknown tools, wrong-typed/oversized args, missing scope; bounded seeded structured-fuzz over tool arg schemas (graceful errors, no panics) *(AUDIT 2026-10-10: `mcp_integration.rs::unknown_method_and_unknown_tool`, `malformed_arguments_error_not_panic`, `every_scoped_tool_rejects_a_foreign_scope_token`, `mcp_structured_fuzz_never_panics_or_leaks_to_channel` (repaired @79e7ac9).)*

## Phase 5: Close-out

- [x] Task: Suites green in the single end-of-session workspace sweep (FR-4, acceptance gate) *(AUDIT 2026-10-10: F19 workspace sweep 3,245 passed / 0 failed @d198bd2 (mission_prodready_continuation_20261008 plan §M7).)*
- [x] Task: Retro + archive per track-per-feature workflow *(AUDIT 2026-10-10: retro = the F18 dated note in metadata.json (6c0bca1); archive flag left to the board owner.)*
