---
type: Track Spec
title: Sculpt & Earthwork Region Nodes — Brush/Shape Area Selection, Cut/Fill Volume, Reportable BIM Regions
description: Add a tactile sculpt tool (brush + defined shapes) to select the affected terrain area the way the user wants, and turn each earthwork edit into a persistent, addressable "modification region" node (footprint shape + material + computed cut/fill volume in real units) that bakes into the surface and is reportable/queryable like any node (D-A8). The first BIM-grade terraforming primitive; layered strata is a later phase. Consumes the node spine (T1). Wave 1.
tags: [feature, sculpt_earthwork_regions_20260725, pending]
timestamp: 2026-07-25T00:00:00Z
resource: ./metadata.json
---

# Specification: Sculpt & Earthwork Region Nodes

**Track ID:** `sculpt_earthwork_regions_20260725`
**Type:** feature · **Wave:** 1 · **depends_on:** `node_lifecycle_addressing_20260725`
**Crates:** `fe-terrain` (terrain_proposal + mesh/{terrain,interp,skirt} + layers + new sculpt module),
`fe-renderer` (terrain_overlay/terrain_height + new brush overlay), `fe-ui` (terrain-tools section content + new sculpt panel)

Anchor: [`../../decisions/spatial-builder-program-20260725.md`](../../decisions/spatial-builder-program-20260725.md).
Foundation: [`../node_lifecycle_addressing_20260725/spec.md`](../node_lifecycle_addressing_20260725/spec.md)
(region = ordinary node), the terrain scale authority (`fe-terrain/src/scale.rs`),
and the existing non-destructive proposal direction (terrain_editor_overhaul).

## Overview

Verbatim user asks (2026-07-25 in-app QA):

1. > "The terrain tools will need some sort of brush tool to actually select the
   > affected area in the ways we want."
2. > "Proposals should essentially give volume data of how much material is
   > removed or added. It would be nice if it was an actual shape you could
   > report on — basically we need a new sculpt tool and more BIM-like features
   > for the terraforming."

Per D-A8 an earthwork edit is a **persistent, addressable modification-region
node**: a footprint shape + material + a computed cut/fill **volume in real
units**, editable as an object and baked into the surface, reportable/queryable
like any node (N-10). The civil-engineer accuracy comes from the real-world
scale authority (N-1), not a separate mode (D-A3). This is BIM depth level
"defined shapes + real cut/fill"; the full **layered strata** model (R1Q4) is a
later phase within this track, not the first landing.

### Ground truth (2026-07-25)

- Terrain proposals exist today: `fe-terrain/src/terrain_proposal.rs`,
  `fe-ui actions/terrain_proposal.rs` (`TerrainProposalAdd/Delete`,
  `embed_proposals`), surfaced in the right-sidebar Terrain-tools + Proposal
  report sections (ui_shell FR-9). Today they lack area selection + volume.
- Terrain surface + interpolation: `fe-terrain/src/mesh/{terrain,interp,skirt}.rs`;
  layer stack: `fe-terrain/src/layers/*`; height query for volume math:
  `fe-renderer/src/terrain_height.rs`; overlay draw: `terrain_overlay.rs`.
- Real-world scale authority for real-unit volumes: `fe-terrain/src/scale.rs`
  (hexon-authoritative scale, TilesetMeta — hexon_scale_orchestration).

## Functional Requirements

- **FR-1 — Area-selection tools (brush + shape).** A sculpt tool lets the user
  define the affected area both tactilely (freeform brush) and precisely
  (defined footprint shapes — circle / rectangle / polygon). The defined shape
  is what makes the region "an actual shape you can report on" (D-A8).
  *Acceptance:* brush and each shape produce a well-formed footprint in
  petal-local meters (N-1); the affected cells are exactly those inside the
  footprint (unit test); Cities:Skylines-tactile feedback (calm overlay, §1/§7).

- **FR-2 — Sculpt operations over the region.** Raise / lower / level (flatten
  to target) / smooth applied within the region against the existing surface.
  *Acceptance:* each op modifies only cells inside the footprint; deterministic
  result given (surface, footprint, op, params); no edit outside the region.

- **FR-3 — Modification-region node (D-A8).** Each edit becomes a persistent,
  addressable node (created via T1 lifecycle) carrying: footprint shape,
  material tag, op + params, and the computed volume. It bakes into the surface
  but remains an editable, addressable object. *Acceptance:* a region resolves
  to a stable address (T1 FR-4); re-selecting it re-opens its params; deleting
  it (T1 delete) reverts its baked contribution or tombstones the region
  (per Q-2); survives serialization.

- **FR-4 — Cut/fill volume in real units.** Compute true added/removed volume of
  the region against the pre-edit surface (heightfield integration), expressed
  in real-world units via the scale authority (N-1). *Acceptance:* volume within
  documented tolerance of an analytic case (e.g. a flat-level over known relief);
  cut vs fill reported separately; units correct under a non-unit `world_scale`.

- **FR-5 — Reporting + egress (N-10).** The Proposal/earthwork report shows each
  region's cut/fill volume and material; the region node is queryable + egress-
  able through T5 (its address carries the volume/shape/material as data).
  *Acceptance:* the report lists per-region and total cut/fill; a region's data
  is retrievable by address; egress string/endpoint returns the quantities.

- **FR-6 — Evolve the proposal system, don't fork it.** Extend the existing
  `terrain_proposal` path so proposals become region nodes with volume; preserve
  the `TerrainProposalAdd/Delete` action contract + `compute_report` where it
  still applies. *Acceptance:* existing proposal tests migrated/green; the
  Terrain-tools + Proposal-report sections show the new area + volume UI; no
  regression to the proposals-only-terrain_json hardening (ui_shell H-C1).

## Non-Functional Requirements

Inherits the shared pool. Load-bearing: **N-1** (real-unit volumes via the scale
authority; geometry in meters), **N-10** (regions are reportable/queryable),
**N-8** (ui_ux checklist on the sculpt panel + overlays). No new fe-ui→fe-terrain
crate dependency (mirror enums + JSON contract). Volume math accuracy is
**planning-grade** with a documented method; survey-grade is a follow-up.

## Dependencies & concurrency

- **depends_on:** `node_lifecycle_addressing_20260725` (region = node; delete/
  address). **blocks:** none.
- **Owns (file partition):** `fe-terrain/src/terrain_proposal.rs`,
  `mesh/{terrain,interp,skirt}.rs`, `layers/*`, new `sculpt` module;
  `fe-renderer/src/{terrain_overlay,terrain_height}.rs` + new brush overlay;
  fe-ui **terrain-tools section content** + new sculpt panel (NOT
  `right_sidebar.rs`). Disjoint from T2 within fe-terrain/fe-renderer.

## Open questions (ratify before build)

- **Q-1 — Brush vs shape scope this round.** Ship **both** brush + defined
  shapes now (recommended — brush for feel, shape for the reportable region per
  D-A8), or brush-only first with shapes as a fast follow?
- **Q-2 — Delete semantics for a baked region.** Deleting a region node
  **reverts** its baked surface contribution (recommended — treats the region as
  the source of truth) or leaves the bake and just tombstones the record?
- **Q-3 — Layered strata depth.** Single-material volume + material tag this
  landing, with per-layer strata/materials as a later phase in this track
  (recommended, matches R1Q4 "multi-track effort"), or attempt strata now?
- **Q-4 — Volume method.** Heightfield prism/grid integration against the
  pre-edit surface, planning-grade with a documented tolerance (recommended), or
  hold for a survey-grade method?

## Ratified decisions (2026-07-25)

User ratified 2026-07-25 (Q-1/Q-2 asked; Q-3/Q-4 recommended defaults adopted).

- **Q-1 → RATIFIED: ship both brush + defined shapes this landing.** Brush for
  feel, defined footprint shapes (circle/rect/polygon) for the reportable region
  (D-A8 needs a shape to report on). Gates FR-1.
- **Q-2 → RATIFIED: deleting a region reverts its baked surface contribution.**
  The region is the source of truth (cleaner BIM model, undo-friendly);
  requires storing the pre-edit delta. Not leave-bake-and-tombstone. Gates FR-3.
- **Q-3 → RATIFIED: single-material volume + material tag this landing;** per-layer
  strata is a later phase within this track (matches R1Q4). Gates FR-3/FR-4 scope.
- **Q-4 → RATIFIED: heightfield prism/grid integration against the pre-edit
  surface, planning-grade with a documented tolerance.** Survey-grade is a
  follow-up. Gates FR-4.

## Out of scope

- The node lifecycle/address/delete primitives (T1).
- The right-click menu that reports/deletes a region (T4 wires the verbs; the
  report content is FR-5 here).
- The public query/egress endpoint plumbing (T5; this track supplies the data).
- Full non-destructive live adjustment stack (D-A8 chose baked; deferred).
- Survey-grade volume + per-layer strata export (later phases/follow-up).
