---
type: Track Spec
title: GPX Path Editor — Author, Annotate, Export Paths on Terrain
tags: [feature, gpx, ui, terrain, gpx_path_editor_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: GPX Path Editor

**Track ID:** `gpx_path_editor_20260711`
**Crates:** `fe-ui` (editor surface), `fractalengine` (path bridge),
`fe-terrain` (gpx writer), `fe-database` (tests only)

## Vision (user, 2026-07-11)

"You can create gpx with a path editor inside of your nodes. Or a series of
gpx with paths — basically we need a gpx editor as a tool for planning,
defining and annotating paths and processes in the domains." Paths are
first-class planning artifacts: authored on the terrain, annotated,
persisted with the petal, exportable as standard .gpx.

## Functional Requirements

- **FR-1 Paths tab (fe-ui):** GIS panel gains a Paths tab listing the
  petal's track nodes; create-new (named), select-to-edit, delete. Editing
  shows an ordered point list (index, position, per-point remove), append
  points from the 3D cursor position, and an Annotate action per point
  (creates a waypoint node at that position via the existing annotation
  property contract).
- **FR-2 Ops contract:** `PathOp` queue + `PathEditStatus` result resource
  mirroring the gpx_ops/asset_ops pattern; fe-ui does no persistence I/O.
- **FR-3 Persistence (bridge):** trackpoints persist as a `gpx_points` flat
  node property (JSON array of `[x, y, z, time_seconds]`, petal-local
  meters); live edits update `TrackRouteMap` + the `GpxTrackLine` entity
  immediately.
- **FR-4 Materialization:** on petal load/switch, track nodes with
  `gpx_points` repopulate `TrackRouteMap` and respawn `GpxTrackLine` —
  imported AND authored paths render across sessions (closes the
  gpx_pipeline session-scoped-render residual). Import path also persists
  `gpx_points` going forward.
- **FR-5 Export:** `ExportGpx` writes a standard GPX 1.1 file (rfd save
  dialog) — local coords inverse-projected to WGS84 via the petal terrain
  origin; pure writer in fe-terrain's gpx module, unit-tested for
  parse-roundtrip with the existing parser.

## Out of scope (v1)

3D drag-gizmo point manipulation (list-based move/remove suffices);
multi-segment tracks; process semantics beyond paths + annotations.

## Phase 2 (user, 2026-07-12) — interactive 3D editing + line styling

v1 shipped a list-panel editor (create/select/delete tracks, index/remove
point rows, "append from cursor" button). User feedback after trying it:
"my expectation is to be able to set nodes shift them and connect them as
well as annotate those nodes then its a line that stretches through with
annotations on specific paths... a gpx path drawing tool." The list panel
is not that — this phase adds real viewport interaction on top of the
existing v1 persistence/contract layer (FR-2..FR-5 unchanged).

**FR-6 Click-to-place:** clicking the terrain while a track is in edit mode
(`PathEditorState.editing_track_id.is_some()`) appends a point at the
click's world position, instead of requiring the "Append from cursor"
button. Reuses `ViewportCursorWorld` (`fe-ui/src/plugin.rs:195`, already
computed every frame via ray/Y=0-plane intersection) — the resource exists
and is already correctly gated off when egui owns the pointer; this FR is
about *triggering* `PathOp::AppendPoint` on click instead of on button
press, guarded by "not currently dragging a point" (FR-7) and "not
mid gizmo-drag on an unrelated node" (existing `manager.is_dragging()`
guard in `viewport_pick.rs:21-23` — path edit mode should set an
equivalent guard so it doesn't fight node selection).

**FR-7 Drag-to-move:** each point in the currently-edited track renders as
a small pickable sphere in the viewport (reuse the `WaypointMarker`
spawn/render pattern from `fe-terrain/src/terrain_plugin.rs:563-590` —
shared mesh/material, `Pickable::default()` component, one marker entity
per point index). Dragging a point marker updates its world position live
(mirror the gimbal drag lifecycle in
`fe-ui/src/node_manager/gimbal_interaction.rs`: pick-on-press via
ray/screen-space hit test, update-on-hold by projecting cursor delta,
commit-on-release) and on release fires `PathOp::AppendPoint`-equivalent
mutation (or a new `PathOp::MovePoint { track_node_id, index, position }`
variant — cleaner than remove+append, avoids index churn) to persist the
new position. Note: existing node dragging uses manual ray-vs-point
picking (`fe-ui/src/node_manager/viewport_pick.rs`, not Bevy's picking
backend) even though `Pickable` is attached to some entities elsewhere
(waypoint markers, GeoJSON overlays) without their click events being
consumed — pick a consistent approach for path-point markers (likely
mirroring `viewport_pick.rs`'s manual-ray style, for consistency with how
gimbal/node drag already work) rather than introducing a second live
picking mechanism.

**FR-8 Connecting line, live-updated:** the existing `render_gpx_tracks`
system (`fe-terrain/src/terrain_plugin.rs:508-560`) already draws a
`LineStrip` through `TrackRouteMap`'s points and respawns on route change
— FR-7's drag commits should flow through the same `TrackRouteMap` update
+ despawn/respawn `GpxTrackLine` path already built in W-F's round-3 work
(`spawn_track_route` helper in `gpx_bridge.rs`), so the line visibly
follows point drags without new plumbing. This FR is mainly about not
regressing that live-update path when FR-7 lands.

**FR-9 Click-to-annotate:** clicking a point marker while holding a
modifier (or via a small radial/context menu on right-click — match
whatever the existing context-menu pattern uses, `viewport.rs:148-157`)
opens the annotation input for that point, replacing the current "Annotate"
button-per-row in the list panel. Placeholder title
(`"Waypoint {index}"`) from v1 becomes a real inline title/body/color
form, reusing `gis.annotation.*` contract fields.

**FR-10 Line style, color, and per-track visibility:** the Layers tab (or
a new per-track row in the Paths tab) gains: a color picker per track
(reuse the annotation card's hex color picker component,
`fe-ui/src/panels/annotation_card.rs`), a line style selector (solid /
dashed — dashed requires a new line-rendering approach, see below), and a
visibility toggle per track (mirror the existing per-layer
visible/opacity controls in `layer_manager_card.rs`). Persist style/color/
visibility as additional flat properties on the track node (e.g.
`gis.track.color`, `gis.track.line_style`, `gis.track.visible`) — same
flat-key convention as `gis.track.name`, `gis.annotation.*`.

**FR-11 Visible line rendering:** the current line is a 1px `LineStrip`
with flat unlit cyan `StandardMaterial` (`terrain_plugin.rs:538-548`) —
this is the "no texture" look reported. Replace with a proper ribbon/tube
mesh: build a quad-strip (width in world units, e.g. 0.3–0.5m) oriented
along each segment's tangent with a slight up-offset to avoid z-fighting
with terrain, using the per-track `gis.track.color` from FR-10 as the
material's base/emissive color. This is a from-scratch mesh-generation
function (no existing ribbon-builder in the codebase to reuse), unit-
testable the same way `mesh::skirt::build_skirt` is (pure function taking
points + width, returning vertices/indices).

## Out of scope (phase 2)

Multi-segment/branching tracks; dashed-line rendering (FR-10 exposes the
selector but only "solid" needs to actually render differently in phase
2 — dashed can no-op to solid with a TODO); collaborative/multi-user
concurrent path editing; undo/redo for point drags.
