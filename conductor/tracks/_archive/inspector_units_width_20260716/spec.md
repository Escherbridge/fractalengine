---
type: Track Spec
title: Inspector — panel width blow-out fix + real-unit transform inputs
tags: [bug, feature, ui, inspector, units, inspector_units_width_20260716]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Specification: Inspector Width + Real Units

**Track ID:** `inspector_units_width_20260716`
**Crates:** `fe-ui`

## Vision / Why (user UX testing, 2026-07-16)

"The side panel is way too huge — custom properties being filled in all the way
pushes out the space it takes up significantly. We should not allow direct edit
but have a copyable input field that shrinks to the normal default side bar
size"; "the asset size, position and rotation inputs should also use real
measurements."

## Root causes (investigated 2026-07-16)

- Custom-property values render as an unbounded non-wrapping `ui.label`
  (`inspector.rs:819-824`); a giant `gpx_points` JSON forces the grid column —
  and the panel (width_range up to 80% of viewport) — enormous. Values are
  already read-only; there is no in-place editor to remove.
- Transform inputs are raw world units / radians with no unit conversion.

## Functional Requirements

- **FR-1 — Width-stable property values.** Property values render as read-only,
  copyable, width-constrained fields (select-to-copy + copy affordance); the
  panel stays at its default width regardless of value size. Delete + Add
  Property flows unchanged.
- **FR-2 — Real-unit transform inputs.** Position edits in meters (convert via
  `PetalMapState.world_scale`; real = world / scale, degenerate scale → 1.0),
  rotation in degrees, and asset size shown/edited in real meters derived from
  the selected node's AABB × scale (falls back to bare multiplier when no AABB).
  Apply converts back to world units/radians.
