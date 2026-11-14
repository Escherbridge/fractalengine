---
type: Review
track: road_builder_ux_20260716
critic: C-CITY
round: 4
date: 2026-11-13
---

# C-CITY completeness check: road_builder_ux (round 4)

Checked against the working tree at `733a3256`. No cargo was run. Statuses: complete, partial, missing,
pending-run (code done, the run waits for the user to name the batch), obsolete and blocked-user (classes the
scoper excluded; they are not gaps). Paths are under `fe-ui/src/` unless noted.

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| FR-1 straight / curved modes | complete | `node_manager/road_chain.rs:279` `on_click`, `:433` `expand_segment` via `curve::bezier`; journey `road_chain_session_journey` (AC-1.1, AC-1.2) |
| FR-1 freeform decimation | partial | `road_chain.rs:304-307`, `:404`; AC-1.3 tested. Defect: `freeform_spacing_m` is passed to `decimate` as world units with no `meters_to_world` (`road_chain.rs:306`, `road_overlay.rs:108`), so on a 1:100 map 2 m spacing becomes 200 m. Spec says "1.0 m via world_scale"; defaults also differ (spacing 2.0 vs 1.0, curve samples 16 vs 12, `road_chain.rs:67-69`) |
| FR-1 AC-1.4 / FR-10 AC-10.1 existing ops only | complete | `road_interaction.rs:155` `apply_commits` uses PathCreateTrack / PathAppendPoint / PathAppendShape / PathRemovePoint; R-CITY-2 RC2-21 |
| FR-2 chaining, no duplicate anchor | complete | `road_chain.rs:253` `commit_points`; AC-2.1 in the journey |
| FR-2 Esc two-stage | complete | `node_manager/mod.rs:218,235,259` (`GestureParams.road`, `is_active`, `cancel_stage`); AC-2.3 in the journey |
| FR-2 Backspace undo last segment | complete | `road_chain.rs:310` `on_backspace`, paced removals `road_interaction.rs:221` `issue_next_removal` (RC2-01..04); AC-2.2 in the journey |
| FR-3 angle snap, distance kept, Alt suspend | complete | `node_manager/road_snap.rs:91`; Alt `road_interaction.rs:332`; tests `angle_snap_table_preserves_distance`, `angle_snap_inert_cases_keep_cursor` |
| FR-3 shortcut toggle | partial (deliberate) | A is the camera fly-left key, so the toggle is panel-only (`road_interaction.rs:30-37`, RC2-06); spec FR-8/AC-8.1 text not amended |
| FR-4 guidelines + total order | complete | `road_snap.rs:199` `guidelines`, `:251` `guide_intersection`, `:315` `resolve_snap`; test `resolve_snap_total_order_alt_and_non_finite` |
| FR-5 snap to paths, bitwise copy | complete | `road_snap.rs:137` `vertex_snap`; test `click_near_a_foreign_endpoint_copies_its_coordinate_bitwise` (`road_interaction.rs:578`); D-3 limitation in `node_manager/AGENTS.md:239` |
| FR-6 ghost + validity | complete (code) | `node_manager/road_overlay.rs:56` `classify_validity`, `:83` ghost, `:183` overlay; AC-6.1 `classify_validity_table`; AC-6.2 blocked-user |
| FR-7 metric readout | complete | `road_overlay.rs:140` `format_readout` (m/km, `wu` fallback), `:237` `draw_road_readout` registered `plugin.rs:1110`; tests at `road_overlay.rs:389,400` |
| FR-8 Tool::Road, hotkey L | complete | `panels/toolbar.rs:27,103-107` (user default §9 #25 = L); exhaustive matches `node_manager/dispatch.rs:109,128` |
| FR-8 router claim | complete | `ClickPriority::Road` after Measure (`node_manager/router.rs:26`); AC-8.2 `dispatch.rs:499` |
| FR-8 Tools-panel Road section | partial | `panels/tool_options.rs:124` `render_road_options`, `:655` `apply_road_setting`; unreachable at HEAD: `ui_shell/right_sidebar.rs:293`/`:595` take no `road` argument while `panels/mod.rs:270` passes `road_state` (R-INT-4 F4 / N1 open; fe-ui does not compile) |
| FR-9 path_kind | complete | `fractalengine/src/gpx_bridge.rs:30,168,209`; tests `:4100`, `:4125`; doc `fractalengine/src/AGENTS.md:699` |
| FR-9 AC-9.3 composition | partial | covered as two halves (road queue test `road_interaction.rs:493`, bridge property test `gpx_bridge.rs:4121`); no single flow asserts both |
| FR-10 first-commit auto-create | complete | `gis/mod.rs:592` `trailing_points`, `:651-655` `road-track:N`; flush `verse_manager/db_results/nodes.rs:128-133`; name "Road N" (journey) |
| FR-10 AC-10.2 failed create clears the tool | partial | bare `DbResult::Error` clears (`db_results/query.rs:102`); a correlated create fails as `DbResult::RequestFailed`, and that arm (`db_results/mod.rs:1000-1020`) never calls `fail_pending_pen_create` (`gis/mod.rs:568`), so the road or pen tool stays stuck (R-CITY-1 F7) |
| D-38 pen anchors on terrain (§9 #26) | complete | `node_manager/path_point_interaction.rs:338` |
| Snap radius world-space 3 m (§9 #28) | complete | `road_chain.rs:67`, converted `road_interaction.rs:120`, ring `road_overlay.rs:220` |
| NFR-1 purity | complete | `road_snap.rs:7` imports only `SnapCandidate`; `road_chain.rs` uses only `bevy::prelude::Resource` for the holder |
| NFR-2 per-frame budget | complete (one deferral) | `MAX_SNAP_ENTRIES` cap `road_interaction.rs:28`; buffers in `RoadLocals`; curved ghost still allocates (RC2-10 DEFERRED until `curve.rs` gains `bezier_into`) |
| NFR-3 input safety | complete | arbiter claim + `EguiWantsInput` gate `road_interaction.rs:340-345` |
| NFR-4 no unwrap in prod | complete | unwraps only below `#[cfg(test)]` (`road_snap.rs:398`, `road_interaction.rs:444`) |

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| P1 angle / vertex / guides / decimation / expansion / chain log | complete | rows above |
| P1 metric_scale helper | obsolete | excluded; `measure::measure_world_scale` + `geometry::format_distance_m` |
| P1 vertex candidate collection | obsolete | excluded; reuses `path_point_interaction.rs` `collect_foreign_anchors` |
| P1 verification | pending-run | R5 filter `conductor/test-batches.md:194` |
| P2 path_kind tasks + doc | complete | FR-9 rows |
| P2 verification | pending-run | `test-batches.md:196` `gpx_bridge::` |
| P3 Tool::Road, router, state, interaction, first commit | complete | rows above; registration `node_manager/mod.rs:36-42,429-430,466,509` |
| P3 scripted acceptance tests | partial | AC-1.1/1.2/2.1/2.2/2.3/5.1/10.1 present; AC-9.3 split; AC-10.2 only for bare Error |
| P3 verification | pending-run / blocked-user | R5 batch; in-app 3-segment check excluded |
| P4 ghost, guides, readout | complete | FR-6, FR-7 rows |
| P4 Tools-panel Road section | partial | FR-8 panel row (sidebar threading) |
| P4 in-tool shortcuts | complete | `road_interaction.rs:32-37`, `road_chain.rs:476` `shortcut_to_event`; A deviation recorded |
| P4 verification | blocked-user | excluded |
| P5 edge cases | partial | petal switch clears (`road_interaction.rs:291-305`, asserted at `:573`); double-create guard via `has_pending_pen_create` (`:139`); degenerate scale falls back through `meters_to_world`; candidate cap; unwrap audit clean. Open: freeform spacing not scaled |
| P5 docs | partial | `node_manager/AGENTS.md:230-285`, `fe-ui/src/AGENTS.md:643`; the paced removal queue and "undo waits for the create echo" (RC2-15) are not in §road-chain, and the §road-builder / §path-editor HANDED-OFF notes still describe AC-10.2 as unwired |
| P5 workspace sweep | obsolete | excluded |
| P5 close-out (metadata, retro) | pending | waits on the sweep and the in-app pass |
| P5 verification | blocked-user | excluded |

## Hand-offs from R-CITY-2: did they reach their owner?

| Line | Reached? | Evidence |
|---|---|---|
| RC2-13 (I-UI-1) Escape in GestureParams | yes | `node_manager/mod.rs:218,235,259` |
| RC2-14 (I-UI-1, I-UI-2) module and system registration | yes | `node_manager/mod.rs:36-42,429-430,466,509`, `plugin.rs:1110`; the right-sidebar threading half is open (R-INT-4 F4 / N1) |
| RC2-15 (I-DOC-UI) doc removal queue, A key, echo wait | partial | A key documented (`node_manager/AGENTS.md:245-246`); queue and echo wait absent |
| RC2-19 (I-BATCH-2 / CITY-15) R5 road filters | yes | `conductor/test-batches.md:194` |
| RC2-10 DEFERRED curved ghost allocation | still deferred | trigger unchanged |
| R-CITY-1 F6 (R-CITY-2) `trailing_points` in pen literal | no | `node_manager/path_point_interaction.rs:564-572` (E0063) |

SEC-INBOX: no entry names a road builder file; entry 2 (world-scale cap) is tracked in the map_scale_authority completeness file.

## HANDED-OFF (owner = next round)

- HANDED-OFF (next round, owner of `fe-ui/src/ui_shell/right_sidebar.rs`): apply R-INT-4 N1 (thread `road: &mut RoadBuilderState` into `render_right_sidebar` and `render_options_section`).
- HANDED-OFF (next round, owner of `node_manager/path_point_interaction.rs`): add `trailing_points: Vec::new(),` at :564.
- HANDED-OFF (next round, owner of `node_manager/road_chain.rs` + `road_overlay.rs`): convert `freeform_spacing_m` with `meters_to_world` at the petal scale before `decimate`; extend the journey with a 1:100 row.
- HANDED-OFF (next round, owner of `verse_manager/db_results/mod.rs`): RequestFailed arm calls `fail_pending_pen_create`, then drop the HANDED-OFF notes in both AGENTS files.
- HANDED-OFF (next round, owner of `fe-ui/src/node_manager/AGENTS.md`): document the paced removal queue and the echo wait in §road-chain (RC2-15).
- HANDED-OFF (next round, track owner): amend spec FR-8/AC-8.1 (angle snap is panel-only) and state the default spacing and sample counts the code ships.
- HANDED-OFF (next round, owner of `road_interaction.rs` tests): one flow that ends in the bridge mapping with `path_kind: "designed"` (AC-9.3), by extending the existing journey.

Verdict: PARTIAL. Snap core, chain, overlay, router, provenance and Escape are in code with tests; fe-ui does not compile until the right-sidebar threading and the pen literal land, and freeform spacing ignores the map scale.
