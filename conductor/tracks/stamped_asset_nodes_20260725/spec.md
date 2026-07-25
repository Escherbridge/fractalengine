---
type: Track Spec
title: Stamped-Asset Nodes — Curve-Follow, Individual Selection, Scale/Rotate Overrides, Instancing at Scale
description: Turn stamped path assets into individually addressable nodes (D-A5) — position locked to and following the actual curve (not the flattened polyline), with per-stamp scale + rotation overrides and free translate disabled — backed by GPU instancing + a spatial pick index so tens of thousands of stamps stay smooth (D-A6). Consumes the node spine's lazy-promotion and path-reflow lifecycle (T1). Wave 1.
tags: [feature, stamped_asset_nodes_20260725, pending]
timestamp: 2026-07-25T00:00:00Z
resource: ./metadata.json
---

# Specification: Stamped-Asset Nodes

**Track ID:** `stamped_asset_nodes_20260725`
**Type:** feature · **Wave:** 1 · **depends_on:** `node_lifecycle_addressing_20260725`
**Crates:** `fe-terrain` (mesh/curve,track,marker + stamp materializer), `fe-renderer`
(loader/ingester/viewport + new `instancing.rs`), `fe-ui` (path-tools section content only)

Anchor: [`../../decisions/spatial-builder-program-20260725.md`](../../decisions/spatial-builder-program-20260725.md).
Foundation: [`../node_lifecycle_addressing_20260725/spec.md`](../node_lifecycle_addressing_20260725/spec.md)
(FR-4 address, FR-5 promotion, FR-2 reflow event), [`../pen_curve_tool_20260722/spec.md`](../pen_curve_tool_20260722/spec.md)
(the bezier/curve model stamps must sample).

## Overview

Verbatim user asks (2026-07-25 in-app QA):

1. > "It would be nice to select stamped assets individually and treat them as
   > their own nodes — disable their location but allow for scale and rotation."
2. > "The stamped assets also don't follow the curved paths."

Per D-A5 each stamp becomes a full addressable node whose **position is derived
from the path** (locked, path-following) but whose **scale + rotation are
per-stamp overridable**. Per D-A6 + N-9 this must scale to tens of thousands, so
stamps render via GPU instancing and only *promote* to full nodes when
individually addressed (T1 FR-5) — the data model is full-node, the hot path is
instanced.

### Ground truth (2026-07-25)

- Stamp placement/spacing runs through the stamp materializer
  (`fe-ui actions/path.rs` / `actions/asset.rs`, `PathAssetApply` against
  `PathEditorState.editing_track_id`) and `fe-terrain/src/mesh/curve.rs`.
- The **curve-follow bug**: stamps space along the flattened polyline, not the
  bezier/Catmull-Rom curve the pen tool produces — so on curved segments they
  drift off the visible path. The curve sampler exists (pen_curve `flatten_route`
  / `mesh/curve.rs`); stamp spacing + the "Align to path tangent" option must
  sample the *curve* at correct arc-length, not the chords.
- Positions are raw petal-local **meters** — no `world_scale` multiply (N-1; the
  2026-07-19 ribbon regression is the precedent).

## Functional Requirements

- **FR-1 — Stamps follow the curve.** Stamp placement (both spacing modes) and
  the "Align to path tangent" option sample the actual curve at correct
  arc-length with the true curve tangent, so stamps sit on the visible path
  through curved segments. *Acceptance:* on a curved bezier path, every stamp
  centroid lies on the rendered curve within tolerance (unit test on the
  sampler); tangent-aligned stamps orient to the curve tangent, not the chord;
  legacy straight paths are byte-identical.

- **FR-2 — Individual selection = addressable node.** Clicking a stamp selects
  it as an individual node (promoting it via T1 FR-5 on first individual
  select). The node is addressable (T1 FR-4) and therefore queryable/egress-able
  (N-10, via T5). *Acceptance:* clicking one of N stamps selects exactly that
  stamp; it resolves to a stable address; un-selected stamps add no per-instance
  store rows (N-9 row-count test).

- **FR-3 — Position locked; scale + rotation overridable.** A selected stamp's
  gimbal exposes **scale + rotate** but **not free translate** — position stays
  path-derived. Per-stamp scale/rotation persist as a sparse override on the
  node; the base transform stays path-following. *Acceptance:* dragging scale/
  rotate updates + persists the override; free-translate handles are absent/
  inert; re-flowing the path preserves the stamp's overrides.

- **FR-4 — Instancing + spatial pick at scale (D-A6, N-9).** Un-promoted stamps
  render via GPU instancing (new `fe-renderer/src/instancing.rs`) and pick via a
  spatial index; promoted stamps carry their overrides into the instanced draw.
  Target: tens of thousands of stamps stay interactive. *Acceptance:* a scene
  with ≥10k stamps renders + picks within budget (documented bench); picking
  returns the correct individual stamp; overrides survive instancing.

- **FR-5 — Delete-stamp re-flow.** Deleting an individual stamp (via T4's menu →
  T1's delete) triggers path re-flow (consumes T1 FR-2's reflow event): the
  remaining stamps re-distribute along the curve. *Acceptance:* deleting a
  mid-path stamp re-distributes the rest per the active spacing mode; other
  stamps' overrides are preserved; count/spacing invariants hold (unit test).

## Non-Functional Requirements

Inherits the shared pool. Load-bearing: **N-1** (meters only, no `world_scale`
in geometry — display formatting only), **N-9** (data/render split; no per-
instance store cost until promotion), **N-3** (path surfaces key ONLY on
`PathEditorState.editing_track_id`), **N-10** (promoted stamps are reportable).
No new fe-ui→fe-terrain crate dependency (mirror enums + JSON contract per the
ui_shell NFR-2 precedent).

## Dependencies & concurrency

- **depends_on:** `node_lifecycle_addressing_20260725` (promotion FR-5, address
  FR-4, reflow FR-2). **blocks:** none.
- **Owns (file partition):** `fe-terrain/src/mesh/{curve,track,marker}.rs` +
  stamp materializer (`fe-ui actions/{asset,path}.rs`); `fe-renderer/src/
  {loader,ingester,viewport}.rs` + new `instancing.rs`; the fe-ui **path-tools
  section content module** (NOT `right_sidebar.rs` — that seam is T6's).
- Disjoint from T3 within fe-terrain (curve/track/marker vs terrain/interp/skirt)
  and fe-renderer (loader/viewport vs terrain_overlay). Runs Wave 1 parallel.

## Open questions (ratify before build)

- **Q-1 — Slide-along-path.** With free translate off, allow a distinct 1-D
  "slide along the path" affordance to reposition a stamp by arc-length
  (recommended — keeps repositioning without breaking the path lock), or fully
  fixed until re-stamp?
- **Q-2 — Promotion granularity.** Promote per-stamp on individual select
  (recommended, cheapest) or promote all of a path's stamps at once when the
  path enters edit mode?
- **Q-3 — Override precedence.** Per-stamp override wins over a path-level
  default scale/rotation (recommended); path default applies to un-overridden
  stamps.
- **Q-4 — Tangent default.** Keep "Align to path tangent" defaulted OFF (current)
  but fixed to use the true curve tangent (recommended), or default it ON now?

## Ratified decisions (2026-07-25)

User ratified 2026-07-25 (Q-1 asked; Q-2..Q-4 recommended defaults adopted).

- **Q-1 → RATIFIED: allow a distinct 1-D "slide along the path" affordance.** A
  selected stamp can be repositioned by arc-length along its curve without
  breaking the path lock (free translate stays off). Gates FR-3's gimbal scope.
- **Q-2 → RATIFIED: promote per-stamp on individual select** (cheapest; matches
  T1 Q-2). Not whole-path-at-once on edit-mode entry. Gates FR-2.
- **Q-3 → RATIFIED: per-stamp override wins over a path-level default** scale/
  rotation; path default applies only to un-overridden stamps. Gates FR-3.
- **Q-4 → RATIFIED: keep "Align to path tangent" defaulted OFF, but fixed to use
  the true curve tangent** (not the chord). Not defaulted ON. Gates FR-1.

## Out of scope

- The node lifecycle/promotion/reflow primitives themselves (T1 owns them).
- The right-click menu that triggers delete/duplicate on a stamp (T4).
- The query/egress endpoint over a stamp node's address (T5 exposes it).
- Non-stamp GLB placement changes; terrain sculpting (T3).
