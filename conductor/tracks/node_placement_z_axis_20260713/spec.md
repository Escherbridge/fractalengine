---
type: Track Spec
title: Vertical Node/Point Placement + Single-Point Track Defaults to Node
tags: [feature, ui, gis, placement, node_placement_z_axis_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Node Placement — Vertical (Height) Axis + Single-Point Track Defaults

**Track ID:** `node_placement_z_axis_20260713`
**Crates:** `fe-ui`, `fractalengine`

## Vision / Why (user, 2026-07-13)

The user reports two placement-semantics gaps:

1. Placement is locked to the ground plane (`y = 0`) everywhere. Every
   viewport raycast used for placement/drag intersects the horizontal plane
   at a fixed height: `ray_plane_y(&ray, 0.0)` in the pen-tool append path
   and `update_viewport_cursor_world`'s ground-plane projection. Nodes and
   path points cannot travel along the vertical/height axis at placement or
   drag time.

   **Terminology note:** the user calls this the "z-axis." In this codebase,
   Bevy uses a Y-up convention, so the user's "z" (vertical/height) maps to
   Bevy's **Y** axis, not Bevy's Z (which is a horizontal/depth axis in this
   engine's ground-plane convention). This spec uses "vertical/height axis"
   throughout and calls out "Bevy Y" explicitly to avoid ambiguity with
   Bevy's own Z axis.

2. A GPX "track" node whose `gpx_points` contains exactly **one** point
   should default to behaving like a plain node — not a path. Today, any
   track node carrying `gpx_points` is treated uniformly as a path/track
   through the same code paths, and the "what does a 1-point track do"
   behavior is inconsistent between rendering and interaction (see FR-3
   investigation below).

## Functional Requirements

### FR-1: Allow vertical (height) placement/drag, not just the `y=0` plane

Today's fixed-plane raycasts:
- `fe-ui/src/node_manager/path_point_interaction.rs::ray_plane_y` — helper
  that intersects a ray with the horizontal plane `y = plane_y`.
  - Pen-tool **append** (new point on empty click): calls
    `ray_plane_y(&ray, 0.0)` — hardcoded to height 0 (~line 227).
  - Point **drag**: calls `ray_plane_y(&ray, plane_y)` where `plane_y` is
    captured from the picked marker's *current* height at drag-start
    (~line 180, `plane_y` sourced at ~line 211-215) — this already
    preserves an existing point's height across a horizontal drag (see
    FR-2; it does not yet let the user *change* height during/via drag).
- `fe-ui/src/plugin.rs::update_viewport_cursor_world` — projects the cursor
  ray onto the infinite `y = 0` plane every frame for `ViewportCursorWorld`
  (used by node/asset placement elsewhere), hardcoding `ground_origin =
  Vec3::ZERO` / `Dir3::Y` and writing back `[point.x, 0.0, point.z]`
  (~lines 456-502).

No terrain height-sampling exists anywhere in the codebase today — every
placement raycast that hits "the ground" hits the literal `y = 0` plane,
not a sampled terrain surface. Any "snap to terrain" option below is
therefore a larger, currently-unimplemented undertaking, not a small addon.

**Options to weigh (this spec does not decide — implementor/architect
should pick based on effort vs. UX in the actual work unit):**

- **(a) Height-modifier drag.** Hold a modifier key (e.g. Shift/Ctrl —
  note Shift/Alt are already used for annotate-click in
  `path_point_interaction.rs`, so pick an unused modifier) + vertical mouse
  movement while dragging a point/node to raise/lower it, decoupled from
  the horizontal ray-plane hit. Lightest to implement: no new UI surface,
  reuses the existing drag state machine, just needs a second drag mode
  that adjusts `plane_y` (or an accumulated height offset) instead of
  reprojecting through `ray_plane_y`.
- **(b) Numeric height field.** Add a height (Y) number field to the
  inspector/tool panel for the selected node/point, edited directly. Needs
  a UI panel touchpoint (likely `tool_panel.rs` per `gis_tool_panel_20260713`
  or the existing inspector) plus a new `UiAction` to push the height edit
  through the same DB-write path as `PathMovePoint`/node property updates.
  More discoverable/precise than a drag gesture, but more surface area.
  Instead of a full snapping system.
- **(c) Terrain-height snapping.** Sample terrain elevation at the XZ
  cursor position and place at that height automatically. **Not
  recommended for this track** — no terrain height-sampling API exists
  yet (`fe-terrain` has GPX ingestion/tiles/mesh but nothing exposed here
  as a per-point height query at cursor time); this would require new
  terrain-query plumbing and is a substantially larger effort than (a)/(b).

**Recommendation:** (a), the height-modifier drag, is the lightest option
that unblocks vertical placement without new UI or new `UiAction` variants
(it composes with the drag machinery in FR-2's existing plane-preserving
code). (b) is a reasonable follow-up for precise numeric entry. (c) is
out of scope until terrain height-sampling exists as its own capability.

### FR-2: Confirm existing drag height-preservation + persistence needs no format change

The drag branch in `handle_path_point_interaction` (~lines 176-189) already
captures the picked marker's height at drag-start
(`plane_y = marker_pick...map(|(g, _)| g.translation().y)`, ~211-215) and
re-intersects at that same height for the duration of the drag — so a
point raised above `y = 0` (by whatever future mechanism from FR-1) keeps
its height across a horizontal-only drag today. This FR is a build-on-top
point for FR-1(a): the height-modifier drag should adjust this captured
`plane_y` value in place rather than replacing the mechanism.

Persistence: `gpx_points` already encodes each point as `[x, y, z,
time_seconds]` (`fractalengine/src/gpx_bridge.rs::route_points_to_json`,
~line 177-184, decoded by `json_to_route_points` ~186-205) and
`PathPointRow.position: [f32; 3]` (`fe-ui/src/gis/mod.rs` ~line 115-116)
already stores full 3D position. **Confirmed: no format change needed** —
the y-component is already read/written faithfully end-to-end; only the
placement/drag *input* is constrained to `y = 0`, not the storage.

### FR-3: Single-point track defaults to a plain node

Define the threshold:
- `gpx_points.len() >= 2` → path (current behavior: renders a polyline via
  `spawn_track_route`).
- `gpx_points.len() == 1` → **node** (this FR's target behavior).
- `gpx_points.len() == 0` → empty track (no line, no node — current
  behavior already handles this as a no-op).

**What currently happens today (verified):**
`fractalengine/src/gpx_bridge.rs::advance_path_materialization` (~line
1178-1218) already gates line-spawning on `points.len() < 2` — i.e. a
1-point track **already does not spawn a polyline** (~line 1193-1195,
`if points.len() < 2 { continue; }`). However, nothing is spawned in its
place: a 1-point track is silently invisible/unselectable today — it is
not "defaulting to a node," it is dropping out of rendering entirely.

**What "default to a node" should add:** when a track's `gpx_points` has
exactly one point, spawn/treat it as a normal, selectable node entity at
that point's position (using the existing node-entity spawn path, not the
`GpxTrackLine`/`spawn_track_route` path) instead of doing nothing. This
likely means branching `advance_path_materialization`'s `points.len() < 2`
arm: `== 1` spawns a plain node at that position; `== 0` remains a no-op.
The exact node-spawn call site/component set should be confirmed against
whatever the standard node-entity spawn helper is in this crate/fe-ui at
implementation time (not enumerated here — this is a spec, not an
implementation).

## Relevant Files

- `fe-ui/src/node_manager/path_point_interaction.rs` — `ray_plane_y` (~106),
  drag branch on captured `plane_y` (~176-189, capture at ~211-215), pen
  append hardcoded to `ray_plane_y(&ray, 0.0)` (~227).
- `fe-ui/src/plugin.rs` — `update_viewport_cursor_world`, Y=0 ground-plane
  projection for `ViewportCursorWorld` (~456-502).
- `fractalengine/src/gpx_bridge.rs` — `route_points_to_json` /
  `json_to_route_points` `[x, y, z, time_seconds]` encode/decode (~175-205);
  `advance_path_materialization`'s `>= 2`-point gate for line spawning
  (~1178-1218, gate at ~1193-1195).
- `fe-ui/src/gis/mod.rs` — `PathPointRow.position: [f32; 3]` (~115-116).

## Constraints

- Bevy is Y-up in this engine; the user's "z-axis" (vertical/height) is
  Bevy's **Y** axis, not Bevy's Z. Any implementation and its own docs
  should say "vertical/height (Bevy Y)" rather than "z-axis" to avoid
  confusion with Bevy's actual Z axis.
- **Never** run `rustfmt` on this repo.
- **Do not** touch quarantine files: `fe-api/*`, `fe-database/src/lib.rs`,
  `.conductor_session_log`, `.codex/`.
- No concurrent `cargo` invocations (workspace-wide build lock convention).

## Dependencies

Soft/loose dependency on `input_router_20260713`: FR-1's height-modifier
drag (option (a)) is a new interaction that may want to route through
whatever input-routing abstraction that track establishes, but this track
does not require it to land first — the interaction can be implemented
directly against the existing `handle_path_point_interaction` system if
`input_router_20260713` is not yet available.
