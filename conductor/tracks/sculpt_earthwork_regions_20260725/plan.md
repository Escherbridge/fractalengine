---
type: Implementation Plan
title: "Implementation Plan: Sculpt & Earthwork Region Nodes"
tags: [sculpt_earthwork_regions_20260725]
resource: ./spec.md
---

# Implementation Plan: Sculpt & Earthwork Region Nodes

Six phases. Area selection + sculpt ops first (the tactile win), then the region
node + volume (the BIM value), then reporting/egress + evolving the proposal
system. TDD on the footprint/sculpt/volume math; single sweep at the end (N-6).

## Phase 1: Area-selection tools (FR-1) [P0]

- [x] Task: footprint model (brush + circle/rect/polygon) in petal-local meters;
      cells-inside-footprint test. **[audit 2026-10-10: DONE e96fa75 — fe-terrain/src/sculpt/mod.rs; tests footprint_contains_matches_shape, rect_bounds_normalize_swapped_corners, disc_to_polygon_is_closed_ring_within_radius]**
- [x] Task: brush overlay in `fe-renderer` (new brush overlay) + sculpt panel in
      the fe-ui terrain-tools section; calm tactile feedback (§1/§7). **[audit 2026-10-10: DONE e96fa75+195f5cc+118e666 — `brush_ring`/`brush_overlay_positions` fe-renderer/src/terrain_overlay.rs:74-90; cursor ring fe-ui/src/sculpt_cursor.rs; brush promoted to a first-class tool fe-ui/src/node_manager/brush_interaction.rs (118e666)]**

## Phase 2: Sculpt operations (FR-2) [P1]

- [x] Task: raise/lower/level/smooth over the footprint against the existing
      surface (`mesh/{terrain,interp}.rs`); only-inside-region test; determinism
      test. **[audit 2026-10-10: PARTIAL (consolidator re-check: un-ticked — the task's required determinism test is absent) — ops DONE e96fa75 — tests apply_gates_outside_and_applies_inside, level_at_full_strength_is_absolute_lower_is_relative, smooth_relaxes_toward_region_mean, nfr1_base_sampler_unmutated_after_apply_and_volume (sculpt/mod.rs). Determinism: ops are pure RNG-free fns but no dedicated determinism test exists (ledger)]** **[DONE 2026-10-11 spatial-builder-residue batch (commit pending): `sculpt_ops_are_bit_identical_on_repeat` (fe-terrain/src/sculpt/mod.rs) — every op x Rect/Circle/Polygon x world_scale {1.0, 0.01}: apply_sculpt heights, sculpt_cut_fill and EarthworkRegion::cut_fill compared via to_bits() across repeat runs]**

## Phase 3: Modification-region node (FR-3) [P1]

- [x] Task: create a region node via T1 lifecycle carrying footprint + material +
      op/params; addressable (T1 FR-4); re-select re-opens params. **[audit 2026-10-10: DONE 195f5cc — earthwork regions become node rows (node_kind=earthwork_region + material + cut/fill); tests earthwork_node_properties_carry_the_endpoint_contract, brush_commit_persists_region_and_creates_endpoint_node, hydrate_earthwork_region_binds_and_seeds_gate (fe-ui/src/actions/terrain_proposal.rs); EditRegionParams verb context_menu.rs:281]**
- [x] Task: delete/revert semantics per ratified Q-2; serialization round-trip. **[audit 2026-10-10: DONE (handler) — handle_delete_region actions/terrain_proposal.rs:1222 drops the record + tombstones the node; tests remove_region_reverts_by_dropping_the_record, region_json_round_trips_with_material_default. RESIDUAL: `UiAction::SculptDeleteRegion` has no UI producer (only the dispatch arm actions/mod.rs:1062) — see ledger]** **[RESIDUAL CLOSED 2026-10-11 spatial-builder-residue batch (commit pending): right-click on a committed region classifies as `HitTarget::EarthworkRegion` (node_manager/context_pick.rs `pick_ground_region` over material-tagged terrain-doc entries) and its Delete confirm routes to `SculptDeleteRegion` (dialogs/context_menu.rs `proposal_delete_action`); tests ground_overlay_delete_routes_by_overlay_kind, pick_ground_region_hits_only_material_tagged_entries, delete_region_drops_only_its_record_then_tombstones_its_node]**

## Phase 4: Cut/fill volume in real units (FR-4) [P1]

- [x] Task: heightfield-integration cut/fill against the pre-edit surface, real
      units via `fe-terrain/src/scale.rs`; analytic-case tolerance test; cut vs
      fill separated; correct under non-unit world_scale (N-1). **[audit 2026-10-10: DONE e96fa75 — tests cut_fill_flat_fill_matches_analytic_scaled, cut_fill_separates_cut_and_fill_over_relief, cut_fill_unscaled_reports_scene_units_never_fake_meters, region_cut_fill_uses_footprint_and_scale]**

## Phase 5: Reporting + egress + proposal evolution (FR-5, FR-6) [P1]

- [x] Task: Proposal/earthwork report shows per-region + total cut/fill +
      material; region data retrievable by address (feeds T5). **[audit 2026-10-10: DONE e96fa75+195f5cc+e5688ca — fe-ui/src/panels/proposal_report_panel.rs; EarthworkVolumeReport seam fe-renderer/src/terrain_overlay.rs:107; REST GET /api/v1/petals/:petal_id/earthwork/summary fe-api/src/endpoint.rs:451]**
- [x] Task: evolve `terrain_proposal` → region nodes with volume; migrate proposal
      tests; preserve `TerrainProposalAdd/Delete` + guard the proposals-only
      terrain_json shape (ui_shell H-C1 no-regression). **[audit 2026-10-10: DONE — regions ride the same terrain.proposals block (fe-terrain/src/terrain_proposal.rs earthwork_region_predicate_requires_a_valid_bake_record); H-C1 guards embed_none_base_doc_carries_every_field_terrain_config_requires, embed_proposals_hydrated_palette_delete_never_removes_a_region_entry (actions/terrain_proposal.rs)]**

## Phase 6: Docs + integrated sweep [P1]

- [x] Task: `fe-terrain/src/AGENTS.md` (+ mesh/, layers/) — footprint, sculpt,
      region-node, volume method + tolerance (N-7). **[audit 2026-10-10: DONE — fe-terrain/src/AGENTS.md §sculpt (:440) + §Earthwork bake (:504) + §earthwork-validity-and-strength (:6)]**
- [x] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests
      (N-6); ui_ux checklist on the sculpt panel. **[audit 2026-10-10: DONE 195f5cc — fmt clean, clippy -D warnings clean, 2050 passed / 0 failed]**
- [x] Task: retro; in-app verify user-gated. **[retro.md written 2026-10-11 spatial-builder-residue batch (commit pending); in-app verify remains USER-GATED (not self-signed; track status stays implemented_pending_inapp_verify)]** (Deferred phase: layered strata +
      survey-grade volume — file as a follow-up if pursued.)
