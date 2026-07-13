---
type: Track Spec
title: Per-Track Styling — color, thickness, visibility from the Paths tab
tags: [feature, ui, gpx, terrain, styling, track_styling_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Per-Track Styling

**Track ID:** `track_styling_20260713`
**Crates:** `fe-ui`, `fe-terrain`, `fractalengine`

## Vision / Why (user, 2026-07-13)

"We also need to allow for controlling track color, thickness and visibility."
Today every GPX track renders identically: `render_gpx_tracks`
(`fe-terrain/src/terrain_plugin.rs:508-560`) builds a `LineStrip` mesh with a
**hardcoded** `base_color: Color::srgb(0.0, 0.8, 1.0)` (cyan), 1px GPU line
width, always visible. The user wants **per-track** color + thickness +
visibility, edited in the **Paths tab** and persisted so it survives reload.

The `node_manager/AGENTS.md` already flagged this as deferred "FR-10": per-track
line styling "depends on `fe_terrain::terrain_plugin::GpxTrackStyle`" — this
track builds that. A `ColorMode` enum and a ribbon-mesh `track_mesh(points,
width, color_mode)` already exist in `fe-terrain/src/mesh/track.rs:27` and
`fe-terrain/src/layers/style.rs` but are NOT wired into `render_gpx_tracks`.

## Functional Requirements

- **FR-1 — Persisted per-track style.** Store style as node properties on the
  track node (mirrors how `gis.track.name` / annotations persist): e.g.
  `gis.track.color` (hex string like the existing `GisResultRow.annotation_color`),
  `gis.track.width` (f32), `gis.track.visible` (bool). Editing pushes through
  the existing `SetNodeProperty` path (like `PathMovePoint`/annotations do). No
  new DB command; no quarantine contact (`SetNodeProperty` dispatch is not
  quarantined).

- **FR-2 — Paths-tab controls.** In the Paths tab edit view
  (`fe-ui/src/panels/path_editor_card.rs::render_edit_view`), add for the
  currently-edited track: an egui color picker (`egui::color_picker` /
  `color_edit_button_srgb`), a thickness `DragValue`/`Slider`, and a visibility
  checkbox. On change, emit a UiAction that writes the corresponding node
  property. Reuse the deferred-push idiom already in that function
  (`to_move`/`to_remove`/`to_annotate` collected then dispatched after the
  borrow ends). A new `UiAction::PathSetStyle { track_node_id, color?, width?,
  visible? }` (or three focused actions) → `SetNodeProperty`. Also consider a
  compact style affordance on each track ROW in `render_track_list` (a color
  swatch + a visibility eye toggle) so styling is reachable without entering
  edit mode — optional, edit-view is the required home.

- **FR-3 — Render honors style.** `render_gpx_tracks`
  (`fe-terrain/src/terrain_plugin.rs:508`) must read each track's style and
  apply it:
  - **Color:** replace the hardcoded `srgb(0.0,0.8,1.0)` with the track's
    `gis.track.color`.
  - **Thickness:** the current `LineStrip` topology ignores width (GPU 1px).
    To honor thickness, switch to the ribbon mesh `track_mesh(points, width,
    ColorMode::Solid{color})` (`fe-terrain/src/mesh/track.rs:27`) which already
    builds a width-aware triangle ribbon — OR document that `LineStrip` width
    is not honorable and use the ribbon path. Prefer the ribbon path so
    thickness is real.
  - **Visibility:** when `visible == false`, do not spawn the mesh (or insert
    `Visibility::Hidden`); when toggled back on, respawn/show. This interacts
    with the `Without<Mesh3d>` gate — a hidden track that later becomes visible
    must re-enter the render path (mirror the reconcile-by-marker discipline;
    despawn+respawn or toggle `Visibility`).
  - The style must reach `render_gpx_tracks` — it currently only has
    `TrackRouteMap` (points, no style). Add a per-track style map/resource
    (e.g. `TrackStyleMap: HashMap<track_node_id, TrackStyle>`) populated from
    node properties by the fractalengine gpx bridge
    (`advance_path_materialization` / `reconcile_track_render` already read
    `NodePropertiesLoaded` and have the property bag), analogous to how
    `TrackRouteMap` is populated. Define `TrackStyle { color, width, visible }`
    (a plain struct) in `fe-terrain` next to `TrackRouteMap` so both crates can
    use it.

- **FR-4 — Defaults + back-compat.** A track with no style properties uses the
  current defaults (cyan, current width, visible) so existing tracks are
  unchanged until edited. Missing/invalid property values fall back to the
  default, never panic.

## Relevant Files

- `fe-terrain/src/terrain_plugin.rs` — `render_gpx_tracks` (508-560, the
  hardcoded color + LineStrip); `GpxTrackLine` (59).
- `fe-terrain/src/mesh/track.rs` — `track_mesh(points, width, color_mode)` (27),
  local `ColorMode` (8).
- `fe-terrain/src/layers/style.rs` — richer `ColorMode` enum (Solid/gradients)
  + `TrackPoint`; and `TrackRouteMap` lives in `fe-terrain/src/iot/animation.rs`
  (add `TrackStyle` + `TrackStyleMap` nearby).
- `fractalengine/src/gpx_bridge.rs` — `advance_path_materialization` /
  `reconcile_track_render` read `NodePropertiesLoaded` property bags
  (populate `TrackStyleMap` here from `gis.track.*` keys); `GPX_*` key consts.
- `fe-ui/src/panels/path_editor_card.rs` — `render_edit_view` (109+),
  `render_track_list` (31-107); deferred-push idiom.
- `fe-ui/src/actions/mod.rs` + `actions/path.rs` — add `PathSetStyle` (or
  color/width/visible actions) → `SetNodeProperty`.
- `fe-ui/src/gis/mod.rs` — `GisResultRow` (has `annotation_color` hex precedent
  for a color property); `PathEditorState`.

## Constraints

- Bevy 0.18, `default-features = false`. Never `rustfmt`. No concurrent cargo.
  fe-terrain + fractalengine are heavy — build `-j1`. No quarantine contact
  (`fe-api/*`, `fe-database/src/lib.rs`) — `SetNodeProperty` is the persist
  path and is not quarantined.
- Keep `TrackStyle` a plain data struct in fe-terrain so fe-ui and
  fractalengine can both reference it without a cycle.
- Thickness needs the ribbon `track_mesh`, not `LineStrip` — switching topology
  is the real work; confirm `track_mesh` output renders correctly before
  wiring the slider.
