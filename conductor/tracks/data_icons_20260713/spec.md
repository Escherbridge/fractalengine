---
type: Track Spec
title: Data Icons — panel row glyphs, 3D billboard markers, 3D floating labels
tags: [feature, ui, viewport, icons, gpx, data_icons_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Data Icons

**Track ID:** `data_icons_20260713`
**Crates:** `fe-ui`

## Vision / Why (user, 2026-07-13)

"We should have icons for the data." Path points and single-point-track nodes
currently render as bare unlit spheres with no type legibility, and the data
panels list items as plain text. The user wants icons on THREE surfaces
(confirmed): (A) 3D-viewport markers (billboarded, camera-facing), (B) panel
row glyphs in the Paths tab / node lists, (C) floating type/label overlays
above points/nodes in the 3D world.

## Reality (recon 2026-07-13)

- No billboard/face-camera system exists anywhere in the codebase (grep for
  `billboard|face_camera|look_at|LookAt` = zero runtime hits) — surface (A)
  needs a NEW small per-frame system.
- egui here renders monochrome, recolorable Unicode glyphs (not color emoji).
  Existing precedent: `sidebar.rs:306` uses `◆` (`\u{25C6}`) for asset-backed
  nodes vs `●` (`\u{25CF}`) for asset-less, tinted `theme::TREE_NODE_ICON`.
  `theme.rs` has `ICON_GEAR/ICON_ONLINE/ICON_OFFLINE` + `TREE_NODE_ICON`.
- Type discriminants: `GPX_TYPE_KEY` property values `"track"` / `"waypoint"` /
  `"trackpoint"`; `NodeEntry.has_asset`; `PathPointRow.time_seconds.is_some()`;
  `materialization_kind` (None/Node/Line) for a track's point-count shape.
- `TEXT_VIEWPORT_HINT` (translucent) is the existing precedent for egui text
  drawn over the 3D viewport — the basis for surface (C).

## Functional Requirements

- **FR-1 (B) — Panel row glyphs (lightest, do first).** Add a type glyph to
  each row: (a) Paths-tab track rows (`path_editor_card.rs` `render_track_list`
  ~78-96) — a "track" glyph (e.g. a route/path glyph) before the name; (b)
  point rows (`render_edit_view` ~181) — a point glyph, optionally varying on
  `time_seconds.is_some()`; (c) reuse/extend the `sidebar.rs:306` node
  glyph pattern if touching node lists. Add 2-3 `ICON_*` color constants to
  `theme.rs` matching the existing style; use plain geometric Unicode
  codepoints (`\u{25xx}`/`\u{27xx}`) that recolor reliably.

- **FR-2 (A) — 3D billboard markers.** Introduce a `Billboard` marker component
  + a new ~15-line per-frame system (in `fe-ui/src/node_manager/` or a small
  new module) that sets each billboard entity's `Transform.rotation` to face
  the `OrbitCameraController` camera (query the camera `GlobalTransform`, set
  rotation to the camera's rotation, or a `look_at`-style compute). Apply it to
  path-point markers (`sync_path_point_markers`) and single-point-track nodes
  (`spawn_single_point_node`) — replace or augment the bare `Sphere` with a
  flat, camera-facing icon quad (unlit `Rectangle` mesh, depth-biased to render
  on top like the gimbal gizmo, vertex-colored or flat-colored per type). CRITICAL:
  the single-point node carries `SpawnedNodeMarker` and must stay pickable by
  the AABB mesh-pick — keep a `Mesh3d` (the quad) so it still yields an `Aabb`.
  If billboarding proves fiddly, the fallback is a distinct unlit
  icon-shaped mesh (flattened cylinder "coin" / cone "pin") tinted per type,
  no camera-facing math — but the user asked for camera-facing, so attempt the
  billboard system first.

- **FR-3 (C) — 3D floating labels.** Draw an egui screen-space overlay each
  frame: for each path point / single-point node (and optionally each track's
  first point), project its world position to screen via
  `Camera::world_to_viewport`, and paint a small translucent label (type glyph
  + name/index) at that screen position using `theme::TEXT_VIEWPORT_HINT`. Do
  NOT spawn 3D text meshes (`bevy_text`/`Text2d` not confirmed enabled) — the
  egui-overlay route avoids in-world text entirely and matches the existing
  viewport-hint precedent. Gate it so it doesn't clutter (e.g. only while a
  track is being edited, or a toggle).

## Relevant Files

- `fe-ui/src/panels/path_editor_card.rs` — `render_track_list` (~31-107),
  `render_edit_view` point loop (~180+).
- `fe-ui/src/panels/sidebar.rs` — existing `◆`/`●` node glyph (~306) to mirror.
- `fe-ui/src/theme.rs` — `ICON_*`/`TREE_NODE_ICON`/`TEXT_VIEWPORT_HINT`; add new
  `ICON_TRACK`/`ICON_WAYPOINT`/`ICON_POINT` etc.
- `fe-ui/src/node_manager/path_point_interaction.rs` — `sync_path_point_markers`
  (~58-107) marker spawn (billboard target).
- `fractalengine/src/gpx_bridge.rs` — `spawn_single_point_node` (~336) marker
  spawn (billboard target); but a Billboard SYSTEM lives in fe-ui — if the
  single-point node needs billboarding, tag it with a fe-ui-visible `Billboard`
  component (mind crate boundaries: the component must be constructable from
  `gpx_bridge.rs`, so it lives in a fe-ui module `gpx_bridge` already imports,
  or is re-exported like `SpawnedNodeMarker`).
- Root `Cargo.toml` — bevy features (`3d_bevy_render`, `sprite_picking`,
  `default_font`); no new dep needed for the billboard math.

## Constraints

- Bevy 0.18, `default-features = false`. Never `rustfmt`. No concurrent cargo.
  fe-ui `-j1`. No quarantine contact.
- Billboard system is per-frame — keep it cheap (only billboard-tagged
  entities, single camera lookup).
- Prefer egui-overlay for (C); do not add `bevy_text`/`Text2d`.
