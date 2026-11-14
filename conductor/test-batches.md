---
type: Reference
title: Usability-Region Test Batches
tags: [testing, batching, ci, usability-regions]
timestamp: 2026-11-01T00:00:00Z
---

# Usability-region test batches

**Rule (user directives 2026-11-01, 2026-11-07 and 2026-11-08):** tests run only when explicitly told (the user,
or a steer/task naming the batch); the pre-commit gate is `cargo check -p <touched crates>`; when told, gates
run only the batch(es) whose source paths were touched by a change, through the chunked runner
`tools\qa\test_batches.ps1 -Batches R1,R8` (one `cargo test` command at a time, that chunk's test executables
and PDBs pruned after it runs, a 60 GB free-disk floor and the build-gate check; `-ListOnly` prints the chunks;
rationale in `tools/qa/AGENTS.md` §test-batches). **Never `cargo test --workspace` on this machine:** on
2026-11-08 the sweep linked every test binary at once, left 258 GB of executables + PDBs in `target/debug/deps`
(plus 207 GB of incremental caches), filled the disk to 1.4 GB free, and died of commit exhaustion once the
system-managed pagefile could not grow. The campaign-end sweep is `-Batches All` (every batch except Full),
run once, when the user says so.

Inventory basis: `#[test]` + `#[tokio::test]` attributes across the workspace —
**4242 total** (3619 `#[test]` across 451 files + 623 `#[tokio::test]` across 92 files; cross-checked
against a `scrt` scan of the same pattern, which independently landed on the same 4242). Counts
below are per-attribute (≈ per-test-function) and are **approximate** — table-driven tests inside a
single `#[test]` fn count once here but may assert many cases.

Regions are organized around how a user actually encounters the app, not crate boundaries — several
crates are split across regions (fe-ui, fe-database, fe-api, fe-runtime, fractalengine), and several
crates map 1:1 onto a region (fe-terrain, fe-sync, fe-policy, fe-identity, fe-canonical-log).

Cargo note used throughout: the default libtest harness accepts **multiple FILTER substrings** after
`--`, OR'd together (`cargo test -p X -- foo:: bar::` runs anything whose fully-qualified test name
contains `foo::` OR `bar::`). `--test <stem>` runs exactly one `tests/<stem>.rs` integration binary.
A `dir/mod.rs` file's tests are named `dir::tests::…` — a `dir::mod` filter matches nothing (it silently
dropped 62 R1/R2 tests until 2026-11-07, QA-0142).

---

## R1 — Hierarchy & navigation

Sidebar tree, verse/fractal/petal/node CRUD, NavigationManager, visibility groups ("eyes").

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/navigation_manager.rs` | 6 |
| `fe-ui/src/verse_manager/{mod,spawn,residency_ledger,primitive_reconcile,primitive_materialize,node_index,lifecycle_events,db_results/mod,db_results/query,db_results/tokens}.rs` | 106 |
| `fe-ui/src/visibility/{mod,resolve,groups,cache}.rs` | 84 |
| `fe-ui/src/atlas/{petal_wizard,visibility_control}.rs` | 9 |
| `fe-ui/src/dialogs/create_entity.rs` | 1 |
| `fe-database/src/verse.rs`, `fe-database/src/handlers/crud.rs`, `fe-database/src/handlers/node_view.rs`, `fe-database/src/handlers/visibility_groups.rs` | 30 |
| `fe-database/tests/{node_lifecycle_test,node_view_test,node_data_write_readback_test,visibility_groups_test}.rs` | 22 |
| `fe-api/tests/visibility_groups_api_test.rs` | 4 |

**Total ≈ 262**

```bash
cargo test -p fe-ui --lib -- navigation_manager:: verse_manager::tests:: verse_manager::spawn verse_manager::residency_ledger verse_manager::primitive verse_manager::node_index verse_manager::lifecycle_events verse_manager::db_results::tests:: verse_manager::db_results::query verse_manager::db_results::tokens visibility:: atlas::petal_wizard atlas::visibility_control dialogs::create_entity
cargo test -p fe-database --lib -- verse:: handlers::crud:: handlers::node_view:: handlers::visibility_groups::
cargo test -p fe-database --test node_lifecycle_test --test node_view_test --test node_data_write_readback_test --test visibility_groups_test
cargo test -p fe-api --test visibility_groups_api_test
```

---

## R2 — Scene editing

Selection, gimbal, transforms, snapping, viewport camera, GLTF asset import, right-click/context menu.

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/node_manager/{mod,context_pick,viewport_pick,selection,shortcuts,sidebar_sync,inspector_sync,transform_broadcast,dispatch,router,pointer/mod,measure}.rs` | 143 |
| `fe-ui/src/gimbal.rs`, `fe-ui/src/geometry.rs` | 24 |
| `fe-ui/src/actions/{transform,node,node_props,dimension_callout,asset}.rs` | 35 |
| `fe-ui/src/dialogs/{gltf_import,context_menu,node_options}.rs` (`dialogs/mod.rs` tests are R6) | 29 |
| `fe-ui/src/panels/annotation_card.rs` | 12 |
| `fe-ui/src/node_manager/ground_snap.rs` (terrain-or-Y=0 placement, CITY-04) | new |
| `fe-ui/src/editor_batch/**` (ordered selection and shared editor routing, DATA-02) | new |
| `fe-database/src/handlers/editor_batch/**` (no tests at HEAD), `fe-database/src/handlers/stamp_transform.rs` (`euler_to_quat_xyzw` table, 1 test) | 1 |
| `fe-renderer/src/{camera,axis_gizmo,addressing,grid,instancing}.rs`, `fe-renderer/tests/renderer_test.rs` | 76 |
| `fractalengine/src/asset_bridge.rs` | 11 |
| `fe-runtime/src/scene_write.rs` (shared scene door: GUI and REST verdicts, `scene_write::tests::`; turn 3) | new |

**Total ≈ 331**

```bash
cargo test -p fe-ui --lib -- node_manager::tests:: node_manager::context_pick node_manager::viewport_pick node_manager::selection node_manager::shortcuts node_manager::sidebar_sync node_manager::inspector_sync node_manager::transform_broadcast node_manager::dispatch node_manager::router node_manager::pointer node_manager::measure gimbal:: geometry:: actions::transform actions::node:: actions::node_props actions::dimension_callout actions::asset dialogs::gltf_import dialogs::context_menu dialogs::node_options panels::annotation_card node_manager::ground_snap editor_batch::
# fe-database scene-write stamping (DATA-01, DATA-02, INT5-15): `handlers::editor_batch` has no test module at HEAD, so its entry reaches nothing yet
cargo test -p fe-database --lib -- handlers::editor_batch:: handlers::stamp_transform::
cargo test -p fe-renderer
cargo test -p fractalengine --bin fractalengine -- asset_bridge::
# GUI gate journey (turn 3): the shared scene door's verdict and stage journey; R9 door parity points here
cargo test -p fe-runtime --lib -- scene_write::tests::
```

---

## R3 — Inspector & query tab

Node properties, GIS query tab (raw/scoped query, egress string building), timeseries settings, the
shared query engine (fe-query), the entity-addressing substrate (fe-entity-store).

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/panels/{inspector,query_tab}.rs`, `fe-ui/src/actions/query.rs` | 10 |
| `fe-ui/src/panels/hierarchy_info.rs` (UI-06) | new |
| `fe-ui/src/gis/{mod,query,layers,egress_strings}.rs` | 73 |
| `fe-ui/src/timeseries_settings.rs` | 8 |
| `fe-database/tests/entity_property_test.rs`, `fe-database/tests/gis_data_test.rs` | 6 |
| `fe-query/src/**` (builder/, columnar/, geo/, duckdb_compat/, gis.rs, spatial_nodes.rs) | 152 |
| `fe-entity-store/src/{lib,address}.rs` | 41 |

**Total ≈ 290**

```bash
cargo test -p fe-ui --lib -- panels::inspector panels::query_tab actions::query gis:: panels::hierarchy_info
cargo test -p fe-database --lib -- entity_property   # if embedded; else:
cargo test -p fe-database --test entity_property_test --test gis_data_test
cargo test -p fe-query
cargo test -p fe-entity-store
```

---

## R4 — Terrain & maps (+ IoT/sensor sub-batch)

Petal map binding, tiles/streaming, sculpt/earthwork, terrain proposals, delta-viz, volume report,
ruler/measurement. **fe-terrain ALWAYS needs `--features render`** (bevy mesh types are feature-gated);
a few tile-fetch tests may additionally need `--features fetch` — add it if a run fails on
missing `reqwest`/`tokio` symbols.

| Source paths | Approx. count |
|---|---|
| `fe-terrain/**` (whole crate except nothing excluded) | 400 |
| `fe-ui/src/terrain_map/{mod,events}.rs`, `terrain_preview.rs`, `terrain_proposal_state.rs`, `sculpt_cursor.rs` | 26 |
| `fe-ui/src/node_manager/brush_interaction.rs` | 9 |
| `fe-ui/src/panels/{terrain_tools_panel,proposal_report_panel,volume_report_panel}.rs` | 26 |
| `fe-ui/src/panels/section_profile_panel.rs` (UI-05) | new |
| `fe-ui/src/actions/terrain_proposal.rs` | 52 |
| `fe-ui/src/verse_manager/db_results/terrain.rs` | 8 |
| `fe-database/src/handlers/{petal_terrain,petal_render_distance}.rs`, `fe-database/tests/petal_render_distance_test.rs` | 16 |
| `fe-api/src/terrain.rs`, `fe-api/tests/{terrain_mutation_test,petal_render_distance_api_test,hexon_tileset_test}.rs` | 18 |
| `fractalengine/src/terrain_bridge.rs` | 40 |
| `fe-renderer/src/{terrain_overlay,terrain_height}.rs` | 16 |

**Subtotal ≈ 611**

### IoT / sensor sub-batch (renders onto the map; separate filter, same region)

| Source paths | Approx. count |
|---|---|
| `extensions/iot-bridge/**` | 22 |
| `fe-sim/src/**` | 43 |
| `fe-database/tests/{device_enrolment_test,device_payload_test,iot_reading_test,observation_plane_test}.rs` | 60 |
| `fe-api/tests/{device_ingest_test,iot_ingest_test,sim_control_test}.rs`, `fe-api/src/sim.rs` | 23 |
| `fe-database/tests/db_test.rs` (J3 flush fn only: `test_rename_entity_omits_node_scene_change`, body at ~L785; turn 3 added the decode-provenance leg at ~L1240 (R-X-29) and the mixed-class all-or-nothing flush leg at ~L1325) | 1 |

**IoT subtotal ≈ 149** — **region total ≈ 760**

```bash
cargo test -p fe-terrain --features render
cargo test -p fe-ui --lib -- terrain_map:: terrain_preview terrain_proposal_state sculpt_cursor node_manager::brush_interaction panels::terrain_tools_panel panels::proposal_report_panel panels::volume_report_panel actions::terrain_proposal verse_manager::db_results::terrain actions::earthwork_volumes::tests:: panels::section_profile_panel
cargo test -p fe-database --lib -- handlers::petal_terrain handlers::petal_render_distance
cargo test -p fe-database --test petal_render_distance_test
cargo test -p fe-api --test terrain_mutation_test --test petal_render_distance_api_test --test hexon_tileset_test
cargo test -p fe-api --lib -- terrain::tests::
cargo test -p fractalengine --bin fractalengine -- terrain_bridge::
cargo test -p fe-renderer --lib -- terrain_overlay terrain_height

# IoT/sensor sub-batch
cargo test -p iot-bridge   # extensions/iot-bridge package name
cargo test -p fe-sim
cargo test -p fe-database --test device_enrolment_test --test device_payload_test --test iot_reading_test --test observation_plane_test
cargo test -p fe-api --test device_ingest_test --test iot_ingest_test --test sim_control_test
# J3 lives inside this fn (the name predates J3; filter by it, do not rename it here)
cargo test -p fe-database --test db_test -- test_rename_entity_omits_node_scene_change
```

---

## R5 — Paths & stamps

GPX import, pen tool, curve/handle editing, stamped-asset materialization.

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/node_manager/{path_handle_interaction,path_gimbal_drag,path_segment_interaction,path_point_interaction,curve}.rs` | 114 |
| `fe-ui/src/node_manager/road_{snap,chain,interaction,overlay}.rs` (road builder, CITY-15) | new |
| `fe-ui/src/actions/path.rs` | 35 |
| `fe-ui/src/panels/path_editor_card.rs` | 19 |
| `fe-ui/src/verse_manager/{path_asset_reconcile,path_asset_materialize}.rs` | 47 |
| `fe-database/tests/{gpx_path_persistence_test,gpx_pipeline_test}.rs` | 7 |
| `fe-api/src/gpx.rs` | 1 |
| `fractalengine/src/gpx_bridge.rs` | 36 |

**Total ≈ 259**

```bash
cargo test -p fe-ui --lib -- node_manager::path_handle_interaction node_manager::path_gimbal_drag node_manager::path_segment_interaction node_manager::path_point_interaction node_manager::curve actions::path panels::path_editor_card verse_manager::path_asset node_manager::road_snap node_manager::road_chain node_manager::road_interaction node_manager::road_overlay
cargo test -p fe-database --test gpx_path_persistence_test --test gpx_pipeline_test
cargo test -p fractalengine --bin fractalengine -- gpx_bridge::
cargo test -p fe-api --lib -- gpx::export_tree_tests::
```

---

## R6 — Notices / dialogs / shell

Toasts, dialogs, window chrome, status bar, embedded webview (PetalPortal), app settings.

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/notice.rs`, `fe-ui/src/ui_shell/{topbar,right_sidebar,modal,left_sidebar}.rs` | 43 |
| `fe-ui/src/dialogs/join.rs`, `fe-ui/src/panels/{widgets,toolbar,sidebar,mod,tool_options,tool_inspector,tool_panel}.rs` | 51 |
| `fe-ui/src/settings.rs`, `fe-ui/src/portal/mod.rs`, `fe-ui/src/plugin.rs` (shared wiring — see consolidation note) | 35 |
| `fe-webview/**` | 64 |
| `fractalengine/src/{panic_log,window_chrome}.rs` | 5 |
| `fe-ui/src/actions/mod.rs` (`actions::tests::` module: notice queue, active dialog, reveal, proposal select, path delete; superset of the UI-07 `active_dialog_entity_settings` journey) | 20 |
| `fe-ui/src/dialogs/peer_debug.rs` (network diagnostics panel) | new |
| `fe-ui/src/dialogs/mod.rs` (tests only: TreeAppearance layout at 1280x720 and 1024x640, QA-0112), `fe-ui/src/panels/mod.rs` (tests only), `fe-ui/src/native_accessibility/{navigation_ui,provider_identity}.rs` (`journey::` modules) | 3+ |

**Total ≈ 198**

```bash
cargo test -p fe-ui --lib -- notice:: ui_shell:: dialogs::join panels::widgets panels::toolbar panels::sidebar panels::tool_options panels::tool_inspector panels::tool_panel settings:: portal:: plugin:: actions::tests::active_dialog_entity_settings actions::tests:: dialogs::peer_debug dialogs::tests:: panels::tests:: native_accessibility::
cargo test -p fe-webview --lib --test security_test --test tauri_backend_test --test tauri_cutover_test
cargo test -p fe-webview --test portal_protocol_journey --test portal_ipc_journey
cargo test -p fractalengine --bin fractalengine -- portal_host::
cargo test -p fractalengine --bin fractalengine -- panic_log:: window_chrome::
```

Portal bundle journeys (`portal_surface_20261107`) have their own row: `fe-webview/tests/portal_protocol_journey.rs`
(custom protocol, CSP, navigation admission) and `fe-webview/tests/portal_ipc_journey.rs` (`fe.portal.v1` bridge).
The crate line above lists the other fe-webview targets explicitly, so the journeys run once. A `--lib` line covers
the unit tests; a new fe-webview test binary must be added to one of the two rows.

---

## R7 — Roles & sharing

RBAC, role propagation/grants, invites, claim ownership, admission, asset ownership.

| Source paths | Approx. count |
|---|---|
| `fe-database/src/{invite,scoped_invite,scope,rbac,role_manager,role_grants}.rs`, `fe-database/src/handlers/{invite,claim_orphaned_verse}.rs` | 42 |
| `fe-database/tests/role_propagation_{durable,api_cap,local}_test.rs` (non-flood variants) | 15 |
| `fe-database/tests/role_propagation_inbound_test.rs` **(SLOW — 606s flood test)** | 12 |
| `fe-policy/**` | 85 |
| `fe-api/tests/asset_ownership_test.rs` | 10 |
| `fe-database/src/handlers/extension_binding.rs`, `fe-database/tests/extension_binding_test.rs` (scoped extension bindings, Manager+ authority) | 1 |

**Total ≈ 165** (153 fast + 12 slow)

```bash
cargo test -p fe-database --lib -- invite:: scoped_invite scope:: rbac:: role_manager role_grants handlers::invite handlers::claim_orphaned_verse
cargo test -p fe-database --test role_propagation_durable_test --test role_propagation_api_cap_test --test role_propagation_local_test
cargo test -p fe-database --test extension_binding_test
cargo test -p fe-policy
cargo test -p fe-api --test asset_ownership_test

# SLOW — campaign-end only
cargo test -p fe-database --test role_propagation_inbound_test
```

---

## R8 — P2P sync & replication

fe-sync, distributed query, tileset transfer, relay, network thread.

| Source paths | Approx. count |
|---|---|
| `fe-sync/**` | 314 |
| `fe-api/tests/distributed_query_test.rs` | 12 |
| `fe-network/**` | 6 |
| `fractalengine-relay/**` | 9 |
| `fe-runtime/src/{peer_registry,distributed_query,blob_transport,blob_store,bevy_blob_reader}.rs` | 34 |
| `fe-runtime/src/{relay_advertisement,dialback,net_diagnostics}.rs` | new |
| `fractalengine/src/relay_serve_bridge.rs` (`--bin fractalengine`) | new |
| `fe-sync/tests/peer_relay_e2e_test.rs` (`relay-server` feature; slow) | new |
| `fe-ui/src/dialogs/join.rs` *(cross-listed — see R6)* | — |

**Total ≈ 375**

```bash
cargo test -p fe-sync -- --skip docs_engine::tests::a_hash_with_no_tileset_context_is_unaffected_by_the_filter
cargo test -p fe-sync --lib -- docs_engine::tests::a_hash_with_no_tileset_context_is_unaffected_by_the_filter
cargo test -p fe-api --test distributed_query_test
cargo test -p fe-network
cargo test -p fractalengine-relay
cargo test -p fe-runtime --lib -- peer_registry distributed_query blob_transport blob_store bevy_blob_reader relay_advertisement dialback net_diagnostics
cargo test -p fe-sync --features relay-server -- relay_server
cargo test -p fe-sync --features relay-server --test peer_relay_e2e_test
cargo test -p fractalengine-relay --features relay-server -- relay_serve
cargo test -p fractalengine --bin fractalengine -- relay_serve_bridge
```

**Binding and scene-asset legs (turn 3):** the all-authors binding wait lives in
`doc_replicator_replicates_row_over_real_transport` (`fe-sync/src/replicator.rs`, `wait_for_binding`); the scene-asset
source fallback is `scene_assets::transfer::tests::blob_source_serves_scene_bytes_only_when_exact`
(`fe-sync/src/scene_assets/transfer.rs`). Both run inside the `fe-sync` line above; no separate filter is needed.

**Docs-engine row (QA-0118):** the `fe-sync` line skips `docs_engine::tests::a_hash_with_no_tileset_context_is_unaffected_by_the_filter`,
and the next line runs it alone. That test carries the QA-0118 re-arm assertion (a refused scene fetch is retried once the
provider admits again) and the portal-bundle prefetch check. Its name predates QA-0118; renaming it is a separate change.

**Slow sub-batch:** `fe-sync/tests/relay_assisted_test.rs`, `fe-sync/tests/tileset_transfer_test.rs` and
`fe-sync/tests/peer_relay_e2e_test.rs` (feature-gated `relay-server`; real relay handshake, run via its own line above)
touch real endpoints/relay handshakes — flagged by the stress/timing scan; verify at campaign end if a
single run is slow, otherwise keep inline (not confirmed as slow as role_propagation_inbound).

---

## R9 — API/MCP & agent surfaces

REST, MCP tools, operator gate, screenshot/vision/input, egress/export, elevated query.

| Source paths | Approx. count |
|---|---|
| `fe-api/**` minus the files cross-listed into R4/R7/R8 (terrain_mutation_test, petal_render_distance_api_test, hexon_tileset_test, asset_ownership_test, distributed_query_test, device_ingest_test, iot_ingest_test, sim_control_test, visibility_groups_api_test) | ≈ 243 |
| `fe-runtime/src/usage.rs` (usage sink, `usage::tests::`) | new |
| `fe-api/src/{metrics,network,network_ops,relay_advertise}.rs` (module `tests::` only) | new |
| `fe-database/src/handlers/elevated_query.rs` | 5 |
| `fe-database/src/{api_token_store,session_cache}.rs`, `fe-database/src/handlers/api_token.rs` | 7 |
| `fe-runtime/src/vision.rs` | 5 |
| `fractalengine/src/{screenshot_bridge,vision_bridge}.rs` | 32 |
| `fractalengine/tests/security_harness.rs` | 5 |
| `fe-api/src/{bindings,bound_routes}.rs`, `fe-api/tests/{bindings_api_test,extensions_api_test}.rs` (already inside `fe-api/**`; listed for the batch filter) | 4 |
| `fe-api/src/{assets,import_hooks,input,portal_bundles,timeseries_query}.rs`, `fe-runtime/src/input.rs`, `fractalengine/src/input_bridge.rs` (module `tests::` only, INT5-15) | 6 |

**Total ≈ 297**

**Door parity (turn 3):** REST and MCP refusal of admin verbs and the door parity rows are `extensions_api_test`,
`portal_bundle_test` and `mcp_integration`, all in the fence below. The shared scene-door verdict journey
(`scene_write::tests::`) runs once under R2, not here.

```bash
cargo test -p fe-api --test api_integration --test api_harness_smoke --test mcp_integration --test screenshot_test --test vision_test --test vision_scope_test --test token_lifecycle_test --test namespace_capability_test --test existence_oracle_test --test record_id_read_test --test gis_test --test export_share_test --test share_signer_persistence_test --test analytics_scope_test --test portal_bundle_test --test ws_integration
cargo test -p fe-api --test bindings_api_test --test extensions_api_test
cargo test -p fe-api --lib -- auth:: crs:: bridge_client endpoint:: entity_store_bridge export:: operator_gate query_guard rest:: mcp:: share:: upload:: visibility:: vision:: ws:: network:: network_ops:: metrics:: relay_advertise:: assets::tests:: import_hooks::tests:: input::tests:: portal_bundles::tests:: timeseries_query::tests::
cargo test -p fe-database --lib -- handlers::elevated_query api_token_store session_cache handlers::api_token
cargo test -p fe-runtime --lib -- vision:: usage:: input::tests::
cargo test -p fractalengine --bin fractalengine -- screenshot_bridge vision_bridge input_bridge::
cargo test -p fractalengine --test security_harness
```

---

## R10 — Identity & persistence

fe-identity, migrations, reconcile, generic DB handlers, canonical data-log (fe-canonical-log).

| Source paths | Approx. count |
|---|---|
| `fe-identity/**` | 64 |
| `fe-canonical-log/**` | 514 |
| `fe-database/src/field_clock/**`, `fe-database/src/handlers/{replicated_row,scene_publication/**}` (CLOCK-03; reached by no batch before this round) | new |
| `fe-database/tests/petal_snapshot_test.rs` (HEXON-09) | new |
| `fe-database/src/{op_log,repo,merge,reconcile,space_manager,model_url_meta,replication_mode,atlas,lib}.rs` | 63 |
| `fe-database/src/handlers/{blob_migration,asset_gc,asset_backfill,inbound_admissions,ts_partial,verse_settings}.rs` | 27 |
| `fe-database/src/handlers/crate_registry.rs` (per-petal install and uninstall, HEXON5-01), `fe-database/src/lib.rs` test modules (`namespace_id_tests`, `join_verse_dispatch_tests`, `subdir_tests`, `migration_tests`) | new |
| `fe-database/src/migration/**`, `fe-database/src/schema.rs` | 40 |
| `fe-database/src/canon_log/**` | 25 |
| `fe-database/tests/{db_test,shared_handle_test,schema_idempotent,migration_conformance_test,migration_equivalence_harness,replication_backpressure_test,canon_log_replay_payload_facts_test,canon_log_composed_admission_test,canon_log_materialization_test,dcl20_bypass_counter_test}.rs` | 69 |

**Total ≈ 802**

```bash
cargo test -p fe-identity
cargo test -p fe-canonical-log
cargo test -p fe-database --lib -- op_log:: repo:: merge:: reconcile:: space_manager model_url_meta replication_mode atlas:: handlers::blob_migration handlers::asset_gc handlers::asset_backfill handlers::inbound_admissions handlers::ts_partial handlers::verse_settings migration:: schema:: canon_log:: field_clock:: handlers::replicated_row:: handlers::scene_publication:: handlers::crate_registry:: namespace_id_tests:: join_verse_dispatch_tests:: subdir_tests:: migration_tests::
cargo test -p fe-database --test db_test --test shared_handle_test --test schema_idempotent --test migration_conformance_test --test migration_equivalence_harness --test replication_backpressure_test --test canon_log_replay_payload_facts_test --test canon_log_composed_admission_test --test canon_log_materialization_test --test dcl20_bypass_counter_test --test petal_snapshot_test
```

**Possible-slow candidates (flagged, not confirmed):** `schema_idempotent.rs`, `migration_equivalence_harness.rs`,
`db_test.rs` matched the restart/stress scan — these restart real SurrealKV stores. Time them once;
move to the slow batch if any exceeds ~30s.

---

## R11 — Plugin / hexon / format

fe-plugin, fe-plugin-test, fe-hexon, fe-hexon-registry, fe-format, fe-sdk, Atlas (hexon browser) UI.

| Source paths | Approx. count |
|---|---|
| `fe-plugin/**` | 58 |
| `fe-plugin-test/**` | 34 |
| `fe-hexon/**` | 79 |
| `fe-hexon-registry/**` | 23 |
| `fe-format/**` | 48 |
| `fe-sdk/**` | 46 |
| `fe-ui/src/atlas/{dashboard,model_editor,search_bar,tag_panel,room_editor}.rs` | 23 |
| `fe-ui/src/actions/hexon.rs` | 1 |
| `fe-ui/src/extensions/**` (consent, slots, panel, builtin, bindings) | 7 |
| `fe-runtime/src/{extension_binding,scene_hook_gate}.rs` | 7 |
| `tools/fe-ext/**` (host CLI in the workspace; `guest/` is excluded and builds standalone) | 5 |
| `extensions/{bi-push,device-adapter,fleet-sim}/**` (standalone crates, own `[workspace]`) | 4 |

**Total ≈ 335**

```bash
cargo test -p fe-plugin -- --skip admin::tests:: --skip registry_client::tests::
cargo test -p fe-plugin-test
cargo test -p fe-hexon -- --skip entitlement_client::tests:: --skip entitlement_gate::tests::
cargo test -p fe-format -- --skip portal_bundle::
cargo test -p fe-format --lib -- portal_bundle::
cargo test -p fe-sdk
cargo test -p fe-runtime --lib -- extension_binding::tests:: scene_hook_gate::tests::
cargo test -p fe-ui --lib -- atlas::dashboard atlas::model_editor atlas::search_bar atlas::tag_panel atlas::room_editor actions::hexon
cargo test -p fe-ui --lib -- extensions::
cargo test -p fe-plugin -- net::tests:: bindings::resolver::tests:: tick::tests::tick_journey_skips_counts_and_degrades_without_flushing
cargo test -p fractalengine --bin fractalengine -- --skip extension_host::admin_channel::tests:: extension_host:: plugin_bridge::
cargo test -p fractalengine --bin fractalengine -- extension_ui_tests::
cargo test -p fe-api --lib -- server::extension_mount_tests::
cargo test -p fe-ui --lib -- panels::plugin_install_panel
```

**Reach lines (turn 3 gate):** `extension_ui_tests::` (first-party extension UI registration in `fractalengine/src/main.rs`),
`server::extension_mount_tests::` (fe-api extension route mount in `fe-api/src/server.rs`) and
`panels::plugin_install_panel` (the superseded v1 consent modal's journey) matched no other batch filter.

**Portal bundle validator (fe-format):** `fe-format/src/portal_bundle.rs` tests (`portal_bundle::tests::`) run in the second line,
and the first line skips them so they run once. Their coverage: validator refusal classes, capability digest, and the capability
increase rule.

**Partition rule (turn 3):** the three sub-batches below own the admin, store and entitlement tests. The lines above
`--skip` those module paths so every test runs exactly once when R11 runs. Keep each new skip in step with its
sub-batch filter, or a test runs twice (or never).

### R11-ADMIN — local admin channel (GUI and `fe-ext admin` only, never REST or MCP)

| Source paths | Note |
|---|---|
| `fe-plugin/src/admin/**` (verb table, frame cap, OS-user pipe or socket transport, channel journey) | new in turn 3; `admin::tests::` |
| `fractalengine/src/extension_host/admin_channel.rs` | new; listener admission, consent relay, busy and shutdown journey |
| `tools/fe-ext/src/admin.rs` | new; `fe-ext admin` argument mapping, install-file path, consent stream |

```bash
cargo test -p fe-plugin -- admin::tests::
cargo test -p fe-ext --lib -- admin::tests::
cargo test -p fractalengine --bin fractalengine -- extension_host::admin_channel::tests::
```

### R11-STORE — public store client, registry service, static index, examples

| Source paths | Note |
|---|---|
| `fe-plugin/src/registry_client.rs` | registry host address policy (no proxy, same-origin redirects, https only) |
| `fe-hexon/src/registry_source.rs` | store client rules table; needs the `remote` feature |
| `fe-hexon-registry/**` | catalogue public and verbatim, the rest behind a token (`tests/registry_test.rs`) |
| `fe-hexon/tests/remote_registry_test.rs` | real fe-hexon-registry round trip incl. the AC-6 sealed plugin install (feature `remote`; EXT-04, HEXON-03) |
| `tools/fe-ext/src/index.rs`, `tools/fe-ext/tests/j8_cli.rs` | index build then verify journey (`index::tests::`); the `j8` legs include the index legs |
| `tools/fe-ext/guest/**`, `extensions/{fleet-sim,device-adapter,bi-push}/**` | standalone crates, own `[workspace]`; run via `--manifest-path` |

```bash
cargo test -p fe-plugin -- registry_client::tests::
cargo test -p fe-hexon --features remote -- registry_source::tests::
cargo test -p fe-hexon --features remote --test remote_registry_test
cargo test -p fe-hexon-registry
cargo test -p fe-ext -- j8
cargo test -p fe-ext --lib -- index::tests::
cargo test --manifest-path tools/fe-ext/guest/Cargo.toml
cargo test --manifest-path extensions/fleet-sim/Cargo.toml
cargo test --manifest-path extensions/device-adapter/Cargo.toml
cargo test --manifest-path extensions/bi-push/Cargo.toml
```

### R11-ENT — entitlements (licence tokens, key wrap, paid-entry gate, token client)

| Source paths | Note |
|---|---|
| `fe-format/src/entitlement.rs` | EdDSA token round-trip, every forged token refused, token-request proof; needs feature `entitlement` |
| `fe-hexon/src/entitlement_client.rs`, `fe-hexon/src/entitlement_gate.rs` | token request and renewal, contract refusal codes, licensed package gate (`fe-hexon` always enables the feature) |
| `fractalengine/src/extension_host/{licence_client,governance}.rs` | licence token client and row 6 under feature `entitlement` (EXT-01; module gated at `extension_host/mod.rs:12`, so the R11 line does not reach it) |

```bash
cargo test -p fe-format --features entitlement --lib -- entitlement::tests::
cargo test -p fe-hexon -- entitlement_client::tests:: entitlement_gate::tests::
cargo test -p fractalengine --bin fractalengine --features entitlement -- extension_host::governance::tests::
```

**Runner note:** `tools/qa/test_batches.ps1` assigns every `cargo test` line under `## R11` to R11, so `-Batches R11`
runs all four blocks above. A standalone `-Batches R11-ADMIN` needs the runner to read `### (R\d+-[A-Za-z]+)` headings
(a NEEDS for `tools/qa/test_batches.ps1`, not this file). Until then, run one sub-batch by pasting its lines.

## Portal QA lane (not cargo)

The portal QA journeys PJ1-PJ10 (`tools/qa/portal/journeys.md`) run as sim scenarios (`tools/qa/sim/scenarios/portal-core.json`,
`portal-replication.json`, `portal-load.json`), not as `cargo test`, so this section has no runner chunks. PJ10 (static export, R17)
is registered for P6 and runs only once P6 lands its export route in `fe-api/src/portal_bundles.rs`; it extends
`fe-api/tests/portal_bundle_test.rs`, which the R9 row already runs.

**Kill switch (P-KILL):** `portal-core.json` calls the `portal_killswitch` op twice (engage, then disengage; the
op posts `POST /api/v1/debug/portal/killswitch {engage}`, routed in `fe-api/src/server.rs` and handled by
`fe-api/src/qa_portal.rs`). Structural check: `python tools/qa/sim/run.py validate portal-core`. The live
engage/disengage run is a sim run (flags in `tools/qa/AGENTS.md` §sim), not a cargo chunk.

**Extension sim lane (not cargo):** `ext-fleet`, `ext-device`, `ext-bindings` and `ext-install-gui` drive administration
through the GUI surface or the `fe-ext admin` channel, never REST (user decision 2026-11-09). Structural check:
`python tools/qa/sim/run.py validate ext-fleet ext-device ext-bindings ext-install-gui fleet-terrain`.
User-named sim batch `fleet-terrain`: `python tools/qa/sim/run.py fleet-terrain` (QA Terrain fixture; run only when the user names it). The GUI QA path (install from
file and from the static index, consent, enable, disable, update with a delta, uninstall, plus one `fe-ext admin` leg)
is `ext-install-gui.json`, run as a sim scenario.

---

## Harness (own batch — always slow, campaign-end or on-demand only)

fe-test-harness two-peer scenarios spin up real sync threads + real SurrealKV + real network
handshakes. Never run inline with a feature change; run when the change touches sync/db/api wiring
directly, or at campaign end.

| Source paths | Approx. count |
|---|---|
| `fe-test-harness/src/scenarios/*.rs` (api_db_sync_cross_thread, distributed_query, role_propagation, scaffold, two_peer_tileset_stream, api_token_flow, invite_flow, two_peer_shard_fabric, two_peer_tombstone_sync, two_peer_timeseries_sync, two_peer_device_payload_sync, two_peer_replica_sync, verse_sync, two_peer_verse_join, two_peer_sync_pipeline, two_peer_blob_exchange, blob_roundtrip) | — |
| `fe-test-harness/tests/{integration,blob_store_cross_read}.rs`, `fe-test-harness/src/fixtures/mod.rs`, `fe-test-harness/src/peer.rs` | 30 |
| `fractalengine/tests/integration_test.rs` | 2 |

```bash
cargo test -p fractalengine-test-harness   # package name per Cargo.toml, binary dir fe-test-harness
cargo test -p fractalengine --test integration_test
```

---

## Misc batch (cross-cutting infra — no single owning region)

Shared message/bridge types consumed by several regions. Run when the change is to the shared type
itself (not a specific feature); otherwise the owning region's batch already exercises it indirectly.

| Source paths | Approx. count |
|---|---|
| `fe-runtime/src/{messages,wiring,app,shared_node,diag15m,ktx2_guard,safe_id,asset_gc,bridge,timeseries}.rs`, `fe-runtime/tests/shared_node_test.rs` | 93 |
| `fe-runtime/src/log_format.rs` (`log_format::tests::`) | new |
| `fe-ui/src/plugin.rs` *(see Consolidation candidates — recommend splitting by owning resource)* | 19 |

```bash
cargo test -p fe-runtime --lib -- messages:: wiring:: app:: shared_node diag15m ktx2_guard safe_id asset_gc bridge:: timeseries:: log_format::
cargo test -p fe-runtime --test shared_node_test
cargo test -p fe-ui --lib -- plugin::
```

Non-cargo check (not a runner chunk; the extension docs link check, EXT-08): `python tools/qa/docs_link_check.py`

---

## Files touched → batches (lookup)

| Path prefix | Batch(es) |
|---|---|
| `fe-ui/src/navigation_manager.rs`, `fe-ui/src/verse_manager/` (excl. `db_results/terrain.rs`, `path_asset_*`) | R1 |
| `fe-ui/src/verse_manager/db_results/terrain.rs` | R4 |
| `fe-ui/src/verse_manager/path_asset_*.rs` | R5 |
| `fe-ui/src/visibility/` | R1 |
| `fe-ui/src/node_manager/` (excl. `brush_interaction.rs`, `path_*`, `road_*`) | R2 |
| `fe-ui/src/node_manager/road_{snap,chain,interaction,overlay}.rs` | R5 |
| `fe-ui/src/node_manager/ground_snap.rs` | R2 |
| `fe-ui/src/editor_batch/**` | R2 |
| `fe-database/src/handlers/{editor_batch/**,stamp_transform.rs}` | R2 |
| `fe-ui/src/panels/hierarchy_info.rs` | R3 |
| `fe-ui/src/panels/section_profile_panel.rs` | R4 |
| `fe-ui/src/actions/mod.rs` (`actions::tests::` module) | R6 |
| `fractalengine/src/extension_host/{licence_client,governance}.rs` | R11-ENT |
| `fe-hexon/tests/remote_registry_test.rs` | R11-STORE |
| `fe-database/src/field_clock/**`, `fe-database/src/handlers/{replicated_row,scene_publication/**}`, `fe-database/tests/petal_snapshot_test.rs` | R10 |
| `fe-ui/src/node_manager/brush_interaction.rs` | R4 |
| `fe-ui/src/node_manager/path_*.rs`, `fe-ui/src/node_manager/curve.rs` | R5 |
| `fe-ui/src/gimbal.rs`, `fe-ui/src/geometry.rs` | R2 |
| `fe-ui/src/actions/` (excl. `terrain_proposal.rs`, `path.rs`, `query.rs`, `hexon.rs`) | R2 |
| `fe-ui/src/actions/terrain_proposal.rs` | R4 |
| `fe-ui/src/actions/path.rs` | R5 |
| `fe-ui/src/actions/query.rs` | R3 |
| `fe-ui/src/actions/hexon.rs` | R11 |
| `fe-ui/src/extensions/**` | R11 |
| `fe-plugin/src/admin/**`, `fractalengine/src/extension_host/admin_channel.rs`, `tools/fe-ext/src/admin.rs` | R11-ADMIN |
| `fe-plugin/src/registry_client.rs`, `fe-hexon/src/registry_source.rs`, `fe-hexon-registry/**`, `tools/fe-ext/src/index.rs` | R11-STORE |
| `fe-format/src/entitlement.rs`, `fe-hexon/src/entitlement_client.rs`, `fe-hexon/src/entitlement_gate.rs` | R11-ENT |
| `fe-runtime/src/scene_write.rs` | R2 (shared scene door; R9 points here) |
| `fe-sync/src/scene_assets/**` | R8 |
| `fe-database/src/handlers/{plugin_host,iot_reading}.rs` | R4 (IoT; no unit tests, exercised by the J3 fn and the device journeys) |
| `tools/qa/sim/scenarios/{portal-core,ext-fleet,ext-device,ext-bindings,ext-install-gui}.json` | sim lanes (Portal QA lane, not cargo) |
| `fe-ui/src/dialogs/` (excl. `join.rs`) | R2/R6 (split by file — see R2, R6 tables) |
| `fe-ui/src/dialogs/join.rs` | R6, R8 |
| `fe-ui/src/panels/` (split per R2/R3/R4/R5/R6 tables) | see region tables |
| `fe-ui/src/gis/`, `timeseries_settings.rs` | R3 |
| `fe-ui/src/terrain_map/`, `terrain_preview.rs`, `terrain_proposal_state.rs`, `sculpt_cursor.rs` | R4 |
| `fe-ui/src/atlas/petal_wizard.rs`, `visibility_control.rs` | R1 |
| `fe-ui/src/atlas/` (rest) | R11 |
| `fe-ui/src/notice.rs`, `ui_shell/`, `settings.rs`, `portal/`, `plugin.rs` | R6 (plugin.rs also Misc) |
| `fe-database/src/handlers/{petal_terrain,petal_render_distance}.rs` | R4 |
| `fe-database/src/handlers/{visibility_groups,crud,node_view}.rs` | R1 |
| `fe-database/src/handlers/{invite,claim_orphaned_verse}.rs` | R7 |
| `fe-database/src/handlers/extension_binding.rs`, `fe-database/tests/extension_binding_test.rs` | R7 |
| `fe-database/src/handlers/elevated_query.rs` | R9 |
| `fe-database/src/handlers/{api_token}.rs`, `api_token_store.rs`, `session_cache.rs` | R9 |
| `fe-database/src/handlers/` (rest: blob_migration, asset_gc, asset_backfill, inbound_admissions, ts_partial, verse_settings, crate_registry) | R10 |
| `fe-database/src/{invite,scoped_invite,scope,rbac,role_manager,role_grants}.rs` | R7 |
| `fe-database/src/canon_log/`, `migration/`, `schema.rs`, `op_log.rs`, `repo.rs`, `merge.rs`, `reconcile.rs`, `space_manager.rs`, `model_url_meta.rs`, `replication_mode.rs`, `atlas.rs`, `lib.rs`, `verse.rs` | R10 (`verse.rs` → R1) |
| `fe-database/tests/role_propagation_inbound_test.rs` | R7 **SLOW** |
| `fe-database/tests/role_propagation_{durable,api_cap,local}_test.rs` | R7 |
| `fe-database/tests/{gpx_path_persistence_test,gpx_pipeline_test}.rs` | R5 |
| `fe-database/tests/{entity_property_test,gis_data_test}.rs` | R3 |
| `fe-database/tests/{device_enrolment_test,device_payload_test,iot_reading_test,observation_plane_test}.rs` | R4 (IoT) |
| `fe-database/tests/{petal_render_distance_test,visibility_groups_test,node_lifecycle_test,node_view_test,node_data_write_readback_test}.rs` | R4 / R1 (see R1, R4 tables) |
| `fe-database/tests/db_test.rs` (J3 IoT flush fn only) | R4 (IoT) |
| `fe-database/tests/` (rest: db_test, shared_handle_test, schema_idempotent, migration_*, replication_backpressure_test, canon_log_*_test, dcl20_bypass_counter_test) | R10 |
| `fe-terrain/**` | R4 (always `--features render`) |
| `fe-api/src/portal_bundles.rs`, `fe-api/src/qa_portal.rs`, `fe-api/tests/portal_bundle_test.rs` | R9 |
| `fe-api/**` | R9, except files cross-listed into R4/R7/R8 (see R9 table header) |
| `fe-api/src/{bindings,bound_routes}.rs`, `fe-api/tests/{bindings_api_test,extensions_api_test}.rs` | R9 |
| `fe-sync/**` | R8 |
| `fe-network/**`, `fractalengine-relay/**` | R8 |
| `fe-runtime/src/{peer_registry,distributed_query,blob_transport,blob_store,bevy_blob_reader}.rs` | R8 |
| `fe-runtime/src/vision.rs` | R9 |
| `fe-runtime/src/{extension_binding,scene_hook_gate}.rs` | R11 |
| `fe-runtime/` (rest) | Misc |
| `fe-identity/**`, `fe-canonical-log/**` | R10 |
| `fe-policy/**` | R7 |
| `fe-plugin/**`, `fe-plugin-test/**`, `fe-hexon/**`, `fe-hexon-registry/**`, `fe-format/**`, `fe-sdk/**` | R11 |
| `fe-renderer/**` (excl. terrain_overlay, terrain_height) | R2 |
| `fe-renderer/src/{terrain_overlay,terrain_height}.rs` | R4 |
| `fe-webview/**` | R6 |
| `extensions/iot-bridge/**`, `fe-sim/**` | R4 (IoT) |
| `extensions/{bi-push,device-adapter,fleet-sim}/**`, `tools/fe-ext/**` | R11 |
| `fractalengine/src/{asset_bridge}.rs` | R2 |
| `fractalengine/src/gpx_bridge.rs` | R5 |
| `fractalengine/src/terrain_bridge.rs` | R4 |
| `fractalengine/src/{plugin_bridge.rs,extension_host/**}` | R11 |
| `fractalengine/src/{screenshot_bridge,vision_bridge}.rs` | R9 |
| `fractalengine/src/{panic_log,window_chrome}.rs` | R6 |
| `fractalengine/src/portal_host/**` | R6 |
| `fractalengine/tests/security_harness.rs` | R9 |
| `fractalengine/tests/integration_test.rs` | Harness |
| `fe-test-harness/**` | Harness (always slow) |
| `fe-runtime/src/usage.rs` | R9 |
| `fe-runtime/src/log_format.rs` | Misc |
| `fe-runtime/src/{relay_advertisement,dialback,net_diagnostics}.rs` | R8 |
| `fe-api/src/{metrics,network,network_ops,relay_advertise}.rs` | R9 |
| `fe-sync/tests/peer_relay_e2e_test.rs` | R8 (slow sub-batch) |
| `fractalengine/src/relay_serve_bridge.rs` | R8 |
| `fe-ui/src/dialogs/peer_debug.rs` | R6 |
| `fe-entity-store/**`, `fe-query/**` | R3 |

---

## Consolidation candidates (swap targets)

Not performed here — named for a future merge pass into longer journey tests, per user directive
("tests as swaps or extending workflows… longer test functions are better").

1. **`fe-canonical-log/src/crypto/key_wrap.rs` (32 tests) + `src/cbor.rs` (32 tests) + `src/crypto/aead.rs` (16) +
   `src/envelope.rs` (20)** — four files of small same-shape round-trip/property tests over wire
   primitives (encode→decode→assert-equal variants per type). Strong candidate to collapse into one
   `wire_roundtrip_journey` table-driven test per module, keyed by payload variant.
2. **`fe-sync/src/sync_thread.rs` (59 across the file incl. module tests) + `src/messages.rs` (24) +
   `src/distributed_query.rs` (36) + `src/sharding.rs` (35) + `src/relay_config.rs` (26)** — sync-thread
   unit tests are almost all "construct message, send, assert received/transformed" shape; a single
   `sync_thread_message_flow` journey test parametrized over message variant would replace most of these.
3. **`fe-database/src/handlers/replicated_row.rs` (35 tests)** — one-assertion-per-field-combination
   style; table-driven collapse candidate (field × op-type matrix instead of one fn per cell).
4. **`fe-ui/src/node_manager/path_point_interaction.rs` (35) + `path_segment_interaction.rs` (19) +
   `path_handle_interaction.rs` (18) + `path_gimbal_drag.rs` (8)** — four files of pen-tool interaction
   tests with near-identical "simulate drag, assert handle moved" shape; a single `pen_tool_journey`
   test walking through create→drag-point→drag-segment→drag-handle→commit would cover the same ground
   in far fewer, longer functions, matching the "run as one workflow instead of many tiny swaps" ask.
5. **`fe-api/tests/mcp_integration.rs` (31) + `api_integration.rs` (14)** — both already integration-style;
   evaluate whether MCP-tool-by-tool tests can fold into one longer "agent session" journey test that
   calls several tools in sequence against one harness instance (amortizes harness startup cost, which
   is the actual expense, not assertion count).
6. **`fe-ui/src/plugin.rs` (19 tests spanning unrelated resources: InspectorFormState, PetalMapState,
   SidebarState, etc.)** — this is a file-organization issue, not a test-count issue: the tests are
   already small, but they're scattered across one god-file that straddles R1/R3/R4/R6/Misc. Splitting
   the resources into their owning modules (not merging tests) would let a batch gate target this file
   precisely instead of pulling in Misc on every unrelated change — flagged here because it's the
   underlying reason R6/Misc have a catch-all entry for `plugin.rs` rather than a clean region.
7. **`fe-database/src/schema.rs` (36 tests)** — per-table schema-shape assertions; candidate for one
   parametrized "schema conforms to spec" test over a table of (table_name, expected_columns) pairs.

---

## Slow sub-batches (campaign-end only)

```bash
# Confirmed slow (user-named, ~606s)
cargo test -p fe-database --test role_propagation_inbound_test

# Flagged by stress/restart scan — time once, promote here if >30s
cargo test -p fe-database --test schema_idempotent --test migration_equivalence_harness --test db_test
cargo test -p fe-sync --test relay_assisted_test --test tileset_transfer_test

# Harness scenarios — always slow (real two-peer sync + real SurrealKV)
cargo test -p fractalengine-test-harness
```

---

## Full-suite command (campaign end only)

Retired 2026-11-08: `cargo test --workspace` links every test binary at once and filled the disk (see the rule
at the top). The campaign-end sweep is the same tests one chunk at a time, link outputs pruned after each:

```powershell
tools\qa\test_batches.ps1 -Batches All      # R1..R11 + Harness + Misc + Slow, sequential
tools\qa\test_batches.ps1 -Batches R4       # `cargo test -p fe-terrain --features render` is R4's first chunk
```
