---
type: plan
title: Hierarchy Visibility + Groups
tags: [hierarchy_visibility_groups_20260808]
resource: ./spec.md
---

# Implementation Plan: Hierarchy Visibility + Groups

All 18 decisions ratified 2026-08-08 (spec.md "RATIFICATION"). D-7 = spawn-then-hide,
so the picker filters ARE the hide mechanism. Rationale for the shipped code lives in
`fe-ui/src/AGENTS.md` §visibility, `fe-ui/src/node_manager/AGENTS.md`
§visibility-pickers / §staged-escape and `fe-database/src/AGENTS.md` §lifecycle
("Node view").

## Phase 0 — resolver + state types [DONE e76bbb9]

- [x] `fe-ui/src/visibility/resolve.rs` lattice (solo > override > ancestor > ANY-hidden-group) + `VisibilityState`.

## Phase 1 — sidebar eyes + apply system [DONE e76bbb9]

- [x] Sidebar eye per row, muted hidden rows, tri-state rollup, "N hidden" status chip (D-16).
- [x] `sync_node_visibility` spawn-then-hide apply + D-13 force-deselect; viewport_pick filter; D-14 camera-focus refusal.

## Phase 2 — make hiding real on every surface [DONE, commit pending]

- [x] One shared picker rule `visibility::pickable` (+ `active_ancestry`) used by viewport_pick, context_pick (ray + ground-stamp tiers), sidebar_sync, pointer bridge, path-segment guard, foreign-anchor snap, camera focus. Tests: `visibility::tests::pickable_refuses_every_effectively_hidden_cause`, `context_pick::tests::the_ground_stamp_tier_skips_hidden_stamps_and_other_petals`, `sidebar_sync::tests::*`, `pointer::tests::track_to_open_refuses_a_hidden_listed_track` + `hiding_the_edited_track_closes_its_session_and_drops_its_selection`, `path_segment_interaction::tests::a_hidden_edited_ribbon_takes_no_segment_clicks`, `path_point_interaction::tests::a_hidden_track_contributes_no_snap_candidates`.
- [x] Pointer rule "cannot open, must close": `track_to_open` refuses a hidden track; an open session whose track becomes hidden is stopped (arm 4 drops its selection); the silent-pick toast and the Paths→viewport arm ignore hidden tracks.
- [x] Sidebar select of a hidden node is consumed and refused with the toast "That object is hidden — show it first".
- [x] Keys (D-17, `node_manager/shortcuts.rs`): H = hide selection (persisted), Shift+H = Show all for the active branch (`VisibilityState::show_all_in`), Alt+H = isolate selection (session solo). Ctrl/Cmd+H unbound. Tests: `h_key_modifiers_map_to_exactly_one_verb`, `h_hides_the_selection_through_the_persisted_write_and_is_inert_without_one`, `alt_h_isolates_the_selection_for_the_session_only`, `shift_h_shows_everything_and_persists_only_the_hides_it_cleared`.
- [x] Esc ladder `EscapeRung::ClearIsolate`. **"Esc clears isolate first" is implemented as first-AFTER-a-live-gesture**: a mid-drag Escape still cancels the drag (F5), the next press clears isolate, before StopEditing/Deselect/ResetTool. Test: `clear_isolate_sits_between_a_live_gesture_and_every_session_rung`.
- [x] Sidebar node-row right-click menu (D-8): Isolate / Hide others / Show all. Test: `visibility::tests::hide_others_persists_a_hide_for_every_other_not_yet_hidden_node`.
- [x] "Add selection to group…" row verb — selects the row and reveals the right-sidebar Groups section (`RevealSection { slug: "groups" }`), landed with Phase 5.
- Open question (not decided, not built): should hiding a track also hide its stamps? Today stamps are filtered on their own marker id, exactly as the renderer hides them; no track→stamp inheritance was invented.

## Phase 3 — persist `view.state` / `view.groups` [DONE, commit pending]

- [x] `DbCommand::SetNodeView { node_id, state, groups, auth, correlation_id }` + `LoadPetalViewState`; `DbResult::NodeViewSet` / `PetalViewStateLoaded` (`NodeViewDto`); reply kinds in `fe-runtime/src/app.rs`; `SetNodeView` registered `UnmappedDeferred` (mapping_inventory).
- [x] `fe-database/src/handlers/node_view.rs`: validate → tombstone-aware resolve → `lifecycle_auth_context` → `authorize_node_edit` (Editor+) → per-key PropertySet/PropertyDeleted op-log writes; `auto`/empty groups delete the key. Test: `fe-database/tests/node_view_test.rs` (Viewer denied + row untouched, Editor allowed, **override survives a DB close/reopen on the same store**, `auto` removes, tombstoned refused, correlated failure is `RequestFailed`).
- [x] `fe-ui/src/visibility/cache.rs`: `hydrate_from_properties` (wired in `db_results/properties.rs`), `apply_petal_view_state`, `apply_node_view_echo`, `load_petal_view_state_on_nav_change`, `UiAction::SetNodeView` → `set_node_view` (optimistic + `CallerAuth::Local`, correlated). Refused writes keep the session state and toast "Visibility not saved (you need Editor access)". Tests in `cache.rs` + `db_results::tests::properties_loaded_hydrates_a_persisted_hide_even_when_unselected`.
- [x] Group membership editing UI (`view.groups`) — right-sidebar Groups section "Add selection to group" / "Remove from group" → `UiAction::SetNodeGroups` → `SetNodeView { groups }` (Phase 5).

## Phase 4 — group registry + persisted fractal/verse eyes [DONE, commit pending]

- [x] Schema (idempotent, no backfill — `NONE` = visible / no groups): `petal.visibility_groups` `option<array<object>> FLEXIBLE` + `DEFINE FIELD OVERWRITE visibility_groups[*] … TYPE object FLEXIBLE` (SurrealDB 3 rejects FLEXIBLE on a bare `array` and does not open array ELEMENTS — fe-database AGENTS §schema-macro); `view_hidden option<bool>` on verse, fractal AND petal. **Assumption (planner):** the petal eye is persisted too — D-1 persisted the levels above and node overrides persist below, so a session-only petal eye in between would be incoherent; reversal = drop the petal field. Test: `schema_idempotent.rs::visibility_fields_apply_to_a_populated_pre_phase4_store`.
- [x] `DbCommand::{SetPetalVisibilityGroups, GetPetalVisibilityGroups, SetScopeHidden}` + `DbResult::{PetalVisibilityGroupsLoaded, ScopeHiddenSet}` (`VisibilityGroupDto`, `ScopeLevel`); hierarchy DTOs carry `view_hidden`; correlated reply families in `fe-runtime/src/app.rs` (test `visibility_group_replies_reach_only_their_own_api_waiter`); both writes `UnmappedDeferred`.
- [x] `fe-database/src/handlers/visibility_groups.rs`: validate (≤256, unique ids ≤64 chars, names 1-80, finite order) → scope resolve (missing = failure) → `lifecycle_auth_context` → `authorize_node_edit` (reused — it is `Write` at any scope) → write → reply. DB thread now releases its SurrealKV store before exiting so an in-process restart can reopen it. Test: `fe-database/tests/visibility_groups_test.rs` (Viewer denied + nothing stored, Editor round-trip exact, **registry + all three eyes survive a DB-thread restart on the same store**, eyes reach `HierarchyLoaded`, missing petal/scope fails, duplicate id rejected).
- [x] fe-ui `visibility/groups.rs`: `UiAction::{SetGroupRegistry, SetScopeHidden, SetNodeGroups}` (optimistic + `CallerAuth::Local`, correlated; refusal keeps the session value + toasts "Groups not saved (you need Editor access)"), registry fetched on petal entry (`load_petal_groups_on_nav_change`), `PetalVisibilityGroupsLoaded` applied for the active petal only, hidden sets hydrated from `HierarchyLoaded`. Every sidebar scope eye persists; Show all persists the branch's re-opened eyes. Active-petal eye stays disabled (D-4). Tests: `visibility::groups::tests::*`.

## Phase 5 — right-sidebar Groups section + unified layer registry [DONE, commit pending]

- [x] `RightSidebarSection::Groups` (slug `"groups"`, label "Groups", 7 sections): create / rename / delete / eye / reorder (whole-registry `SetGroupRegistry`, pure transforms tested in `registry_edits_change_exactly_one_thing`) + "Add selection to group" / "Remove from group"; empty Group copy "No members — select nodes and use Add to group". Slug round-trip + `RevealSection` tests iterate `ALL_SECTIONS`, so they cover the new variant.
- [x] Left sidebar: collapsed "Groups" section (eye + member count, read + toggle only, "Manage Groups…" reveal) and the node-row "Add selection to group…" verb.
- [x] Unified registry (D-11): `gis::overlay_registry` → `OverlayEntry { key: OverlayKey::{TerrainLayer, Group, ScaleBar, WorldGrid}, name, visible, opacity }`; `apply_overlay_toggle(key, visible, petal_id, groups)` (needs the petal id + registry for its persisted writes — deviation from the planned two-argument shape); Layer Manager renders only from it. Tests: `overlay_registry_lists_layers_then_groups_then_ruler_overlays`, `overlay_registry_without_a_map_has_no_terrain_rows`, `overlay_toggle_maps_each_key_to_its_action`, `app_overlay_writes_only_its_own_setting`.
- [x] GPX/GeoJSON toggles live: empty-source `gpx_track`/`geojson_overlay` = CATEGORY layer (`fe-terrain` `gpx_layer_for`/`geojson_layer_for`, test `overlay_binding_prefers_per_source_layer_then_category_layer`); visibility applied at build time too, and `StyleHidden` keeps a `gis.track.visible=false` ribbon hidden through layer toggles. `INERT_LAYER_NAMES` + "Coming soon" removed.

## Phase 6 — REST + MCP endpoints [DONE, commit pending]

- [x] `GET|PUT /api/v1/petals/{p}/visibility-groups`, `PATCH|DELETE …/{group_id}` (`fe-api/src/visibility.rs`): Viewer+/Editor+ floor → `authorized_petal_scope` (uniform `NOT_FOUND_OR_DENIED`) → correlated DB round trip with `caller_auth` (DB re-checks Editor+). PATCH/DELETE = correlated Get→Set read-modify-write (last-writer-wins window documented in fe-api AGENTS §visibility-groups).
- [x] MCP `list_visibility_groups` (Viewer) + `set_visibility_group` (Editor; upsert or `delete: true`) sharing the cores (31-row tool table).
- [x] Tests: `fe-api/tests/visibility_groups_api_test.rs` (Viewer GET ok / PUT refused, Editor PUT round-trips, malformed 400 never stored, PATCH upsert + DELETE + unknown 404, foreign petal byte-identical to a missing one on GET and PUT, MCP set → list → Viewer write refused → delete); routes added to `existence_oracle_test.rs`; MCP inventory/foreign-scope sweeps updated.

## W4 adversarial review (2026-10-11) — FIXED, commit pending

- [x] Layer/style-hidden ribbons (`Visibility::Hidden` set by fe-terrain, outside the lattice) were still ray-pickable and snappable once Phase 5 made the layer toggles live → `visibility::rendered` gate in `viewport_pick`, `context_pick`, `ForeignSnapSources` (test `a_ribbon_hidden_by_its_layer_is_not_picked_until_shown`).
- [x] H on a stamp instance sent a doomed `SetNodeView` and Shift+H could never restore it → stamp hides are session-only and Show all clears the branch tracks' stamp hides (tests in `shortcuts.rs`, `show_all_also_restores_hidden_stamps_of_the_branch_tracks_without_persisting_them`).
- [x] `SetNodeView` missing from `correlation_of_command` (an API-correlated write could never match its echo) → added (test `a_correlated_set_node_view_receives_its_own_echo`).
- [x] Groups editor now caps names / registry size / per-node membership at the DB's validation bounds; a failed registry READ no longer toasts "Groups not saved" (`fe-ui:groups-load:` prefix).
- [x] Late-spawned node entities (GLTF, ribbons, stamps, restart hydration before spawn) stayed visible because `sync_node_visibility` applied only on epoch/petal change, and a Layer Manager toggle re-showed a lattice-hidden ribbon → the pass also runs on `Added<SpawnedNodeMarker>` and re-asserts `Hidden` on entities it hid (test `late_spawns_are_hidden_and_a_foreign_reshow_is_reverted`).
- [x] A petal persisted hidden and then entered had a disabled eye (D-4), so its contents stayed hidden → the active petal's eye can SHOW but never hide (`panels/sidebar.rs::petal_eye`, test `the_active_petal_can_be_shown_but_never_hidden`).

## USER_GATED (in-app)

- Hidden nodes/tracks cannot be clicked, right-clicked, sidebar-selected or path-edited.
- H / Shift+H / Alt+H feel; Esc clears isolate.
- A hidden override survives an app restart.
- Groups section CRUD + Group/scope eyes feel right; a Group hide and a closed fractal/verse eye survive an app restart.
- Layer Manager "GPX tracks" / "GeoJSON overlays" toggles hide and show the ribbons / overlays, and a layer-hidden ribbon cannot be clicked or snapped to.
- The Groups rail glyph (U+25C9) and the sidebar eye glyphs render in the bundled egui font.
