---
type: Implementation Plan
title: "Implementation Plan: Unified Spatial Editor + Analytics-First Terrain Proposals"
tags: [terrain_editor_overhaul_20260718]
resource: ./spec.md
---

# Implementation Plan: Unified Spatial Editor + Analytics-First Terrain Proposals

## Overview

Sequenced so the two bounded QA bugs (FR-3, FR-4) land first as quick wins, then
the selection/dispatch spine (FR-1, FR-2) that the terrain editor needs, then the
non-destructive terrain proposal system (FR-5) and its reporting payoff (FR-6).
TDD throughout; single workspace sweep at the very end per standing policy
(`RUST_MIN_STACK=134217728 cargo test --workspace -j2`).

## Phase 0: QA bug fixes (FR-3 gimbal-on-path, FR-4 cascade-delete) — decision-light

- [ ] Task: FR-4 cascade-delete — route `DbResult::NodeDeleted` (fe-ui arm,
      `db_results/mod.rs:151` / `handle_node_deleted` `nodes.rs:106`) to
      `PathAssetCache::invalidate(node_id)` + `PathAssetApplied::invalidate(node_id)`
      (TDD: delete a stamped track → assert every `PathAssetInstance` with that
      `source_track_id` is despawned AND no resurrection on petal re-entry;
      no-op/idempotent for stampless tracks, NFR-5)
- [ ] Task: FR-3 gimbal-on-path — teach `draw_gimbal_system`/`gimbal_center` to
      resolve a target transform from the FR-1 read-model (interim: read
      `PathEditorState.selected_point/segment` directly if FR-1 not yet in) and
      ungate the gimbal for path selections in Select/Pen (TDD: pure
      target-resolution fn; gimbal appears for vertex/segment/track selection)
- [ ] Verification: manual + unit — deleting stamped paths cleans up glTF;
      selecting a path vertex shows a draggable gimbal [checkpoint]

## Phase 1: Typed selection read-model + object-type-aware dispatch (FR-1, FR-2)

- [ ] Task: `SelectionKind` read-model resource projected each frame from
      `NodeManager.selected` + `PathEditorState.*` (NO storage merge — ui_ux.md §5;
      TDD: projection truth-table across all authority states, mutual exclusion)
- [ ] Task: Fold Phase-0 FR-3's interim target-resolution onto the read-model
      (remove the direct `PathEditorState` read once the projection exists)
- [ ] Task: `Operation` dispatch table keyed on `(Tool, SelectionKind, hit-type)`;
      wire into the `ClickArbiter` chain preserving first-claim-wins + viewport
      gating (TDD: pure op-selection fn; ownership/arbitration unchanged)
- [ ] Verification: left-click does the object-appropriate op for node / vertex /
      segment / stamp; road_builder_ux seam noted for shared dispatch [checkpoint]

## Phase 2: Proposed-overlay terrain data model + rendering (FR-5 core) — NON-destructive

- [ ] Task: `TerrainProposal` record model (op kind, footprint polygon, target
      height/delta, params) + a `MapLayer::ProposalOverlay` parallel to the
      existing display-overlay layers; NEVER writes `TerrainHeightField`/tileset
      (TDD: proposal apply is pure geometry over a sampled base, ground truth
      untouched — assert the heightfield is byte-identical after a proposal)
- [ ] Task: Ghosted/tinted overlay material in fe-renderer that renders proposed
      geometry atop true terrain; reads `TerrainHeightField` read-only for the base
- [ ] Task: Additive persistence via `SetPetalTerrain` proposals block (round-trip
      that does not clobber tileset/layer config — mirror the GIS layer round-trip
      pattern; TDD: save→load→save proposal determinism)
- [ ] Verification: place a raise/flatten proposal → renders as a distinct overlay,
      true terrain unchanged, survives petal reload [checkpoint]

## Phase 3: Edit tools / brushes producing proposals (FR-5 tools)

- [ ] Task: C:S-style tool palette in the "Terrain Tools" panel (replaces the
      placeholder `tool_panel.rs:601-613`): raise/lower/flatten/ramp/slope + pad +
      cut/fill volume, each emitting a `TerrainProposal` via the FR-2 dispatch
- [ ] Task: Proposal selection + edit + delete through the FR-1 read-model
      (`SelectionKind::TerrainProposal`); gimbal on a selected proposal (FR-3 path)
- [ ] Verification: each brush produces a correct, selectable, editable proposal;
      proposal meshes respect the mesh budget (NFR-2) [checkpoint]

## Phase 4: Measurement + scale/geometry reporting (FR-6)

- [ ] Task: Wire `polygon_area_m2`/`world_to_real_distance`/`bearing_deg` into a
      report panel on any selected proposal/geometry — extent (m), area (m²),
      cut/fill volume (m³), slope (%), bearing (°); honest "no map scale" state
      (TDD: metric conversions vs known fixtures; unscaled → world-units + chip)
- [ ] Task: Interactive tape/area/bearing measure tools (coordinate scope with
      hexon_scale_orchestration Phase 5 — decide subsumption at phase start)
- [ ] Verification: report matches hand-computed values for a known proposal at a
      known world_scale; unscaled petal reports world units, never fake meters
      [checkpoint]

## Phase 5: Close-out

- [ ] Task: Single end-of-track workspace sweep (test/clippy -D warnings/fmt)
- [ ] Task: Update `fe-ui/src/AGENTS.md` (selection read-model, dispatch table) +
      `fe-terrain/src/AGENTS.md` (proposal overlay, reporting) per the
      directory-doc convention
- [ ] Task: Retro + archive per track-per-feature workflow; note RBAC + hexon
      proposal-layer follow-ups if deferred
