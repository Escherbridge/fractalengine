---
type: Review
track: map_scale_authority_20260716
gate: R-CITY-1
reviews: [CITY-01, CITY-02, CITY-03, CITY-04, CITY-08, CITY-09]
date: 2026-11-13
---

# R-CITY-1 adversarial review (turn 4)

Diffs reviewed: `git diff 7af0a144 -- <owned files>` for CITY-01/02/03/04/08/09, plus the 21:55 steer (world-scale cap, MEDIUM).

F1 FIXED - fe-format/src/scale.rs:5-41 - steer 21:55: `sanitize_world_scale` accepted any finite positive f64, so `1e300` became `inf` and `1e-300` became `0` after the `as f32` placement cast. Added `MIN_WORLD_SCALE = 1e-6`, `MAX_WORLD_SCALE = 10_000.0` and `is_world_scale_in_range`; out-of-range sanitizes to 1.0, and `scale_bounds` are intersected with the global range (narrow only, never widen). Range deviates from the steer's 0.01 default because the spec names a 1:1000 region preset, the shipped hexon-manager slider goes to 1:10000 (dialogs/hexon_manager.rs:467) and `derive_scale_from_bounds` gives ~6.4e-6 for a zoom-0 tileset; a 0.01 floor would silently change those maps. Tests extended: 1e300 / 1e-300 / f64::MAX rows in `sanitize_rejects_non_positive_and_non_finite`, `clamp_vectors_match_terrain_parity`, `accessor_table`.
F2 FIXED - fe-database/src/handlers/crud.rs:186-200 - steer rule 2: new `placement_scale` re-checks `(v as f32).is_finite() && > 0` (else 1.0) for both `create_node_as` and `import_gltf_handler` (via `placement_scale_vec`); `placement_scale_follows_the_petal_map` gains 1e300 / 1e-300 / f64::MAX terrain rows seeded raw (inbound-replica shape) plus direct `placement_scale_vec` assertions.
F3 HANDED-OFF (CLOCK-09 / R-CLOCK-2) - fe-database/src/handlers/petal_terrain.rs:63 `prepare_terrain_write` - steer rule 3: refuse a terrain document whose `world_scale` is present and not `fe_format::scale::is_world_scale_in_range`; inbound replicated rows keep the tolerant read.
F4 HANDED-OFF (R-HEXON-1) - fe-api/src/terrain.rs:246 `set_petal_terrain_core` (shared by REST PUT and MCP `set_petal_terrain`) - after `validate_publisher_pins`, return `InvalidConfig` when `!fe_format::scale::is_world_scale_in_range(config.world_scale)`.
F5 HANDED-OFF (orchestrator: no round-4 owner) - fe-database/src/handlers/object_creation.rs:178 `prepare` - steer rule 2 for caller-supplied scale: add `&& scale.iter().all(|v| { let n = *v as f32; n.is_finite() && n > 0.0 })` to the transform `ensure!` (covers `create_object` and `create_node_with_asset_as`).
F6 HANDED-OFF (R-CITY-2, owns the file) - fe-ui/src/node_manager/path_point_interaction.rs:532 - CITY-09 added `PendingPenCreate.trailing_points` but this full struct literal (CITY-05) lacks it: E0063 compile break in fe-ui. Add `trailing_points: Vec::new(),`.
F7 FIXED + HANDED-OFF (R-EXT-3 / EXT-12, own db_results/mod.rs) - fe-ui/src/gis/mod.rs:561 - CITY-09's AC-10.2 claim is false: a correlated `CreateNode` failure arrives as `DbResult::RequestFailed` (fe-database handlers/mod.rs:68 `correlated_failure`), not `DbResult::Error`, and the RequestFailed arm (db_results/mod.rs:989) never clears `pending_pen_create`, so a failed road/pen create strands the tool. Added `PathEditorState::fail_pending_pen_create(correlation_id)` with a test row in `clear_pending_pen_create_reports_and_clears`; NEEDS the arm to call `path_state.fail_pending_pen_create(correlation_id)`.
F8 HANDED-OFF (R-EXT-3 / EXT-12) - fe-ui/src/verse_manager/db_results/mod.rs:~1960 - CITY-09 did not extend the pen flush journey: add `trailing_points: vec![[4.0,0.0,4.0]]` to one flush case and assert the queued actions are `[PathAppendPoint, PathAppendShape]` in order.
F9 FIXED - fe-ui/src/node_manager/ground_snap.rs:43 - when the sampled terrain sits above the ray's reach (camera low, plateau ahead), `ground_point` returned the y=0 hit under the terrain; it now returns that XZ at the sampled surface height. Table test extended with a plateau row.
F10 FIXED - fe-ui/src/verse_manager/db_results/nodes.rs:130 - needless `trailing_points.clone()` on an owned value; moved instead.
F11 FIXED - fe-ui/src/verse_manager/path_asset_reconcile.rs:214 - stale `#[allow(clippy::neg_cmp_op_on_partial_ord)]` after CITY-03 removed the comparison.
F12 HANDED-OFF (I-DB-1) - fe-database/src/lib.rs:888 - still matches `Ok(id)` against the new `(String, [f32;3])` and emits `NodeAdded { scale: [1.0;3] }`; plus fe-database/Cargo.toml lacks `fe-format`. Both already routed to I-DB-1; until it lands fe-database does not compile.
F13 HANDED-OFF (I-LIBS-1, I-UI-1) - fe-format/src/lib.rs has no `pub mod scale;`; fe-ui/Cargo.toml has no `fe-format`; fe-ui/src/node_manager/mod.rs has no `mod ground_snap;`; plugin.rs cursor still intersects Y=0 (CITY-04 seam). All routed already; listed so C-CITY checks them.
F14 KEPT (start_editing runs on the line before and actions apply in order) - fe-ui/src/verse_manager/db_results/nodes.rs:128 - `PathAppendShape` resolves its track from `editing_track_id` at apply time, not an explicit id.
F15 KEPT (no journey drives `advance_path_edits` CreateTrack; mutation of the list turns it red) - fractalengine/src/gpx_bridge.rs:4121 - new fn `authored_track_properties_are_designed`.
F16 KEPT (justified in CITY-02 task; no placement journey existed) - fe-database/src/handlers/crud.rs:1776 - new fn `placement_scale_follows_the_petal_map`.
F17 HANDED-OFF (I-DOC-CORE, CITY-14) - fe-format/src/AGENTS.md §scale - record the global range, why it is wider than the stamp range, and that bounds only narrow it.
F18 HANDED-OFF (I-BATCH-1) - conductor/test-batches.md - confirm filters reach `scale::tests::` (fe-format R11), `handlers::crud::` (R1), `node_manager::ground_snap` (R2), `gis::tests` (R5).
F19 KEPT (verified) - CITY-01 pinned seams exported as named; fe-terrain re-export keeps its parity tests unchanged; CITY-08 path_kind written on authored create and GPX import only; no swallowed sends or per-frame allocation added; leak-discipline words absent.

Verdict: PASS-WITH-FIXES (fe-ui and fe-database compile only after F6, F12 and F13 land)
