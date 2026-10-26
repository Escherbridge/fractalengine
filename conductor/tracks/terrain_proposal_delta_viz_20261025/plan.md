---
type: plan
track: terrain_proposal_delta_viz_20261025
title: "Implementation Plan: terrain proposal delta viz + selected-volume report"
created: 2026-10-25
resource: ./spec.md
computed_at_commit: 9165f75
partitions_confidence: verified
---

# Implementation plan: terrain_proposal_delta_viz_20261025

> Citations computed @`9165f75` (S7 review-and-fix). Partitions verified and collision-proofed; all slices launched and complete. This document captures the final plan state after W1–W4 execution.

## 0. Assumptions — D-DV-1..12 ratified (2026-10-26 S8 close-out)

Each decision proceeding on spec §6's recommendation. Reversals costs recorded for future reference.

- **D-DV-1** civil red/blue diverging ramp. *Reversal cost: low* (one pure fn in `terrain_overlay.rs`).
- **D-DV-2** auto ramp max + override. *Low.*
- **D-DV-3** 100 ms debounce, coarse 64 / full 128 lattice. *Low* (constants).
- **D-DV-4** extend the Proposal-report section, renamed "Volume report". *Medium* (moving panels later touches `right_sidebar.rs`).
- **D-DV-5** 2.5D prism. *Medium for P4* (3D needs an elevation band in the request type, which is reserved now).
- **D-DV-6** composite totals + standalone per-item. *Low.*
- **D-DV-7** no endpoint, CSV copy only. *Low.*
- **D-DV-8** both overlays on. *Trivial.*
- **D-DV-9** preview the armed tool's draft. *Medium* (a later Commit/Discard model adds state, but the seam type is unchanged).
- **D-DV-10** reuse the `ProposalOverlay` layer. *Medium* (a new MapLayer touches config names).
- **D-DV-11** palette proposals stay non-nodes. *Low.*
- **D-DV-12** P3 conditional. *n/a.*

## 1. Doctrine for every slice

- **Agents sketch, the orchestrator inks.** Slice agents never run cargo build/test/clippy. The orchestrator runs ONE serial gate per wave end (`CARGO_PROFILE_DEV_DEBUG=0`, `-p fe-terrain -p fe-renderer -p fe-ui` then workspace check), and the full sweep once at track end.
- **NFR-1 is sacred.** Every new fn takes the base as `&impl Fn(f32,f32)->Option<f32>`; `TerrainHeightField` is never written.
- **NFR-5:** fe-ui never imports fe-terrain. Cross-crate types live in `fe-renderer/src/terrain_overlay.rs` (single owner: S1b).
- **Docs convention:** one-line doc comments in code; rationale in the directory `AGENTS.md` section named in each slice.
- **Tests model real flows** (flow names below). Pure-logic tests are table-driven. No tautologies: every assertion must go red if the covered line were inverted.
- **No read-only reviewers:** review slices fix their own findings.

## 2. Phase / slice table

| Phase | Slice | Wave | Model | Task | Owns (files) | Depends |
|---|---|---|---|---|---|---|
| P1 | **S1** | W1 | sonnet | Pure delta lattice + one composite: new `delta_viz.rs` (`DeltaLattice { ghost_positions, drape_positions, dh, indices, cut_fill }` from base + proposed sampler + footprint + step, with a vertex cap); lift private `earthwork_surface_delta` logic into `sculpt::composite_proposed_height` (pure); `for_each_cell` → `pub(crate)` | `fe-terrain/src/delta_viz.rs` (new), `fe-terrain/src/sculpt/mod.rs`, `fe-terrain/src/lib.rs` (`pub mod delta_viz;`), `fe-terrain/src/AGENTS.md` (§delta-viz) | none |
| P1 | **S1b** | W1 | haiku | Seam + styling in fe-renderer: `cut_fill_rgba(dh_m, ramp_max_m, neutral_m)` diverging ramp; `heatmap_material()` (unlit, blended, vertex colors); seam types `ProposalPreview` (Resource: petal_id, op snake, footprint, target, delta, strength, `PreviewSource`), `ProposalPreviewReport` (Message), and **P2's** `VolumeReportRequest` (Message: correlation, petal, footprint, reserved `elevation_band: Option<[f32;2]>`) + `VolumeReportResult` (Message: totals + `Vec<VolumeItem>` + step + scaled) | `fe-renderer/src/terrain_overlay.rs`, `fe-renderer/src/AGENTS.md` (§terrain-overlay) | none |
| P1 | **S2** | W2 | sonnet | Render integration in fe-terrain: `render_terrain_proposals` builds the **lattice ghost** (replaces the perimeter lid, R-1) + **heatmap drape** per committed proposal; new `render_baked_region_heatmaps` (regions vs the pristine field, R-3); new `render_proposal_preview` (reads `ProposalPreview`, debounced per OD-3, coarse/full lattice, writes `ProposalPreviewReport`, sole owner of `ProposalPreviewEntity`); `bake_earthwork_regions` switches to `sculpt::composite_proposed_height` (FR-5); budget + layer binding; register messages/resources | `fe-terrain/src/terrain_plugin.rs`, `fe-terrain/src/terrain_proposal.rs` (deprecate/remove `proposal_overlay_mesh_data`) | S1, S1b |
| P1 | **S3** | W2 | sonnet | fe-ui preview producer: new `terrain_preview.rs` system `publish_proposal_preview` that writes `ProposalPreview` from (a) the armed palette op/radius/delta at the hovered ground point, (b) the sculpt shape draft, (c) the live brush corridor (`stroke_corridor_footprint` over `ActiveStroke` samples, via a read-only accessor), (d) the selected committed proposal (for the readout). Clears on disarm, commit, cancel and petal switch. Live readout line in the Terrain tools section reads `ProposalPreviewReport` | `fe-ui/src/terrain_preview.rs` (new), `fe-ui/src/node_manager/brush_interaction.rs` (accessor only), `fe-ui/src/panels/terrain_tools_panel.rs` (readout line), `fe-ui/src/lib.rs`, `fe-ui/src/plugin.rs`, `fe-ui/src/AGENTS.md` (§terrain-proposal-editor) | S1b |
| **gate W2** | orchestrator | n/a | n/a | Serial build + `-p fe-terrain -p fe-renderer -p fe-ui` tests once; in-app smoke of P1 is user-gated (F5 re-check). S4's tests first compile at gate W3 (S5 adds its `mod` line) | n/a | S2, S3, S4 |
| P2 | **S4** | W2 | sonnet | Pure selection report: new `volume_report.rs`: `selection_volume_report(base, proposals, regions, selection: &Footprint, step, world_scale) -> SelectionReport { area, cut, fill, net, items, overlap, step_m, scaled }` (composite clipped to selection + per-item standalone) | `fe-terrain/src/volume_report.rs` (new) | S1 |
| P2 | **S5** | W3 | sonnet | Answer requests in fe-terrain: `answer_volume_report_requests` reads `VolumeReportRequest`, computes via S4 on the active petal's config + field, writes `VolumeReportResult` (drop wrong-petal / stale correlation); add `pub mod volume_report;` | `fe-terrain/src/terrain_plugin.rs`, `fe-terrain/src/lib.rs`, `fe-terrain/src/AGENTS.md` (§volume-report) | S2, S4 |
| P2 | **S6** | W3 | sonnet | fe-ui Volume report: new `volume_report_panel.rs` (selection source: selected region/proposal or a drawn selection footprint; sends `VolumeReportRequest`; renders totals, legend, per-item table, contained-node list via pure `nodes_in_footprint(petal, footprint)` over `VerseManager`; Copy-as-TSV); `proposal_report_panel.rs` totals switch from area×delta to true figures (region node props + results), R-4; host it in the existing ProposalReport section (OD-4) | `fe-ui/src/panels/volume_report_panel.rs` (new), `fe-ui/src/panels/proposal_report_panel.rs`, `fe-ui/src/panels/mod.rs`, `fe-ui/src/ui_shell/right_sidebar.rs`, `fe-ui/src/plugin.rs` (registration; after S3), `fe-ui/src/panels/AGENTS.md` (§terrain-tools) | S1b, S3 |
| **gate W3** | orchestrator | n/a | n/a | Serial build + crate tests once | n/a | S5, S6 |
| P1+P2 | **S7** | W4 | opus | Review-and-fix (fresh context, `/code-review high`): NFR-1 proofs, one-composite invariant, budget/truncation, throttle allocation, unit honesty, fe-ui→fe-terrain dep absence. Fixes its own findings | `conductor/tracks/terrain_proposal_delta_viz_20261025/review-s7.md` (new) + fix-only edits to any S1-S6 file | S5, S6 |
| P1+P2 | **S8** | W4 | haiku | Docs consolidation: AGENTS.md sections, root `AGENTS.md` §petal-map pointer, tracks.md row, plan ticks | `AGENTS.md`, `fe-terrain/src/AGENTS.md`, `fe-renderer/src/AGENTS.md`, `fe-ui/src/AGENTS.md`, `fe-ui/src/panels/AGENTS.md`, `conductor/tracks.md`, this `plan.md` | S7 |
| **sweep** | orchestrator | n/a | n/a | Full workspace test + clippy `-D warnings` once; in-app verify F5 (user) | n/a | S8 |
| P3 | S9 (deferable) | W5 | sonnet | Pure profile sampler `section.rs` (`section_profile(base, composite, a, b, samples) -> Vec<{station_m, existing, proposed}>`) | `fe-terrain/src/section.rs` (new), `fe-terrain/src/lib.rs` | S1 (OD-12) |
| P3 | S10 (deferable) | W5 | sonnet | Section-line tool + egui painter profile (cut/fill shaded) via a `SectionRequest`/`SectionResult` seam | `fe-renderer/src/terrain_overlay.rs`, `fe-ui/src/panels/section_panel.rs` (new), `fe-terrain/src/terrain_plugin.rs` | S9 |
| P4 | n/a (future) | n/a | n/a | Spec-only (FR-11). Needs the utility node contract first; no slice committed | n/a | n/a |

## 3. Flow tests per slice

Each name is the test to write. Each asserts an observable outcome.

**S1 (`delta_viz.rs`, `sculpt/mod.rs`; table-driven pure)**
- `lattice_ghost_follows_relief_inside_footprint`: Raise 2 over a sloped base; every interior ghost vertex Y == base+2. This is the R-1 regression: the vertex count exceeds the footprint vertex count.
- `heatmap_dh_sign_matches_cut_and_fill`: a Level over a ramp; vertices left of the midline have dh<0, right dh>0.
- `lattice_integrated_volume_equals_cut_fill_volume`: the FR-5 one-truth invariant.
- `composite_applies_regions_in_record_order`: migrated from `earthwork_surface_delta_applies_regions_in_order`.
- `lattice_respects_vertex_cap_and_reports_truncation`.
- `nfr1_base_untouched_by_delta_lattice`.

**S1b (`terrain_overlay.rs`)**
- `cut_fill_ramp_table`: table covering dh = −max, −max/2, −neutral/2 (transparent), 0, +max/2, +max, +2·max (saturates). Red channel dominant for cut, blue for fill.
- `heatmap_material_is_blended_unlit_vertex_colored`.

**S2 (Bevy `App` flows in `terrain_plugin.rs` tests, real `TerrainHeightField`)**
- `committed_raise_renders_lattice_ghost_not_flat_lid`: the user-visible F5 regression.
- `armed_preview_spawns_ghost_and_heatmap_without_db_or_heightfield_write`.
- `preview_report_matches_committed_volume_for_same_inputs`.
- `clearing_preview_despawns_preview_entities`.
- `rapid_preview_edits_coalesce_within_debounce`.
- `baked_region_shows_heatmap_against_pristine_field`.
- `overlays_hidden_when_proposal_layer_hidden`.
- `heatmap_build_stops_at_residency_budget` (extends `proposal_ghosts_stop_at_the_residency_budget_and_grow_with_it`).

**S3 (fe-ui App flows)**
- `armed_palette_publishes_preview_before_add`.
- `brush_stroke_in_progress_publishes_corridor_preview`. The footprint equals what release commits.
- `commit_or_disarm_clears_preview`.
- `petal_switch_clears_preview`.

**S4 (pure, table-driven)**
- `selection_report_clips_overlapping_regions_to_selection`.
- `selection_report_unscaled_never_fabricates_meters`.
- `per_item_sum_equals_total_when_items_disjoint_and_flags_overlap_otherwise`.
- `selection_report_matches_analytic_flat_fill`.

**S5 (App flow)**
- `volume_request_round_trip_answers_matching_correlation`.
- `request_for_inactive_petal_is_dropped`.

**S6 (fe-ui)**
- `selected_region_report_shows_true_cut_fill_not_area_times_delta`: the R-4 regression.
- `nodes_inside_selection_listed_with_kind` (pure `nodes_in_footprint`, table).
- `unscaled_petal_report_shows_world_units_chip`.
- `copy_as_tsv_contains_totals_and_every_item_row`.

**Cut / declined:** no tests for color constants, getters, seam struct construction, or egui layout snapshots.

## 4. Task checklist

### Phase 1: Visible delta
- [x] S1: delta lattice + composite lift (+ §delta-viz doc) — W1 95ac555
- [x] S1b: ramp, heatmap material, seam types for P1+P2 (+ §terrain-overlay doc) — W1 95ac555
- [x] S2: lattice ghost, heatmap drape, baked-region heatmap, live preview system, bake on shared composite — W2 585784b
- [x] S3: preview producer + live readout — W2 585784b
- [x] Gate W2 (orchestrator) — tests passed

### Phase 2: Volume report
- [x] S4: selection report math — W2 585784b
- [x] S5: request/answer system — W3 5bf9883
- [x] S6: Volume report panel + true totals — W3 5bf9883
- [x] Gate W3 (orchestrator) — tests passed

### Close
- [x] S7: review-and-fix (opus) with recorded verdict — W4 9165f75
- [x] S8: docs consolidation — W4 (in progress)
- [ ] Full sweep (orchestrator, once)
- [ ] In-app verify F5 (user-gated): see the cut/fill delta live before commit; select a region and read the true volumes
- [ ] Retro + archive

### Phase 3 (deferable, OD-12)
- [ ] S9 profile sampler
- [ ] S10 section tool + profile panel

### Phase 4 (future)
- [ ] Spin out a utility-node-contract track; reserve only (`VolumeReportRequest.elevation_band`, kind-extensible items)

## 5. Tripwires

- `TerrainHeightField` must stay PRE-BAKE. The heatmap's "existing" surface depends on it, and so do the Q-2 revert and the cut/fill base.
- `render_terrain_proposals` is the sole despawner of `ProposalOverlayEntity`. New preview entities get their own marker and their own sole owner (`render_proposal_preview`). Never add either to the `apply_terrain_assignments` despawn query.
- The preview must not bump `ActivePetalTerrain.revision` or send any `DbCommand`.
- Smooth/Level region previews must go through `sculpt::proposed_height` (Smooth needs `region_mean`). The proposal-seam `Smooth` is identity (`terrain_proposal.rs:219`).
- Brush preview footprints must come from the same `stroke_corridor_footprint` the release commit uses, or preview ≠ commit.
- Keep `terrain_overlay.rs` single-owner (S1b in W1, S10 only in W5).

## 6. Commit strategy

One commit per slice after its wave gate is green: `feat(fe-terrain): S1 delta lattice …`, etc. S7 lands as `fix(...)` commits plus `review-s7.md`. S8 is a `docs(...)` commit. Trailer per the repo's current attribution rule. Pushes are held for user approval (standing directive).
