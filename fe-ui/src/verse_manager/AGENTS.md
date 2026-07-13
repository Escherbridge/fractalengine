# fe-ui/src/verse_manager — hierarchy tree + DB result draining

- `mod.rs` — `VerseEntry`/`FractalEntry`/`PetalEntry`/`NodeEntry` tree types,
  `VerseManager` resource + its query/mutate methods, `VerseManagerPlugin`,
  and the hierarchy unit tests.
- `db_results.rs` — `apply_db_results`, the large `DbResult` match that
  updates the in-memory tree, dialog state, and inspector state in response
  to every DB thread reply. Also owns `tokens_to_entries` /
  `refresh_inspector_tokens` (API token list bookkeeping) and
  `is_for_selected_node` — the `NodePropertiesLoaded`/`NodePropertySet`/
  `NodePropertyDeleted` arms gate on this (dropping stale results for a
  node that's no longer selected) as part of the annotation-save fix; see
  root `AGENTS.md` §gis-query-ui.
- `spawn.rs` — GLTF-backed scene spawning (`spawn_node_entity`), the
  fallback placard sign for asset-less nodes (`spawn_fallback_sign` +
  `FallbackSign` marker component), primitive-mesh spawning
  (`spawn_primitive_entity`, `build_primitive_mesh`, `PrimitiveNode` marker —
  FR-1), and path-driven wall spawning (`spawn_wall_entity`, `build_wall_mesh`,
  `WallNode` marker — FR-5; see §wall). Shared by `db_results.rs`,
  `petal_respawn.rs`, and `primitive_reconcile.rs`.
- `petal_respawn.rs` — despawns/respawns scene entities in-place when
  `NavigationManager::active_petal_id` changes, using the in-memory tree
  directly (no DB round-trip). `NodeEntry` carries no `properties` field
  (adding one would break `db_results.rs`'s explicit-field `NodeEntry`
  construction, which is out of this crate's edit scope — see §primitives),
  so per-petal-switch materialization still spawns primitive nodes as
  fallback signs; `primitive_reconcile.rs` promotes them once selected.
- `primitive_reconcile.rs` — `reconcile_selected_primitive` (FR-1 promotion +
  FR-2 live reconcile) and `resolve_primitive_material` (FR-3 texture
  resolution via `fe_hexon::handlers::material::resolve_material_textures` +
  `FsBlobStore`). `PrimitiveMaterialAssets` holds the shared default material.
  Also owns the FR-5 wall systems `promote_selected_wall` + `wall_reconcile`
  and the `decode_gpx_points` helper (see §wall).

`find_petal_mut` on `VerseManager` stays private (not `pub(super)`) — Rust's
privacy rule already makes private items of a module visible to all of its
descendants, so `db_results.rs` can call it without widening the type's
public API.

## §primitives (FR-1..FR-4, `bim_primitives_on_paths_20260712`)

A primitive descriptor (`fe_sdk::primitive::PrimitiveDescriptor`) rides on a
node's `primitive` property as `PropertyValue::Json` (C5). The **only**
currently-wired source of per-node properties in owned files is
`InspectorFormState.node_properties` — populated for the **selected** node
via the existing `NodePropertiesLoaded` DB round-trip (owned by
`db_results.rs`, read-only here). `reconcile_selected_primitive` therefore
drives both materialization (FR-1) and live-edit reconcile (FR-2) off the
selected node only; petal-wide materialization for *all* primitive nodes on
petal switch requires `NodeEntry` to carry `properties`, which is fenced to
a future pass once `db_results.rs`'s `NodeEntry` construction sites can be
touched. `TextureRegistryRes` wraps the engine-decoupled
`fe_sdk::TextureRegistry` as a Bevy `Resource` (the SDK type itself has no
bevy dependency by design).

## §wall (FR-5/FR-6, C3, `bim_primitives_on_paths_20260712` P3)

A **wall** is a `PrimitiveKind::Wall` node whose shape is driven by a source
path's polyline (not `dims`). Descriptor shape (owned by `fe-sdk`, see
`fe-sdk/src/AGENTS.md` §primitive-wall): `dims = [height]` +
`source_path = Some(track_node_id)`.

**Geometry — `spawn.rs::build_wall_mesh(polyline, height)`.** Each consecutive
polyline segment `p[i]→p[i+1]` becomes one vertical quad (two triangles, 4
verts + 6 indices) rising `height` along +Y from the base points. The outward
normal is the horizontal perpendicular of the segment. It's a hand-built raw
`Mesh` (positions/normals/uvs/indices) mirroring the `bake_splat_mesh` idiom in
`fe-terrain/src/splat/render.rs` — but **without** any dependency on
fe-terrain internals (the mesh is assembled locally here). A wall entity's
`Transform` is identity: the polyline vertices are already petal-local world
coordinates (the same `gpx_points` the path renders from), so the geometry
carries its own position rather than being centred-and-translated like a cube.

**Point source.** A wall reads the **same DB-backed `gpx_points`** the path
line renders from (key `"gpx_points"`, a `[[x,y,z,t],...]` JSON array written
by `fractalengine/src/gpx_bridge.rs`). `decode_gpx_points` drops the `t`
component into the `[x,y,z]` polyline. We deliberately do **not** read the
editor's local point buffer (`PathEditorState`) — the DB row is the single
source of truth, so a wall matches whatever the persisted path is.

**Re-projection — driven entirely by Track 1's events (no `gpx_bridge` edit).**
`wall_reconcile` is a `MessageReader<DbResult>` system that reuses the exact
lifecycle events Track 1 (`path_node_binding_hardening`) already emits — Bevy
messages broadcast to every reader independently, so this system's cursor never
starves `advance_path_materialization`'s:
  - `DbResult::NodePropertiesLoaded { node_id, properties }` — fired by Track 1's
    petal-load batch (`request_petal_gpx_materialization`) **and** every live
    path edit (`persist_and_render_points` re-emits properties). Any spawned
    `WallNode` whose `source_path == node_id` rebuilds its mesh in place from the
    fresh points → a wall re-extrudes whenever its source path changes.
  - `DbResult::NodeDeleted { node_id, .. }` — any wall whose `source_path ==
    node_id` is despawned (its shape driver is gone; `PathOp::DeleteTrack`
    cascades to a `NodeDeleted`).

`promote_selected_wall` handles first materialization: when the selected node
carries a `Wall` descriptor and isn't yet a spawned `WallNode`, it despawns any
placeholder `FallbackSign`, spawns an empty-geometry wall, and issues one
`GetNodeProperties` for the `source_path` so the wall projects immediately
(rather than only on the next petal-load batch). Same selected-node scope
limitation as the shape primitives (§primitives) — petal-wide auto-spawn of
unselected wall nodes needs `NodeEntry.properties`, still fenced.

## §building-composition (FR-6)

A "building" needs **no new abstraction**. It is simply N `Wall` primitive
nodes (+ optional GLTF model nodes) grouped under one petal via the existing
verse/fractal/petal/node hierarchy. Each wall independently binds to its own
`source_path` and re-projects on that path's changes; they render together
because they share a petal and each spawns its own entity via the systems
above. A remodel = layering a GLTF model node next to/over the extruded walls
in the same petal. Per spec, do not build a grouping primitive/type.
