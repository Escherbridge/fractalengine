---
type: Implementation Plan
title: "Implementation Plan: Stamped-Asset Nodes — Curve-Follow, Selection, Overrides, Instancing"
tags: [stamped_asset_nodes_20260725]
resource: ./spec.md
---

# Implementation Plan: Stamped-Asset Nodes

Five phases. FR-1 (curve-follow) is a self-contained win that can land before the
node work; FR-2/3 depend on T1's promotion; FR-4 is the scale hardening.
TDD on the samplers + override math; single sweep at the end (N-6).

## Phase 1: Stamps follow the curve (FR-1) [P0]

- [x] Task: make stamp spacing sample the curve at correct arc-length (both
      spacing modes) instead of the flattened polyline; unit test — stamp
      centroids lie on the bezier within tolerance; legacy straight paths
      byte-identical. **[audit 2026-10-10: DONE 56ccc3b+623145f — fe-terrain/src/mesh/curve.rs arc-length sampler; tests stamp_sits_on_the_curve_not_the_chord, straight_route_samples_identically_to_chords, spacing_offsets_are_even_and_capped, count_offsets_spread_evenly; materializer flattens through the bezier at both cache feeders (623145f, verse_manager/path_asset_materialize.rs)]**
- [x] Task: "Align to path tangent" uses the true curve tangent (not chord);
      tangent test. All geometry in meters (N-1), no `world_scale`. **[audit 2026-10-10: DONE 56ccc3b — tests tangent_follows_curve_not_chord, positions_pass_through_without_scaling (curve.rs)]**

## Phase 2: Individual selection = addressable node (FR-2) [P1]

- [x] Task: viewport pick resolves an individual stamp; on first individual
      select, promote via T1 FR-5 → stable address (T1 FR-4). **[audit 2026-10-10: DONE 56ccc3b+623145f — StampRenderIndex/pick_nearest (fe-renderer/src/instancing.rs) used by node_manager/context_pick.rs; select→PromoteInstance→NodePromoted round-trip; tests select_flags_first_unpromoted_stamp_for_promotion, selecting_already_promoted_stamp_does_not_promote_again (fe-ui/src/actions/asset.rs), promote_is_authorized_and_idempotent + promotion_writes_the_stamp_read_contract_properties (fe-database/tests/node_lifecycle_test.rs)]**
- [ ] Task: row-count test (N-9) — un-selected stamps add zero store rows. **[audit 2026-10-10: OPEN — promotion is select-gated (asset.rs tests above) but no test asserts the store row count stays flat for un-selected stamps]**

## Phase 3: Position-locked gimbal + scale/rotate overrides (FR-3) [P1]

- [ ] Task: gimbal for a stamp exposes scale + rotate, disables free translate;
      per-stamp override persists as a sparse record; base transform stays
      path-derived. **[audit 2026-10-10: PARTIAL (consolidator re-check: un-ticked) — panel path DONE 56ccc3b+623145f — sparse overrides + 1-D slide, no free translate in the stamp editor fe-ui/src/panels/tool_panel.rs:406-408; tests scale_override_persists_after_promotion, rotation_override_persists_quaternion_after_promotion, overrides_are_sparse_untouched_axis_stays_none. RESIDUAL: the viewport Move-tool entity gimbal treats a bridged stamp like a Node (node_manager/gimbal_interaction.rs:267, 401) — CONFIRMED no stamp translate lock: gimbal_interaction.rs:267-273 resolves a gimbal-axis hit with `SelectionKind::Node` standing in for Stamp, so Move still drags a bridged stamp — ledger STAMP-GIMBAL-TRANSLATE-LOCK]**
- [x] Task: override-survives-reflow test. **[audit 2026-10-10: DONE — reflow_shifts_surviving_overrides_and_drops_deleted, reflow_leaves_other_tracks_untouched (fe-ui/src/actions/asset.rs:724,764)]**

## Phase 4: Instancing + spatial pick at scale (FR-4) [P1]

- [x] Task: `fe-renderer/src/instancing.rs` — GPU-instanced stamp draw; promoted
      overrides fold into the instanced buffer. **[audit 2026-10-10: DONE (data layer) 56ccc3b — InstanceBatch/to_matrix with overrides folded (instancing.rs); the DRAW rides Bevy auto-instancing over shared GLB handles; a custom instanced pipeline is a recorded follow-up (fe-ui/src/verse_manager/AGENTS.md:312-317)]**
- [x] Task: spatial pick index for stamps; correctness test (returns the right
      stamp) + a ≥10k-stamp render+pick bench with a documented budget. **[audit 2026-10-10: DONE 56ccc3b — StampSpatialIndex; tests spatial_index_picks_the_nearest_stamp, bench_10k_build_and_pick; budget TARGET_INSTANCE_CEILING=10_000 documented fe-renderer/src/AGENTS.md §instancing (timed criterion harness = follow-up)]**

## Phase 5: Delete-stamp re-flow (FR-5) [P1]

- [x] Task: consume T1's reflow event → re-distribute remaining stamps per the
      active spacing mode, preserving other stamps' overrides; invariant test. **[audit 2026-10-10: DONE 623145f — LifecycleEvent PathReflow pumped crossbeam→Bevy (verse_manager/lifecycle_events.rs) drives override remap + restamp; tests tombstoning_a_promoted_stamp_reflows_with_deleted_index (node_lifecycle_test.rs:733), reflow_dropping_deleted_index_removes_its_override, reflow_deleting_selected_stamp_clears_selection]**

## Phase 6: Docs + integrated sweep [P1]

- [x] Task: `fe-terrain/src/mesh/AGENTS.md` + `fe-renderer/AGENTS.md` — curve
      sampling, override model, instancing/pick (N-7). **[audit 2026-10-10: DONE — fe-terrain/src/mesh/AGENTS.md §curve-stamps (:72), fe-renderer/src/AGENTS.md §instancing (:134), fe-ui/src/actions/AGENTS.md]**
- [x] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests
      (N-6); ui_ux checklist on the path-tools section. **[audit 2026-10-10: DONE 623145f — fmt clean, clippy -D warnings clean, 2050 passed / 0 failed]**
- [ ] Task: retro; in-app verify user-gated.
