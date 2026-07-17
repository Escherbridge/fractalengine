---
type: Track Spec
title: Path Interaction — precise picking, vertex/segment selection, whole-path gimbal, width default
tags: [feature, bug, ui, gpx, pen-tool, picking, gimbal, path_interaction_20260716]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Specification: Path Interaction Overhaul

**Track ID:** `path_interaction_20260716`
**Crates:** `fe-ui`, `fe-terrain`, `fractalengine`

## Vision / Why (user UX testing, 2026-07-16)

"Individual path nodes are not selectable or moveable — it should be similar to
Illustrator's pen tool"; "the path should not be so wide"; "the path is also too
selectable — can select other objects around it"; "I see no gimbal controls for
the path and I would need all of them — it should be similar to a group of other
assets"; "each segment on the gpx needs to allow for selection as well and show
measurement on selection [in real metric units]".

## Root causes (investigated 2026-07-16)

- Ribbon picking is AABB-only (`viewport_pick.rs:161`) — a km-scale flat box
  swallows clicks meant for nearby objects.
- Vertex drag exists but is Pen-tool-gated (`path_point_interaction.rs:179-183`);
  no `selected_point` state, no highlight.
- Ribbon mesh is world-space with identity transform, so the gimbal has no
  anchor; `transform_broadcast` would write a node transform instead of points.
- Default ribbon `TrackStyle` is 2.0 world units wide (`animation.rs:129`).

## Functional Requirements

- **FR-1 — Precise ribbon picking.** Narrow-phase ray-vs-segment distance test
  (≤ half_width + slop) on the actual polyline replaces AABB-hit for track
  lines; nearby objects become pickable again.
- **FR-2 — Vertex select + move in Select tool.** Clicking a vertex marker
  selects it (highlight); drag moves it (existing `PathMovePoint` persistence).
  Works in Select and Pen tools while a track is open for editing.
- **FR-3 — Segment selection + metric measurement.** While editing, clicking the
  ribbon between two vertices selects that segment, highlights it, and shows its
  real length (m/km via `PetalMapState.world_scale`); edit view also shows total
  path length.
- **FR-4 — Whole-path gimbal (move/rotate/scale).** Ribbon mesh becomes
  centroid-anchored; Move/Rotate/Scale gimbal renders at the centroid and
  commits by baking the transform delta into all `gpx_points` (persisted via
  path ops), never a node transform.
- **FR-5 — Sane width default.** Default track width 0.5 world units (both
  fe-terrain default and fe-ui field default); thickness slider range widened
  down to 0.1.
