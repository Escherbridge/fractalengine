---
type: Reference
title: Usability-Region Test Batches
tags: [testing, batching, ci, usability-regions]
timestamp: 2026-11-01T00:00:00Z
---

# Usability-region test batches

**Rule (user directive 2026-11-01):** gates run only the batch(es) whose source paths were
touched by a change. The full workspace suite (`cargo test --workspace`, plus
`cargo test -p fe-terrain --features render`) runs exactly once, at campaign end.

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
| `fe-ui/src/dialogs/{gltf_import,context_menu,node_options,mod}.rs` | 30 |
| `fe-ui/src/panels/annotation_card.rs` | 12 |
| `fe-renderer/src/{camera,axis_gizmo,addressing,grid,instancing}.rs`, `fe-renderer/tests/renderer_test.rs` | 76 |
| `fractalengine/src/asset_bridge.rs` | 11 |

**Total ≈ 331**

```bash
cargo test -p fe-ui --lib -- node_manager::tests:: node_manager::context_pick node_manager::viewport_pick node_manager::selection node_manager::shortcuts node_manager::sidebar_sync node_manager::inspector_sync node_manager::transform_broadcast node_manager::dispatch node_manager::router node_manager::pointer node_manager::measure gimbal:: geometry:: actions::transform actions::node:: actions::node_props actions::dimension_callout actions::asset dialogs::gltf_import dialogs::context_menu dialogs::node_options panels::annotation_card
cargo test -p fe-renderer
cargo test -p fractalengine --bin fractalengine -- asset_bridge::
```

---

## R3 — Inspector & query tab

Node properties, GIS query tab (raw/scoped query, egress string building), timeseries settings, the
shared query engine (fe-query), the entity-addressing substrate (fe-entity-store).

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/panels/{inspector,query_tab}.rs`, `fe-ui/src/actions/query.rs` | 10 |
| `fe-ui/src/gis/{mod,query,layers,egress_strings}.rs` | 73 |
| `fe-ui/src/timeseries_settings.rs` | 8 |
| `fe-database/tests/entity_property_test.rs`, `fe-database/tests/gis_data_test.rs` | 6 |
| `fe-query/src/**` (builder/, columnar/, geo/, duckdb_compat/, gis.rs, spatial_nodes.rs) | 152 |
| `fe-entity-store/src/{lib,address}.rs` | 41 |

**Total ≈ 290**

```bash
cargo test -p fe-ui --lib -- panels::inspector panels::query_tab actions::query gis::
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

**IoT subtotal ≈ 148** — **region total ≈ 759**

```bash
cargo test -p fe-terrain --features render
cargo test -p fe-ui --lib -- terrain_map:: terrain_preview terrain_proposal_state sculpt_cursor node_manager::brush_interaction panels::terrain_tools_panel panels::proposal_report_panel panels::volume_report_panel actions::terrain_proposal verse_manager::db_results::terrain
cargo test -p fe-database --lib -- handlers::petal_terrain handlers::petal_render_distance
cargo test -p fe-database --test petal_render_distance_test
cargo test -p fe-api --test terrain_mutation_test --test petal_render_distance_api_test --test hexon_tileset_test
cargo test -p fractalengine --bin fractalengine -- terrain_bridge::
cargo test -p fe-renderer --lib -- terrain_overlay terrain_height

# IoT/sensor sub-batch
cargo test -p iot-bridge   # extensions/iot-bridge package name
cargo test -p fe-sim
cargo test -p fe-database --test device_enrolment_test --test device_payload_test --test iot_reading_test --test observation_plane_test
cargo test -p fe-api --test device_ingest_test --test iot_ingest_test --test sim_control_test
```

---

## R5 — Paths & stamps

GPX import, pen tool, curve/handle editing, stamped-asset materialization.

| Source paths | Approx. count |
|---|---|
| `fe-ui/src/node_manager/{path_handle_interaction,path_gimbal_drag,path_segment_interaction,path_point_interaction,curve}.rs` | 114 |
| `fe-ui/src/actions/path.rs` | 35 |
| `fe-ui/src/panels/path_editor_card.rs` | 19 |
| `fe-ui/src/verse_manager/{path_asset_reconcile,path_asset_materialize}.rs` | 47 |
| `fe-database/tests/{gpx_path_persistence_test,gpx_pipeline_test}.rs` | 7 |
| `fe-api/src/gpx.rs` | 1 |
| `fractalengine/src/gpx_bridge.rs` | 36 |

**Total ≈ 259**

```bash
cargo test -p fe-ui --lib -- node_manager::path_handle_interaction node_manager::path_gimbal_drag node_manager::path_segment_interaction node_manager::path_point_interaction node_manager::curve actions::path panels::path_editor_card verse_manager::path_asset
cargo test -p fe-database --test gpx_path_persistence_test --test gpx_pipeline_test
cargo test -p fractalengine --bin fractalengine -- gpx_bridge::
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

**Total ≈ 198**

```bash
cargo test -p fe-ui --lib -- notice:: ui_shell:: dialogs::join panels::widgets panels::toolbar panels::sidebar panels::tool_options panels::tool_inspector panels::tool_panel settings:: portal:: plugin::
cargo test -p fe-webview
cargo test -p fractalengine --bin fractalengine -- panic_log:: window_chrome::
```

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

**Total ≈ 164** (152 fast + 12 slow)

```bash
cargo test -p fe-database --lib -- invite:: scoped_invite scope:: rbac:: role_manager role_grants handlers::invite handlers::claim_orphaned_verse
cargo test -p fe-database --test role_propagation_durable_test --test role_propagation_api_cap_test --test role_propagation_local_test
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
| `fe-ui/src/dialogs/join.rs` *(cross-listed — see R6)* | — |

**Total ≈ 375**

```bash
cargo test -p fe-sync
cargo test -p fe-api --test distributed_query_test
cargo test -p fe-network
cargo test -p fractalengine-relay
cargo test -p fe-runtime --lib -- peer_registry distributed_query blob_transport blob_store bevy_blob_reader
```

**Slow sub-batch:** `fe-sync/tests/relay_assisted_test.rs` and `fe-sync/tests/tileset_transfer_test.rs`
touch real endpoints/relay handshakes — flagged by the stress/timing scan; verify at campaign end if a
single run is slow, otherwise keep inline (not confirmed as slow as role_propagation_inbound).

---

## R9 — API/MCP & agent surfaces

REST, MCP tools, operator gate, screenshot/vision/input, egress/export, elevated query.

| Source paths | Approx. count |
|---|---|
| `fe-api/**` minus the files cross-listed into R4/R7/R8 (terrain_mutation_test, petal_render_distance_api_test, hexon_tileset_test, asset_ownership_test, distributed_query_test, device_ingest_test, iot_ingest_test, sim_control_test, visibility_groups_api_test) | ≈ 243 |
| `fe-database/src/handlers/elevated_query.rs` | 5 |
| `fe-database/src/{api_token_store,session_cache}.rs`, `fe-database/src/handlers/api_token.rs` | 7 |
| `fe-runtime/src/vision.rs` | 5 |
| `fractalengine/src/{screenshot_bridge,vision_bridge}.rs` | 32 |
| `fractalengine/tests/security_harness.rs` | 5 |

**Total ≈ 297**

```bash
cargo test -p fe-api --test api_integration --test api_harness_smoke --test mcp_integration --test screenshot_test --test vision_test --test vision_scope_test --test token_lifecycle_test --test namespace_capability_test --test existence_oracle_test --test record_id_read_test --test gis_test --test export_share_test --test share_signer_persistence_test --test analytics_scope_test --test ws_integration
cargo test -p fe-api --lib -- auth:: crs:: bridge_client endpoint:: entity_store_bridge export:: operator_gate query_guard rest:: mcp:: share:: upload:: visibility:: vision:: ws::
cargo test -p fe-database --lib -- handlers::elevated_query api_token_store session_cache handlers::api_token
cargo test -p fe-runtime --lib -- vision::
cargo test -p fractalengine --bin fractalengine -- screenshot_bridge vision_bridge
cargo test -p fractalengine --test security_harness
```

---

## R10 — Identity & persistence

fe-identity, migrations, reconcile, generic DB handlers, canonical data-log (fe-canonical-log).

| Source paths | Approx. count |
|---|---|
| `fe-identity/**` | 64 |
| `fe-canonical-log/**` | 514 |
| `fe-database/src/{op_log,repo,merge,reconcile,space_manager,model_url_meta,replication_mode,atlas,lib}.rs` | 63 |
| `fe-database/src/handlers/{blob_migration,asset_gc,asset_backfill,inbound_admissions,ts_partial,verse_settings}.rs` | 27 |
| `fe-database/src/migration/**`, `fe-database/src/schema.rs` | 40 |
| `fe-database/src/canon_log/**` | 25 |
| `fe-database/tests/{db_test,shared_handle_test,schema_idempotent,migration_conformance_test,migration_equivalence_harness,replication_backpressure_test,canon_log_replay_payload_facts_test,canon_log_composed_admission_test,canon_log_materialization_test,dcl20_bypass_counter_test}.rs` | 69 |

**Total ≈ 802**

```bash
cargo test -p fe-identity
cargo test -p fe-canonical-log
cargo test -p fe-database --lib -- op_log:: repo:: merge:: reconcile:: space_manager model_url_meta replication_mode atlas:: handlers::blob_migration handlers::asset_gc handlers::asset_backfill handlers::inbound_admissions handlers::ts_partial handlers::verse_settings migration:: schema:: canon_log::
cargo test -p fe-database --test db_test --test shared_handle_test --test schema_idempotent --test migration_conformance_test --test migration_equivalence_harness --test replication_backpressure_test --test canon_log_replay_payload_facts_test --test canon_log_composed_admission_test --test canon_log_materialization_test --test dcl20_bypass_counter_test
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

**Total ≈ 312**

```bash
cargo test -p fe-plugin
cargo test -p fe-plugin-test
cargo test -p fe-hexon
cargo test -p fe-hexon-registry
cargo test -p fe-format
cargo test -p fe-sdk
cargo test -p fe-ui --lib -- atlas::dashboard atlas::model_editor atlas::search_bar atlas::tag_panel atlas::room_editor actions::hexon
```

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
| `fe-ui/src/plugin.rs` *(see Consolidation candidates — recommend splitting by owning resource)* | 19 |

```bash
cargo test -p fe-runtime --lib -- messages:: wiring:: app:: shared_node diag15m ktx2_guard safe_id asset_gc bridge:: timeseries::
cargo test -p fe-runtime --test shared_node_test
cargo test -p fe-ui --lib -- plugin::
```

---

## Files touched → batches (lookup)

| Path prefix | Batch(es) |
|---|---|
| `fe-ui/src/navigation_manager.rs`, `fe-ui/src/verse_manager/` (excl. `db_results/terrain.rs`, `path_asset_*`) | R1 |
| `fe-ui/src/verse_manager/db_results/terrain.rs` | R4 |
| `fe-ui/src/verse_manager/path_asset_*.rs` | R5 |
| `fe-ui/src/visibility/` | R1 |
| `fe-ui/src/node_manager/` (excl. `brush_interaction.rs`, `path_*`) | R2 |
| `fe-ui/src/node_manager/brush_interaction.rs` | R4 |
| `fe-ui/src/node_manager/path_*.rs`, `fe-ui/src/node_manager/curve.rs` | R5 |
| `fe-ui/src/gimbal.rs`, `fe-ui/src/geometry.rs` | R2 |
| `fe-ui/src/actions/` (excl. `terrain_proposal.rs`, `path.rs`, `query.rs`, `hexon.rs`) | R2 |
| `fe-ui/src/actions/terrain_proposal.rs` | R4 |
| `fe-ui/src/actions/path.rs` | R5 |
| `fe-ui/src/actions/query.rs` | R3 |
| `fe-ui/src/actions/hexon.rs` | R11 |
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
| `fe-database/src/handlers/elevated_query.rs` | R9 |
| `fe-database/src/handlers/{api_token}.rs`, `api_token_store.rs`, `session_cache.rs` | R9 |
| `fe-database/src/handlers/` (rest: blob_migration, asset_gc, asset_backfill, inbound_admissions, ts_partial, verse_settings) | R10 |
| `fe-database/src/{invite,scoped_invite,scope,rbac,role_manager,role_grants}.rs` | R7 |
| `fe-database/src/canon_log/`, `migration/`, `schema.rs`, `op_log.rs`, `repo.rs`, `merge.rs`, `reconcile.rs`, `space_manager.rs`, `model_url_meta.rs`, `replication_mode.rs`, `atlas.rs`, `lib.rs`, `verse.rs` | R10 (`verse.rs` → R1) |
| `fe-database/tests/role_propagation_inbound_test.rs` | R7 **SLOW** |
| `fe-database/tests/role_propagation_{durable,api_cap,local}_test.rs` | R7 |
| `fe-database/tests/{gpx_path_persistence_test,gpx_pipeline_test}.rs` | R5 |
| `fe-database/tests/{entity_property_test,gis_data_test}.rs` | R3 |
| `fe-database/tests/{device_enrolment_test,device_payload_test,iot_reading_test,observation_plane_test}.rs` | R4 (IoT) |
| `fe-database/tests/{petal_render_distance_test,visibility_groups_test,node_lifecycle_test,node_view_test,node_data_write_readback_test}.rs` | R4 / R1 (see R1, R4 tables) |
| `fe-database/tests/` (rest: db_test, shared_handle_test, schema_idempotent, migration_*, replication_backpressure_test, canon_log_*_test, dcl20_bypass_counter_test) | R10 |
| `fe-terrain/**` | R4 (always `--features render`) |
| `fe-api/**` | R9, except files cross-listed into R4/R7/R8 (see R9 table header) |
| `fe-sync/**` | R8 |
| `fe-network/**`, `fractalengine-relay/**` | R8 |
| `fe-runtime/src/{peer_registry,distributed_query,blob_transport,blob_store,bevy_blob_reader}.rs` | R8 |
| `fe-runtime/src/vision.rs` | R9 |
| `fe-runtime/` (rest) | Misc |
| `fe-identity/**`, `fe-canonical-log/**` | R10 |
| `fe-policy/**` | R7 |
| `fe-plugin/**`, `fe-plugin-test/**`, `fe-hexon/**`, `fe-hexon-registry/**`, `fe-format/**`, `fe-sdk/**` | R11 |
| `fe-renderer/**` (excl. terrain_overlay, terrain_height) | R2 |
| `fe-renderer/src/{terrain_overlay,terrain_height}.rs` | R4 |
| `fe-webview/**` | R6 |
| `extensions/iot-bridge/**`, `fe-sim/**` | R4 (IoT) |
| `fractalengine/src/{asset_bridge}.rs` | R2 |
| `fractalengine/src/gpx_bridge.rs` | R5 |
| `fractalengine/src/terrain_bridge.rs` | R4 |
| `fractalengine/src/{screenshot_bridge,vision_bridge}.rs` | R9 |
| `fractalengine/src/{panic_log,window_chrome}.rs` | R6 |
| `fractalengine/tests/security_harness.rs` | R9 |
| `fractalengine/tests/integration_test.rs` | Harness |
| `fe-test-harness/**` | Harness (always slow) |
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

```bash
cargo test --workspace
cargo test -p fe-terrain --features render
```
