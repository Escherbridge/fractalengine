---
type: Worker Report
title: BIM Primitives on Paths — P3 (FR-5 path→wall, FR-6 building composition)
tags: [bim_primitives_on_paths_20260712, worker-report, p3]
timestamp: 2026-07-13T00:00:00Z
resource: ./spec.md
---

# P3 Report — FR-5 (path → wall extrusion) + FR-6 (building composition)

Scope: **P3 only**. Edit-only — no cargo/build/test/check/clippy run
(coordinator owns the serialized sweep). Track 1
(`path_node_binding_hardening`) has landed and is test-green, so P3 was
unblocked. Tests written but NOT run.

## Files changed (only the fenced set)

- `fe-sdk/src/primitive.rs` — added `PrimitiveKind::Wall`; added
  `source_path: Option<String>` to `PrimitiveDescriptor` (serde default +
  skip-if-none). Updated the 4 existing round-trip cases with `source_path:
  None` and added 4 new tests (wall round-trip, source_path-defaults-none,
  source_path-omitted-when-none, wall parse).
- `fe-sdk/src/AGENTS.md` — new §primitive-wall (the WHY: why object-key
  `source_path` and not a `dims` slot; forward-compat via serde default).
- `fe-ui/src/verse_manager/spawn.rs` — `build_wall_mesh(polyline, height)`
  (raw quad-strip Mesh), `WallNode` marker component, `spawn_wall_entity`.
  New test module: 4 tests (segment vertex/index counts, top-row height,
  degenerate-polyline empty mesh, bad-height clamp).
- `fe-ui/src/verse_manager/primitive_reconcile.rs` — `promote_selected_wall`
  (first-materialization), `wall_reconcile` (path-driven re-projection off
  Track-1 events), `decode_gpx_points` helper. Excluded `Wall` from the
  shape-primitive path in `reconcile_selected_primitive`. Fixed the existing
  test's descriptor literal; added 3 tests (source_path change detection,
  decode maps xyz/skips short, decode empty for non-array).
- `fe-ui/src/verse_manager/mod.rs` — registered `promote_selected_wall` +
  `wall_reconcile` systems; re-exported `build_wall_mesh` + `WallNode`.
- `fe-ui/src/verse_manager/AGENTS.md` — new §wall + §building-composition;
  updated `spawn.rs`/`primitive_reconcile.rs` bullets.

**Not touched**: `fractalengine/src/gpx_bridge.rs` — intentionally avoided
(see "Track-1 events" below). No quarantined file touched.

## Wall descriptor shape (the resolved design)

A wall is a `PrimitiveKind::Wall` node reusing the **existing**
`PrimitiveDescriptor` (no second descriptor type — the workspace has exactly
one, per fe-sdk's own AGENTS.md):

```json
{
  "kind": "wall",
  "dims": [height],                 // single extrusion height, world units
  "source_path": "track-node-id",   // gpx_points-carrying track that drives shape
  "texture_ref": "blob-id" | null   // FR-3 material, same as other primitives
}
```

- `dims = [height]` — the vertical extrusion height. All *horizontal* geometry
  comes from `source_path`, never from `dims`.
- `source_path: Option<String>` (new field) — the track node_id whose
  `gpx_points` polyline is the wall's shape (C3, the GPX merge). It is
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, so (a) pre-Wall
  descriptors still parse and (b) non-wall descriptors serialize
  byte-identically (no spurious `source_path: null`).
- It rides the property bag as `PropertyValue::Json` (C5) — an **object**
  shape, which round-trips losslessly through the untagged `fe-runtime`
  `PropertyValue` (objects are safe; only scalar/array `Json` had the
  variant-order caveat T4 documented). This is exactly why `source_path` is an
  object key and not an overloaded `dims` slot.

## build_wall_mesh — signature + extrusion

```rust
// fe-ui/src/verse_manager/spawn.rs
pub fn build_wall_mesh(polyline: &[[f32; 3]], height: f32) -> Mesh
```

Extrusion: each consecutive segment `polyline[i] → polyline[i+1]` becomes one
**vertical quad** = 2 triangles = 4 verts + 6 indices. Bottom edge = the two
base points (their own `y` is the base elevation); top edge = the same XZ at
`base_y + height`. The outward normal is the horizontal perpendicular of the
segment (`(dz, 0, -dx)` normalized; degenerate segments fall back to +Z). UVs
are per-quad `[0..1]²`.

It is a **hand-built raw `Mesh`** (positions/normals/uvs/indices via
`Mesh::new(PrimitiveTopology::TriangleList, …)` + `insert_attribute` +
`insert_indices`) — the same idiom as `fe-terrain/src/splat/render.rs:352
bake_splat_mesh`, but assembled **locally in fe-ui** with **no fe-terrain
dependency** (imports are `bevy::asset::RenderAssetUsages` +
`bevy::mesh::{Indices, PrimitiveTopology}`, matching the proven splat imports
for Bevy 0.18). Guards: `< 2` points → empty mesh (no panic); non-finite/≤0
height → clamped to `2.0` (mirrors `dim_or`). No `unwrap`/`expect`.

The wall entity's `Transform` is **identity** — polyline vertices are already
petal-local world coordinates (the same `gpx_points` the path line renders
from), so the geometry is self-positioning (unlike a cube whose centred mesh is
placed via `Transform::from_xyz`).

## Exactly which Track-1 event(s) drive wall re-projection (and how I subscribed WITHOUT editing gpx_bridge)

I consumed Track 1's **existing** `DbResult` lifecycle events. `DbResult` is a
Bevy `Message`; each `MessageReader<DbResult>` has its **own independent
cursor**, so a new reader system never starves the existing ones
(`advance_path_materialization`, `advance_path_edits`, `apply_db_results`). No
new notification path, no `gpx_bridge` edit.

`wall_reconcile` (a `MessageReader<DbResult>` system) reacts to two events:

1. **`DbResult::NodePropertiesLoaded { node_id, properties }`** — this is the
   single signal that fires both on **petal load/switch** (Track 1's
   `request_petal_gpx_materialization` issues `GetNodeProperties` for every
   node in the active petal) **and on every live path edit** (Track 1's
   `persist_and_render_points` writes `gpx_points`, and the DB thread re-emits
   the properties). For any spawned `WallNode` whose `source_path == node_id`,
   the wall reads that node's `gpx_points`, decodes it via `decode_gpx_points`,
   and **rebuilds its mesh in place** (`mesh3d.0 = meshes.add(build_wall_mesh(…))`).
   → a wall re-extrudes whenever its source path changes (append / move /
   remove point, import, etc.).

2. **`DbResult::NodeDeleted { node_id, .. }`** — for any `WallNode` whose
   `source_path == node_id`, the wall entity is **despawned** (its shape driver
   is gone). `PathOp::DeleteTrack` already cascades to a `NodeDeleted` (Track
   1's FR-2 delete work), so deleting the source track tears down its walls.

First materialization is separate: `promote_selected_wall` reads the selected
node's `primitive` descriptor from `InspectorFormState.node_properties` (the
one already-wired per-node property source — same selected-node scope as the
shape primitives, §primitives). When it sees a not-yet-spawned `Wall`, it
despawns any placeholder `FallbackSign`, spawns an **empty-geometry** wall
(carrying `source_path` + `height`), and issues one `GetNodeProperties` for the
`source_path` so the very next frame's `NodePropertiesLoaded` fills the
geometry via `wall_reconcile` — the wall appears immediately rather than only
on the next petal-load batch.

**Point source (C3 GPX-merge):** walls read the **same DB-backed
`gpx_points`** the path renders from (key `"gpx_points"`, `[[x,y,z,t],...]`
JSON written by `gpx_bridge.rs`). `decode_gpx_points` drops `t` into `[x,y,z]`.
I deliberately did **not** read the editor's local buffer (`PathEditorState`) —
the DB row is the single source of truth, so a wall always matches the
persisted path.

## Building = N walls (FR-6, the compose pattern)

No new abstraction, per spec. A "building" is simply **N `Wall` primitive nodes
(+ optional GLTF model nodes) grouped under one petal** via the existing
verse/fractal/petal/node hierarchy. Each wall independently binds to its own
`source_path` and re-projects on that path's changes; they render together
because they share a petal and each spawns its own `WallNode` entity through the
systems above. A remodel = layering a GLTF model node next to/over the extruded
walls in the same petal. Documented in
`fe-ui/src/verse_manager/AGENTS.md` §building-composition. No grouping
primitive/type was built.

## Tests written (NOT run — coordinator's sweep)

- `fe-sdk/src/primitive.rs`: wall round-trip (added to the all-kinds case) +
  wall parse + source_path-defaults-none + source_path-omitted-when-none.
- `fe-ui/src/verse_manager/spawn.rs`: `build_wall_mesh` segment→vertex/index
  counts (4 pts → 3 segs → 12 verts / 18 indices), top-row = base+height,
  degenerate (<2 pts) → empty, bad-height clamp to positive.
- `fe-ui/src/verse_manager/primitive_reconcile.rs`: source_path change is a
  detectable descriptor diff, `decode_gpx_points` maps xyz & skips short
  entries, decode empty for non-array.

Needs `-p fe-sdk -p fe-ui` in the coordinator's serialized sweep (fe-ui at
`-j 1`). No new crate deps introduced — `fe-ui` already depends on
`fe-runtime`, `fe-sdk`, `fe-hexon`.

## Design choices I was unsure about (flag for review)

1. **Empty-then-fill wall spawn.** `promote_selected_wall` spawns an
   empty-geometry wall and relies on the follow-up `GetNodeProperties` →
   `NodePropertiesLoaded` to fill it (one-frame-later geometry). Alternative
   would be to block spawn until points are in hand, but that needs a pending
   correlation map like `gpx_bridge`'s — heavier, and the empty-mesh entity is
   harmless (renders nothing) for the one frame. Chose the lighter path.
2. **Selected-node scope for promotion.** Same limitation the P1/P2
   `reconcile_selected_primitive` carries: an *unselected* wall node spawns as a
   `FallbackSign` on petal load and only promotes to a real wall once selected,
   because `NodeEntry` carries no `properties` field (adding one requires
   `db_results.rs` edits, still fenced). In-app verification (select a wall
   node, set its descriptor, watch it extrude) is unaffected. Petal-wide
   auto-spawn is the same fast-follow §primitives already flagged.
3. **Wall transform is identity.** I put the polyline in world coordinates and
   left the entity Transform at identity, rather than centring the mesh and
   translating. This matches how `gpx_bridge`'s `GpxTrackLine` renders the path
   (points are already petal-local world coords), so a wall sits exactly on its
   path. If a future gizmo/transform surface (P4/FR-7) wants to translate a wall
   as a rigid body, that will need the mesh recentred + a real Transform —
   noted but out of P3 scope (C1: no interactive gizmo in v1).
4. **Height = `dims[0]`.** Stored redundantly on `WallNode.height` (copied from
   the descriptor at spawn) so `wall_reconcile` can rebuild geometry without
   re-reading the property bag each time the *source path* changes. A change to
   the wall's own `height` descriptor is not yet a live-reconcile trigger (only
   source-path changes are) — editing wall height currently needs a
   reselect/respawn. Flagged as a possible FR-2-style follow-up if live height
   edits are wanted.

P3_COMPLETE
