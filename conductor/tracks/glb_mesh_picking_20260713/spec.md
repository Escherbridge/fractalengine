---
type: Track Spec
title: GLB Mesh Picking — Precise Raycast Selection for glTF Model Nodes
tags: [feature, ui, picking, raycast, glb_mesh_picking_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: GLB Mesh Picking

**Track ID:** `glb_mesh_picking_20260713`
**Crates:** `fe-ui`

## Vision / Why (user, 2026-07-13)

Node selection in the viewport currently uses a **distance-to-origin sphere
test**: `handle_viewport_click` (`fe-ui/src/node_manager/viewport_pick.rs`)
iterates `SpawnedNodeMarker` entities and picks whichever marker's world
translation is within `PICK_RADIUS` of the closest point on the click ray —
it never tests against the model's actual geometry. For a glTF model this
means clicking empty space near the node origin can select it, while
clicking on a visually large part of the mesh far from that origin misses.

The user's "object detection" ask is really: **click on the glb model's
actual rendered surface (or at minimum its bounding volume) and have that
resolve to the correct node.** glTF models spawn as a `SceneRoot` whose
pickable geometry (mesh + `Aabb`) lives on **child entities** instantiated
by Bevy's glTF scene loader, not on the root entity carrying
`SpawnedNodeMarker`. `fe-ui/src/gimbal.rs`'s `gimbal_center` already solves
an adjacent problem (finding the AABB center for gizmo placement) by
scanning the root entity first, then its immediate children, for a
`bevy::camera::primitives::Aabb` component — this track should mirror that
walk for picking instead of centering.

## Functional Requirements

- **FR-1:** Replace the proximity (distance-to-ray) test in
  `handle_viewport_click` with a real raycast against glb geometry.
  - Minimum bar: AABB raycast using `bevy::camera::primitives::Aabb`
    (already imported via `gimbal.rs`) — ray/AABB slab intersection in
    world space, replacing the sphere-radius check.
  - Stretch/ideal: mesh-triangle-accurate picking via Bevy 0.18's
    `MeshRayCast` system param, if available and ergonomic under this
    workspace's enabled feature set. **Verify availability before
    depending on it** — the root `Cargo.toml` (`bevy = { default-features
    = false, features = [...] }`) already enables `"bevy_picking",
    "mesh_picking", "sprite_picking", "ui_picking"` alongside
    `"3d_bevy_render"`, so `bevy::picking::mesh_picking::MeshRayCast` (or
    the 0.18-equivalent import path) is plausibly in scope — confirm the
    exact type path and required `SystemParam` plumbing compile before
    committing to it as the primary path. AABB is the required fallback
    if mesh-triangle picking proves unavailable or too costly to wire
    within this track's scope.
- **FR-2:** Handle glTF scene hierarchy correctly: the pickable
  geometry (mesh / `Aabb`) is attached to **child entities** of the
  `SceneRoot` (confirmed in `fe-ui/src/verse_manager/spawn.rs`, which
  spawns `SceneRoot(handle)` + `SpawnedNodeMarker` together on one root
  entity, with Bevy's glTF loader instantiating the actual mesh subtree
  underneath at scene-load time). The picker must walk children the same
  way `gimbal_center` does (`fe-ui/src/gimbal.rs:56-77`: try the entity
  itself, then scan `Children` for an `Aabb`) — but resolve the hit back
  to the **root** entity/`SpawnedNodeMarker` for selection purposes
  (mirrors how `gimbal_center` computes gizmo position for the root
  while reading geometry from the child).
- **FR-3:** Register as a `NodePick`-priority consumer in the
  `input_router`. **This FR depends on `input_router_20260713`**, which
  does not yet exist in `conductor/tracks/` as of this writing. Until
  that router lands, this track's raycast logic can be prototyped
  in-place inside the existing `handle_viewport_click` system in
  `viewport_pick.rs`; the final form should extract the raycast into a
  router-callable consumer rather than a bespoke per-system click
  handler. Do not block FR-1/FR-2 implementation on the router's
  existence — land the precise-pick logic first, then re-wire the call
  site once the router exists.
- **FR-4:** Path-point marker picking (`pick_marker` in
  `fe-ui/src/node_manager/path_point_interaction.rs`, ~line 85) uses the
  same along-ray + `PICK_RADIUS` sphere test pattern (radius test against
  a `PathPointMarker`'s `GlobalTransform`, no Bevy picking) — this is
  explicitly **out of scope for precision upgrades** (markers are small
  fixed-size gizmo spheres; a proximity test is the correct model for
  them) but must **not regress** when FR-1/FR-2 change
  `viewport_pick.rs`. If any shared helper is extracted (e.g. a common
  "closest point on ray" utility), confirm `pick_marker`'s behavior is
  unchanged by diffing marker-click behavior before/after. The
  deliverable is: model picking becomes precise (AABB/mesh), marker
  picking stays exactly as precise/imprecise as it already is.

## Relevant Files

- `fe-ui/src/node_manager/viewport_pick.rs` — `handle_viewport_click`
  (lines ~10-80): current proximity pick over `node_query: Query<(Entity,
  &GlobalTransform, &SpawnedNodeMarker)>`; `PICK_RADIUS = 1.5`;
  ray built via `camera.viewport_to_world`. This is the primary edit site
  for FR-1/FR-2.
- `fe-ui/src/node_manager/path_point_interaction.rs` — `pick_marker`
  (~line 85-102): along-ray + `PICK_RADIUS` sphere test against
  `PathPointMarker`; keep working per FR-4.
- `fe-ui/src/gimbal.rs` — `gimbal_center` (lines 56-77): the child-`Aabb`
  scan pattern to mirror for FR-2 (entity-then-children lookup against
  `bevy::camera::primitives::Aabb`, using `GlobalTransform` +
  `Children` queries).
- `fe-ui/src/verse_manager/spawn.rs` — `spawn_node_entity` /
  `spawn_stamped_entity` (glb-spawning nodes ~line 24 and ~line 70): both
  spawn `SceneRoot(handle)` + `SpawnedNodeMarker { node_id, petal_id, .. }`
  on the same root entity; confirms geometry arrives as children after
  scene instantiation, not synchronously at spawn time.
- `fe-ui/src/plugin.rs` — `SpawnedNodeMarker` definition (~line 25-31):
  `{ node_id: String, petal_id: String }`, the component identifying a
  selectable root entity.
- Root `Cargo.toml` (workspace, lines 29-46) — bevy feature list:
  `"3d_bevy_render"`, `"scene"`, `"bevy_picking"`, `"mesh_picking"`,
  `"sprite_picking"`, `"ui_picking"` are enabled; check these are
  sufficient for `MeshRayCast` before relying on it (FR-1 stretch path).

## Constraints

- Bevy 0.18, workspace built with `default-features = false` — only the
  features listed in the root `Cargo.toml` are available; do not add new
  bevy features without checking workspace-wide compile cost first.
- `bevy::render::mesh::VertexAttributeValues` is **private** in this
  workspace's Bevy version — if mesh-triangle raycasting requires reading
  raw vertex data manually (rather than through `MeshRayCast`'s own
  internals), read mesh geometry via `Mesh::attribute(...).as_float3()`
  (per existing workspace convention), not by matching on
  `VertexAttributeValues` directly.
- **NEVER run rustfmt on this repository.**
- **DO NOT touch quarantine files**: `fe-api/*`, `fe-database/src/lib.rs`,
  `.conductor_session_log`, `.codex/`. These are out of scope for this
  track regardless of any incidental proximity in git status.
- No concurrent `cargo build`/`cargo check`/`cargo test` invocations
  across tracks — coordinate before running builds.
- This is a spec/planning track only: no code changes, no build/test
  runs are part of authoring this document.

## Dependencies

- **Depends on:** `input_router_20260713` for FR-3's final integration
  (router-priority consumer registration). That track does not exist yet
  in `conductor/tracks/` as of 2026-07-13; FR-1/FR-2 can and should
  proceed independently against the existing `viewport_pick.rs` call
  site in the meantime.
