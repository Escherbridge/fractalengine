---
type: Track Spec
title: Camera — stale focus jump + near-plane clipping on close zoom
tags: [bug, camera, renderer, camera_focus_clip_20260716]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Specification: Camera Focus + Clip Fixes

**Track ID:** `camera_focus_clip_20260716`
**Crates:** `fe-renderer`, `fe-ui`, `fe-database`

## Vision / Why (user UX testing, 2026-07-16)

"The camera often clips after asset placements — zooming in to a default area
then going to where it should be, or simply zooming in on one of the duck glbs."

## Root causes (investigated 2026-07-16)

- Focus targets the sidebar's cached `NodeEntry.position`, hardcoded `[0.0;3]`
  on create (`db_results/nodes.rs:68`) because `DbResult::NodeCreated` omits the
  position — so fresh nodes focus to origin first, then the real spot after a
  hierarchy reload (two-step teleport; no lerp exists).
- Near plane is Bevy's default 0.1 and `min_distance` is 0.5
  (`camera.rs:75,214`); close orbit on a compact GLB crosses the near plane.
  `apply_camera_scale` adjusts `far`/`max_distance` but never `near`.

## Functional Requirements

- **FR-1 — NodeCreated carries position.** `DbResult::NodeCreated` echoes the
  create position; the fe-ui tree stores it instead of `[0.0;3]`.
- **FR-2 — Focus resolves the live entity.** `CameraFocusTarget` carries the
  node id; `apply_camera_focus` prefers the spawned entity's live
  `GlobalTransform` translation over the cached tree position.
- **FR-3 — Close-zoom without clipping.** Explicit `near = 0.01` (Bevy
  reverse-Z keeps depth precision) and `min_distance = 0.05`; `apply_camera_scale`
  leaves `near` fixed.
