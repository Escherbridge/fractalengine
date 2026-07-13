---
type: Track Spec
title: Pen Tool — Polyline, Curves, Sensitivity, Shapes
tags: [feature, ui, gpx, pen-tool, curves, pen_tool_curves_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Pen Tool

**Track ID:** `pen_tool_curves_20260713`
**Crates:** `fe-ui` (+ possibly `fe-terrain` for curve sampling)
**Work units:** W7a (polyline pen — parallel wave), W7b (curves/shapes —
sequential)

## Vision (user, 2026-07-13)

Transform the clunky cursor-driven "Append from cursor" point placement into
a proper **pen tool** with curve abilities: adjustable sensitivity toggles
(sharp angles ↔ smooth curves), Bezier/spline curves, and shape primitives
(ellipses, shapes). NOT a deletion — a substantial new drawing tool.

## Functional Requirements

- **FR-1 (W7a):** `Tool::Pen` variant + `P` hotkey. Pen mode active + click on
  the viewport = append a control point (raycast to y=0), giving click-to-place
  polyline drawing that persists to `gpx_points`. Reuses the existing
  `PathAppendPoint` action + marker/line rendering.
- **FR-2 (W7a):** Remove the "Append from cursor" button; replace with a hint
  pointing to the Pen tool. Preserve move/annotate on existing points.
- **FR-3 (W7b):** Bezier / Catmull-Rom curves via Bevy 0.18's built-in
  `CubicCurve` family (present in the dep, unused today). Curve → polyline
  sampling that writes `gpx_points`.
- **FR-4 (W7b):** Sensitivity/tension toggle — sharp-vs-smooth interpolation.
- **FR-5 (W7b):** Shape primitives (ellipse/circle) generated as `gpx_points`
  point sets.

## Reusable vs. build-new (recon 2026-07-13)

- **Reuse:** camera ray (`camera.viewport_to_world`), y=0 projection
  (`ray_plane_y`), marker rendering (`sync_path_point_markers`), line
  rendering (`render_gpx_tracks` LineStrip), point persistence
  (`PathAppendPoint` → `gpx_points`), arc-length engine (`PathTracker`, linear),
  input idiom (`ButtonInput`, `Tool`/`ToolState`).
- **Build-new (W7b):** all curve math (no spline code exists), curve→polyline
  sampling, ellipse/shape generation, and any terrain-height-aware placement
  (both raycasts hit y=0, not terrain).

## Status

W7a (polyline pen) lands in the parallel green wave. W7b (curves/sensitivity/
shapes) is sequential, built on green; may be handed off if runway runs out.
