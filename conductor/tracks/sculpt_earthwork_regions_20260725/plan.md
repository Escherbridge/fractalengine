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

- [ ] Task: footprint model (brush + circle/rect/polygon) in petal-local meters;
      cells-inside-footprint test.
- [ ] Task: brush overlay in `fe-renderer` (new brush overlay) + sculpt panel in
      the fe-ui terrain-tools section; calm tactile feedback (§1/§7).

## Phase 2: Sculpt operations (FR-2) [P1]

- [ ] Task: raise/lower/level/smooth over the footprint against the existing
      surface (`mesh/{terrain,interp}.rs`); only-inside-region test; determinism
      test.

## Phase 3: Modification-region node (FR-3) [P1]

- [ ] Task: create a region node via T1 lifecycle carrying footprint + material +
      op/params; addressable (T1 FR-4); re-select re-opens params.
- [ ] Task: delete/revert semantics per ratified Q-2; serialization round-trip.

## Phase 4: Cut/fill volume in real units (FR-4) [P1]

- [ ] Task: heightfield-integration cut/fill against the pre-edit surface, real
      units via `fe-terrain/src/scale.rs`; analytic-case tolerance test; cut vs
      fill separated; correct under non-unit world_scale (N-1).

## Phase 5: Reporting + egress + proposal evolution (FR-5, FR-6) [P1]

- [ ] Task: Proposal/earthwork report shows per-region + total cut/fill +
      material; region data retrievable by address (feeds T5).
- [ ] Task: evolve `terrain_proposal` → region nodes with volume; migrate proposal
      tests; preserve `TerrainProposalAdd/Delete` + guard the proposals-only
      terrain_json shape (ui_shell H-C1 no-regression).

## Phase 6: Docs + integrated sweep [P1]

- [ ] Task: `fe-terrain/src/AGENTS.md` (+ mesh/, layers/) — footprint, sculpt,
      region-node, volume method + tolerance (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests
      (N-6); ui_ux checklist on the sculpt panel.
- [ ] Task: retro; in-app verify user-gated. (Deferred phase: layered strata +
      survey-grade volume — file as a follow-up if pursued.)
