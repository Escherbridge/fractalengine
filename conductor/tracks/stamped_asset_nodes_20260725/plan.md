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

- [ ] Task: make stamp spacing sample the curve at correct arc-length (both
      spacing modes) instead of the flattened polyline; unit test — stamp
      centroids lie on the bezier within tolerance; legacy straight paths
      byte-identical.
- [ ] Task: "Align to path tangent" uses the true curve tangent (not chord);
      tangent test. All geometry in meters (N-1), no `world_scale`.

## Phase 2: Individual selection = addressable node (FR-2) [P1]

- [ ] Task: viewport pick resolves an individual stamp; on first individual
      select, promote via T1 FR-5 → stable address (T1 FR-4).
- [ ] Task: row-count test (N-9) — un-selected stamps add zero store rows.

## Phase 3: Position-locked gimbal + scale/rotate overrides (FR-3) [P1]

- [ ] Task: gimbal for a stamp exposes scale + rotate, disables free translate;
      per-stamp override persists as a sparse record; base transform stays
      path-derived.
- [ ] Task: override-survives-reflow test.

## Phase 4: Instancing + spatial pick at scale (FR-4) [P1]

- [ ] Task: `fe-renderer/src/instancing.rs` — GPU-instanced stamp draw; promoted
      overrides fold into the instanced buffer.
- [ ] Task: spatial pick index for stamps; correctness test (returns the right
      stamp) + a ≥10k-stamp render+pick bench with a documented budget.

## Phase 5: Delete-stamp re-flow (FR-5) [P1]

- [ ] Task: consume T1's reflow event → re-distribute remaining stamps per the
      active spacing mode, preserving other stamps' overrides; invariant test.

## Phase 6: Docs + integrated sweep [P1]

- [ ] Task: `fe-terrain/src/mesh/AGENTS.md` + `fe-renderer/AGENTS.md` — curve
      sampling, override model, instancing/pick (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests
      (N-6); ui_ux checklist on the path-tools section.
- [ ] Task: retro; in-app verify user-gated.
