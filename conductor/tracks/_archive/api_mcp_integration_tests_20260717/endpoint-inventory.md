---
type: Inventory
title: "fe-api endpoint inventory: router surface vs test coverage"
tags: [api_mcp_integration_tests_20260717, fe-api, inventory, coverage]
resource: ./plan.md
---

# fe-api endpoint inventory (API-INV-1, 2026-10-11)

This file lists every route the fe-api router mounts (`fe-api/src/server.rs::build_router`), the
handler behind it, and the test that covers it. Where a route has no test, the row says why. It
also covers the `/ws` realtime surface, the `/mcp` tool table, and the unmounted `/ws/canonical`.

**Mechanical cross-check.** Run from the repo root:

```bash
grep -o '"/[^"]*"' fe-api/src/server.rs | sort -u | wc -l   # -> 70
```

There are **70 distinct path literals**, and each one appears in this file. That is 67
`/api/v1/*` paths plus `/ready`, `/ws` and `/mcp`. The 70 paths carry **78 method rows**,
because some paths take more than one method: 77 rows in the route tables, plus `POST /mcp`,
which is described in its own section. (VIS-PH6, 2026-10-11, added the two
visibility-groups paths and their four method rows.)

A scripted check confirmed that every one of the 68 literals (pre-VIS-PH6) appears in this
file as a backticked path, and that the route tables held 73 method rows. `/ws/canonical` is not in
`server.rs`; it is listed separately as N/A.

**Legend.**
- **EO** = `fe-api/tests/existence_oracle_test.rs::unknown_and_foreign_ids_are_indistinguishable_on_every_scoped_route`.
  It drives the real router and checks one thing: a never-existing id and a real id in another
  verse get byte-identical denials. A route covered only by EO has its **denial path** tested and
  its happy path untested.
- **MCP twin** = the happy path runs through the shared `*_core` function that the matching MCP
  tool calls (see the MCP table). The REST handler itself is covered only on the denial path.
- **GAP** = no test exercises the happy path on either transport. These are listed for
  follow-up and are not fixed in this batch.

All test paths below are relative to `fe-api/tests/` unless they name another crate.

## Public routes (no bearer)

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| GET | `/api/v1/health` | inline closure | `api_harness_smoke.rs::harness_health_ready_and_authed_roundtrip` | |
| GET | `/ready` | `server::ready_handler` | `api_harness_smoke.rs::harness_health_ready_and_authed_roundtrip` | |
| GET | `/ws` | `ws::ws_handler` | `ws_integration.rs::ws_handshake_auth_subscribe_and_scene_broadcast_end_to_end`, `ws_rejects_a_bad_token_and_closes`, `ws_unknown_and_foreign_ids_get_identical_denials`, `ws_transform_update_cannot_move_a_promoted_stamp` | The realtime surface exists. Tests use a live tungstenite socket on a bound listener. |
| GET | `/api/v1/shared/{token}` | `share::redeem_share_url` | `export_share_test.rs::share_issue_redeem_round_trip_enforces_scope_ceiling`, `share_expired_tampered_and_invalid_rejected`; `share_signer_persistence_test.rs` | |

## Hierarchy + per-endpoint surface

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| GET | `/api/v1/hierarchy` | `rest::get_hierarchy` | `token_lifecycle_test.rs::minted_token_authenticates_then_is_revoked_live` | |
| POST | `/api/v1/verses` | `rest::create_verse` | `api_integration.rs::rbac_insufficient_role_denied` (role floor) | Happy path is the MCP twin (`create_verse`, `mcp_integration.rs::round_trip_create_read_update_read`). |
| POST | `/api/v1/verses/{verse_id}/fractals` | `rest::create_fractal` | `mcp_integration.rs::rest_create_petal_rejects_a_foreign_fractal_behind_an_own_prefix` (calls the handler directly: deny + happy); EO | |
| POST | `/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals` | `rest::create_petal` | `mcp_integration.rs::rest_create_petal_rejects_a_foreign_fractal_behind_an_own_prefix`; `api_integration.rs::rest_creates_deny_a_foreign_target_behind_an_own_prefix`; EO | |
| POST | `/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals/{petal_id}/nodes` | `rest::create_node` | `mcp_integration.rs::rest_create_node_rejects_a_foreign_petal_behind_an_own_prefix` (legacy body byte-identical), `rest_create_node_with_asset_id_is_place_asset` (MCP-2.4); `api_integration.rs::rest_creates_deny_a_foreign_target_behind_an_own_prefix`; EO | |
| POST | `/api/v1/nodes` | `rest::create_node_legacy` | EO (POST-body table) | Shares `create_node_in_petal` with the hierarchical create, so the happy path and asset_id behaviour are covered there. The only difference is that `petal_id` comes from the body. |
| GET | `/api/v1/nodes/{node_id}` | `endpoint::get_node` | `api_integration.rs::read_node_returns_each_type_specific_payload` (T5-PARITY), `foreign_verse_token_is_denied_by_token_scope_policy`; EO | |
| DELETE | `/api/v1/nodes/{node_id}` | `endpoint::delete_node` | `api_integration.rs::rest_delete_sends_the_ui_tombstone_op_with_an_identified_caller` (T5-PARITY); EO | The DB half of the parity check is `fe-database/tests/node_lifecycle_test.rs::ui_and_api_tombstones_write_the_same_op_log_entry`. |
| GET | `/api/v1/nodes/{node_id}/address` | `endpoint::get_node_address` | EO | The MCP twin `node_address` is exercised only by the sweeps and fuzz; no dedicated happy-path assertion exists. The projection is unit-tested in `fe-renderer/src/addressing.rs`. |
| GET | `/api/v1/address` | `endpoint::resolve_address` | `api_integration.rs::foreign_verse_token_is_denied_by_token_scope_policy` (own passes, foreign denied) | |
| GET | `/api/v1/petals/{petal_id}/nodes` | `endpoint::list_nodes_by_kind` | `existence_oracle_test.rs::own_petal_path_still_promotes_and_lists_its_stamps`; EO | |
| GET | `/api/v1/petals/{petal_id}/earthwork/summary` | `endpoint::earthwork_summary` | `api_integration.rs::earthwork_summary_sums_live_region_rows` (T5-PARITY, real rows); EO | |
| POST | `/api/v1/petals/{petal_id}/paths/{path_id}/instances/{instance_index}/promote` | `endpoint::promote_instance` | `existence_oracle_test.rs::own_petal_path_still_promotes_and_lists_its_stamps`; EO | |

## Node transform + properties

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| PATCH | `/api/v1/nodes/{node_id}/transform` | `rest::update_transform` | `api_integration.rs::rest_transform_rejects_moving_a_promoted_stamp_but_rotates_it`; EO | |
| GET | `/api/v1/nodes/{node_id}/transform` | `rest::get_transform` | EO | **GAP**: there is no REST happy-path read. The same `GetNodeTransform` round trip is used internally by `stamp_translate_denied`, which the transform tests cover. |
| PATCH | `/api/v1/nodes/{node_id}/properties` | `rest::set_node_property` | EO | MCP twin: `set_property_core`, `mcp_integration.rs::property_tools_round_trip`. |
| GET | `/api/v1/nodes/{node_id}/properties` | `rest::get_node_properties` | EO | MCP twin: `get_properties_core`, `mcp_integration.rs::property_tools_round_trip`. |
| DELETE | `/api/v1/nodes/{node_id}/properties/{key}` | `rest::delete_node_property` | EO | MCP twin: `delete_property`, `mcp_integration.rs::property_tools_round_trip`. |

## Assets

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| GET | `/api/v1/assets/{content_hash}` | `assets::get_asset` | `asset_ownership_test.rs::unplaced_upload_is_readable_from_its_anchor_petal_only`; `fe-api/src/assets.rs::get_asset_by_hash_requires_a_covering_referencing_node` | |
| GET | `/api/v1/assets/by-id/{asset_id}` | `assets::get_asset_by_id` | `asset_ownership_test.rs` (3 reach tests); `fe-api/src/assets.rs::get_asset_by_id_round_trips_and_resolves_scope` | |
| POST | `/api/v1/petals/{petal_id}/assets` | `upload::upload_petal_asset` | `asset_ownership_test.rs::unplaced_upload_is_readable_from_its_anchor_petal_only` (201 + fetch), `refused_uploads_write_nothing` (403 / 404 / 503, MCP-TEST-UPLOAD), `upload_past_the_owner_quota_is_413_and_writes_nothing`; EO | |
| GET | `/api/v1/nodes/{node_id}/asset` | `assets::get_node_asset` | `fe-api/src/assets.rs::get_node_asset_round_trips_bytes_and_headers` (+ 404 / invalid id); EO | |

## Petal import / export / GPX / waypoints

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| GET | `/api/v1/petals/{petal_id}/export` | `format::export_petal` | EO | **GAP**: no `.hexon` petal-export happy-path test. |
| POST | `/api/v1/petals/{petal_id}/import` | `format::import_petal` | EO | **GAP**: no `.hexon` petal-import happy-path test. |
| POST | `/api/v1/petals/{petal_id}/import/gpx` | `gpx::import_gpx` | EO | MCP twin: `gpx::import_gpx_core`, `mcp_integration.rs::waypoint_and_gpx_tools_round_trip`, `import_gpx_refuses_more_points_than_the_cap`. |
| GET | `/api/v1/petals/{petal_id}/export/gpx` | `gpx::export_gpx` | `api_integration.rs::gpx_export_emits_track_and_both_waypoints_exactly_once`; EO | |
| POST | `/api/v1/petals/{petal_id}/waypoints` | `rest::create_waypoint` | EO | MCP twin: `create_waypoint_core`, `mcp_integration.rs::waypoint_and_gpx_tools_round_trip`. |
| PATCH | `/api/v1/nodes/{waypoint_id}/move` | `rest::move_waypoint` | EO | MCP twin: `move_waypoint_core`, `mcp_integration.rs::move_waypoint_preserves_rotation_and_scale`. |
| GET | `/api/v1/nodes/{track_id}/elevation-profile` | `rest::get_elevation_profile` | EO | **GAP**: no happy-path test. |
| GET | `/api/v1/nodes/{track_id}/stats` | `rest::get_track_stats` | EO | **GAP** at this route. Track stats are asserted through `/gis/tracks` (`gis_test.rs::tracks_returns_only_track_nodes_with_stats`). |

## Terrain, tiles, GIS

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| GET | `/api/v1/petals/{petal_id}/visibility-groups` | `visibility::list_visibility_groups` | `visibility_groups_api_test.rs::a_viewer_reads_but_cannot_replace_and_an_editor_round_trips`, `a_foreign_petal_is_denied_exactly_like_a_missing_one`; EO | |
| PUT | `/api/v1/petals/{petal_id}/visibility-groups` | `visibility::replace_visibility_groups` | `visibility_groups_api_test.rs::a_viewer_reads_but_cannot_replace_and_an_editor_round_trips`, `a_foreign_petal_is_denied_exactly_like_a_missing_one`; EO | |
| PATCH | `/api/v1/petals/{petal_id}/visibility-groups/{group_id}` | `visibility::patch_visibility_group` | `visibility_groups_api_test.rs::patch_upserts_one_group_and_delete_removes_it`; EO | |
| DELETE | `/api/v1/petals/{petal_id}/visibility-groups/{group_id}` | `visibility::delete_visibility_group` | `visibility_groups_api_test.rs::patch_upserts_one_group_and_delete_removes_it`; EO | |
| GET | `/api/v1/petals/{petal_id}/terrain` | `terrain::get_terrain_config` | `terrain_mutation_test.rs::rest_put_persists_terrain_that_get_reads_back`; EO | |
| PUT | `/api/v1/petals/{petal_id}/terrain` | `terrain::set_terrain_config` | `terrain_mutation_test.rs::rest_put_persists_terrain_that_get_reads_back`, `refused_puts_leave_the_stored_terrain_untouched`, `rest_put_answers_with_its_own_write_when_a_gui_set_races_it`; EO | |
| DELETE | `/api/v1/petals/{petal_id}/terrain` | `terrain::delete_terrain_config` | `terrain_mutation_test.rs::rest_delete_clears_the_stored_terrain`; EO | |
| GET | `/api/v1/tiles/elevation/{tileset_id}/{z}/{x}/{y_png}` | `terrain::get_elevation_tile` | `hexon_tileset_test.rs::install_then_list_meta_and_tile_round_trip_with_petal_binding_authz`; EO | |
| GET | `/api/v1/tiles/satellite/{tileset_id}/{z}/{x}/{y_jpg}` | `terrain::get_satellite_tile` | EO | **GAP**: the fixture tileset has no satellite layer, so only the denial path is covered. |
| GET | `/api/v1/tilesets` | `terrain::list_available_tilesets` | `hexon_tileset_test.rs::install_then_list_meta_and_tile_round_trip_with_petal_binding_authz`; EO | |
| GET | `/api/v1/tilesets/{tileset_id}/meta` | `terrain::get_tileset_meta` | `hexon_tileset_test.rs::install_then_list_meta_and_tile_round_trip_with_petal_binding_authz`; EO | |
| GET | `/api/v1/petals/{petal_id}/gis/nodes` | `gis::list_gis_nodes` | `api_integration.rs::gis_nodes_round_trip`; `gis_test.rs` (bbox / radius / bbox_ll / role / scope); EO | |
| GET | `/api/v1/petals/{petal_id}/gis/tracks` | `gis::list_gis_tracks` | `gis_test.rs::tracks_returns_only_track_nodes_with_stats`, `tracks_rejects_insufficient_role`; EO | |

## Field definitions

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| POST | `/api/v1/field-defs` | `rest::create_field_def` | none | **GAP**: no test on any transport. The route is Manager-gated and requires `require_scope` on the body scope. |
| GET | `/api/v1/field-defs/{scope}` | `rest::list_field_defs` | `api_integration.rs::field_def_mutations_require_the_stored_scope` (foreign scope denied); EO | Denial only. |
| PATCH | `/api/v1/field-defs/by-id/{field_def_id}` | `rest::update_field_def` | `api_integration.rs::field_def_mutations_require_the_stored_scope`; EO | Denial only. |
| DELETE | `/api/v1/field-defs/by-id/{field_def_id}` | `rest::delete_field_def` | `api_integration.rs::field_def_mutations_require_the_stored_scope`; EO | Denial only. |

## Query, egress, analytics, IoT

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| POST | `/api/v1/query` | `rest::execute_query` | `api_integration.rs::query_happy_path_and_single_select_enforced`, `query_injection_guard_rejections`, `query_byte_ceiling_errors_not_truncates`; `distributed_query_test.rs::query_distributed_mode_*`; `record_id_read_test.rs`; EO (distributed body) | |
| POST | `/api/v1/query/elevated` | `rest::execute_elevated_query` | `elevated_query_test.rs` (7 tests: opt-in, Owner, op-logged, rollback, timeout, no-db_reader, privilege rows); `record_id_read_test.rs::owner_elevated_query_cannot_read_privilege_rows_by_id` | |
| POST | `/api/v1/query/share` | `share::issue_share_url` | `export_share_test.rs::share_issue_redeem_round_trip_enforces_scope_ceiling`, `egress_dialect_rejects_comments_and_nested_selects`; `share_signer_persistence_test.rs` | |
| GET | `/api/v1/petals/{petal_id}/export.parquet` | `export::export_parquet` | `export_share_test.rs::export_parquet_round_trips_and_is_scope_filtered`; EO | |
| GET | `/api/v1/petals/{petal_id}/export.csv` | `export::export_csv` | `api_integration.rs::export_csv_returns_rows_and_headers`; `export_share_test.rs::export_etag_and_if_range_round_trip`; EO | |
| POST | `/api/v1/analytics/query` | `rest::execute_analytics_query` | `analytics_scope_test.rs` (3 tests); `distributed_query_test.rs::analytics_*`; EO (POST-body table) | |
| POST | `/api/v1/petals/{petal_id}/iot/readings` | `iot::ingest_readings` | `iot_ingest_test.rs::ingest_batch_persists_and_reads_back` (+ 10 more); router path in `fe-test-harness/src/scenarios/api_db_sync_cross_thread.rs`; EO | |

## Hexon tileset management

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| POST | `/api/v1/hexons/tilesets/install` | `terrain::install_hexon_tileset` | `hexon_tileset_test.rs::install_then_list_meta_and_tile_round_trip_with_petal_binding_authz`, `five_mib_tileset_installs_via_rest_and_mcp`; EO | |
| DELETE | `/api/v1/hexons/tilesets/{hexon_id}` | `terrain::remove_hexon_tileset` | EO | **GAP**: denial only. The binding rule it depends on is unit-tested in `fe-api/src/terrain.rs::store_mutation_requires_the_requested_petals_exclusive_binding`. |
| PATCH | `/api/v1/hexons/tilesets/{hexon_id}/seeding` | `terrain::toggle_seeding` | EO | **GAP**: denial only, same binding rule as the DELETE row. |
| GET | `/api/v1/hexons/tilesets` | `terrain::list_installed_hexons` | EO | **GAP**: denial only. |
| GET | `/api/v1/hexons/storage` | `terrain::get_storage_info` | EO | **GAP**: denial only. |

## Hexon crate registry (dormant)

`ApiState.hexon_registry` is `None` on both hosts (`fractalengine/src/main.rs`,
`fractalengine-relay/src/main.rs`). Every crate route therefore answers
"hexon registry not configured" after its guards run. The security audit of 2026-07-31 classifies
this registry as dormant attack surface. For the scoped crate routes, the guards are covered by
EO; there is no happy path to test until a host wires a registry.

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| POST | `/api/v1/crates/publish` | `hexon::publish_crate` | none | N/A while dormant. The route is Owner-gated and scope-less, so it has no EO row. |
| POST | `/api/v1/crates/{hexon_uri}/install` | `hexon::install_crate` | EO | Dormant. |
| DELETE | `/api/v1/crates/{hexon_uri}/uninstall` | `hexon::uninstall_crate` | EO | Dormant. |
| GET | `/api/v1/crates/search` | `hexon::search_crates` | EO | Dormant. |
| GET | `/api/v1/crates/installed` | `hexon::list_installed` | EO | Dormant. |
| GET | `/api/v1/crates/{hexon_uri}` | `hexon::get_crate_manifest` | EO | Dormant. |
| GET | `/api/v1/crates/{hexon_uri}/entries` | `hexon::get_crate_entries` | EO | Dormant. |
| GET | `/api/v1/crates/{hexon_uri}/entries/{entry_id}/asset` | `hexon::get_crate_asset` | EO | Dormant. |
| GET | `/api/v1/crates/available` | `hexon::available_crates` | EO | Dormant. |

## Sim control (Owner; fails closed off a lab host)

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| POST | `/api/v1/sim/start` | `sim::post_start` | `sim_control_test.rs::owner_drives_every_verb_through_the_seam`, `non_owner_is_forbidden_before_the_seam`, `unconfigured_host_fails_closed` | |
| POST | `/api/v1/sim/stop` | `sim::post_stop` | `sim_control_test.rs::owner_drives_every_verb_through_the_seam` (verbs table), `unconfigured_host_fails_closed` | |
| GET | `/api/v1/sim/status` | `sim::get_status` | `sim_control_test.rs::owner_drives_every_verb_through_the_seam`, `unconfigured_host_fails_closed` | |
| POST | `/api/v1/sim/step` | `sim::post_step` | `sim_control_test.rs::owner_drives_every_verb_through_the_seam`, `unconfigured_host_fails_closed` | |
| POST | `/api/v1/sim/inject-fault` | `sim::post_inject_fault` | `sim_control_test.rs::owner_drives_every_verb_through_the_seam`, `unconfigured_host_fails_closed` | |

## `/mcp` (POST, `mcp::mcp_handler`)

**Transport and table.** The route has a 16 MiB body cap and an 8-way concurrency limit. All
calls go through one `ToolSpec` table, as described in fe-api/AGENTS.md §mcp-dispatch.

**Tool counts.** The table has **31 rows**. `tools/list` advertises **26 tools on a non-lab
host** (the GUI always; the relay without `sim-control`) and **31 on a sim lab host**. On a
non-lab host, the five `sim_*` rows are hidden and answer `unknown tool` (SIM-GUI-TOOLS).
`mcp_integration.rs::tools_list_inventory` asserts both configurations.

**Whole-table tests.** Every row is also covered by these tests:
- `every_tool_rejects_a_sub_min_role_token`: role floor.
- `every_scoped_tool_rejects_a_foreign_scope_token`: 23 scoped rows.
- `mcp_structured_fuzz_never_panics_or_leaks_to_channel`: every tool reaches its handler.

**Protocol tests.** `unknown_method_and_unknown_tool` and `malformed_arguments_error_not_panic`
cover the JSON-RPC envelope.

| Tool | Happy-path / behaviour test |
|---|---|
| get_hierarchy | `mcp_integration.rs::round_trip_create_read_update_read`, `mcp_full_scene_loop` |
| create_verse / create_fractal / create_petal / create_node | `mcp_integration.rs::round_trip_create_read_update_read`; `wart_*` denial tests |
| update_transform | `mcp_integration.rs::mcp_full_scene_loop`, `update_transform_world_units_pass_through_unconverted`, `update_transform_rejects_moving_a_promoted_stamp` |
| read_node / delete_node / promote_instance | `mcp_integration.rs::per_endpoint_crud_tools_role_and_validation`, `mcp_full_scene_loop` (delete), `promote_instance_unknown_and_foreign_paths_are_indistinguishable` |
| node_address | Sweeps and fuzz only. There is no dedicated happy assertion; the same gap applies to the REST twin. |
| query_timeseries | `distributed_query_test.rs::mcp_query_timeseries_returns_the_merged_outcome` |
| sim_start / sim_stop / sim_status / sim_step / sim_inject_fault | `sim_control_test.rs::mcp_sim_tools_share_the_guarded_dispatch` (lab host; start + step). The other verbs share the REST dispatch fns covered above. |
| upload_asset / place_asset | `mcp_integration.rs::upload_asset_round_trip_then_place`, `place_asset_rejects_a_foreign_scope_asset`, `mcp_full_scene_loop` (MCP-TEST-LOOP) |
| set_property / get_properties / delete_property | `mcp_integration.rs::property_tools_round_trip`, `mcp_full_scene_loop` |
| create_waypoint / move_waypoint / import_gpx | `mcp_integration.rs::waypoint_and_gpx_tools_round_trip`, `move_waypoint_preserves_rotation_and_scale`, `import_gpx_refuses_more_points_than_the_cap` |
| set_petal_terrain / list_tilesets / install_tileset | `mcp_integration.rs::terrain_and_tileset_tools_reach_their_cores`; `terrain_mutation_test.rs::mcp_set_petal_terrain_sets_then_clears`; `hexon_tileset_test.rs::five_mib_tileset_installs_via_rest_and_mcp` |
| list_visibility_groups / set_visibility_group | `visibility_groups_api_test.rs::mcp_set_then_list_visibility_groups` (Editor upsert + delete, Viewer list, Viewer write refused) |
| get_gis_nodes | `mcp_integration.rs::property_tools_round_trip`, `mcp_full_scene_loop` |
| query | `mcp_integration.rs::query_tool_uses_the_guarded_local_pipeline` |

`/mcp` itself is a path literal in `server.rs` and is counted in the 70.

## Not in the router

| Method | Path | Handler | Covering test | Note |
|---|---|---|---|---|
| GET (upgrade) | `/ws/canonical` | `canonical_ws::handler::canonical_ws_handler` | `fe-api/src/canonical_ws/handler.rs` tests (dispatch, incl. `missing_and_foreign_scopes_get_byte_identical_refusals`); `canonical_ws/router.rs` tests | **N/A, unmounted by design** (CANON-WS stays NO_GO). It is nested only by `build_router_with_canonical_log`, which no host calls. See `fe-api/src/canonical_ws/AGENTS.md` §deliberately-unmounted. |

## Gap summary

These method rows have no happy-path test on either transport:

| Area | Gap rows |
|---|---|
| Node transform | GET `/nodes/{id}/transform` |
| Petal archive | GET `/petals/{p}/export`, POST `/petals/{p}/import` (`.hexon`) |
| Track reads | GET `/nodes/{id}/elevation-profile`, GET `/nodes/{id}/stats` |
| Tiles | GET satellite tile |
| Field definitions | POST `/field-defs` (no test at all); the other three field-def methods are denial-only |
| Hexon tileset management | DELETE `/hexons/tilesets/{id}`, PATCH `.../seeding`, GET `/hexons/tilesets`, GET `/hexons/storage` |
| Addressing | `node_address` on both REST and MCP |

The crate registry rows are N/A while the registry is dormant. Every other route has a
happy-path test on its own transport or through its MCP twin's shared core. Every scoped route
has its denial path covered by EO.
