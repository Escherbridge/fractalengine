---
type: Track Spec
title: BIM Primitives on Paths
description: First-party 3D shape primitives with a hexon-backed texture area, path-to-wall extrusion that merges with the GPX layer, and an API-first geometric-transform / statistical-analysis surface extensions can drive over the API.
tags: [feature, bim_primitives_on_paths_20260712, in_progress]
timestamp: 2026-07-12T00:00:00Z
resource: ./metadata.json
---

# Overview

Expose BIM-like primitives early: a "Blender super-ultra-lite" that lets a petal hold simple
3D shape primitives (cube, plane, cylinder, sphere) with a **texture area** extensions can
append textures to, and — the north star — **merge with the GPX path work** so a path can be
extruded into a **wall**, walls composed into **buildings**, and building models **layered in
to design remodels**.

Interaction is **API-first**. Extensions do not get an interactive gizmo/pointer subsystem in
v1; they operate over the existing API surface. Extensions are for **geometric transformations
and statistical analysis**, not hand-driven editing tools. This keeps the scope to net-new
*data + render* surface rather than a net-new *input* subsystem.

Related context: [product.md](../../product.md) (digital-twin / GIS petals),
[tech-stack.md](../../tech-stack.md) (Bevy 0.18), [workflow.md](../../workflow.md)
(TDD red→green, >80% coverage, fmt + clippy -D warnings). Sequenced after and dependent on
[path_node_binding_hardening](../path_node_binding_hardening_20260712/spec.md) — the
path→wall phase requires the hardened DB↔render↔panel binding to build walls from paths
reliably.

# Background

- 3D primitives are effectively free: Bevy shape structs (`Cuboid`, `Sphere`, `Plane3d`,
  `Cylinder`) are **already used** in `fe-terrain/src/terrain_plugin.rs:574,674` and
  `fe-ui/src/verse_manager/spawn.rs:39-75`. No custom mesh baking needed for the primitives
  themselves.
- The Node→Bevy bridge is real and localized in `fe-ui/src/verse_manager/`. The
  materialization loop `petal_respawn.rs:58-95` already branches "GLTF asset vs. fallback
  cuboid placard" per node. A primitive is a **third branch**.
- A node's params ride on the existing `properties: HashMap<String, PropertyValue>` map — a
  primitive needs **no new value type**; a `PropertyValue::Json` descriptor
  (`{kind, dims, texture_ref}`) is sufficient.
- The nearest working template for "shared material + swappable texture + reconcile-by-marker"
  is `fe-terrain/src/splat/render.rs` (SplatPlugin): `init_splat_assets` (:87-103) builds a
  shared `StandardMaterial` + procedural `Image` once into a Resource; `reconcile_splat_chunks`
  (:119-181) spawns/despawns by marker; `make_soft_disc_image` (:367) builds a texture from raw
  RGBA bytes.
- The **texture area does not exist yet**. `fe-hexon/src/handlers/material.rs:13`
  (`MaterialHandle`) maps texture roles → content-addressed blob hashes but only *verifies*
  blobs on install — there is **no `MaterialHandle` → live Bevy `StandardMaterial` loader**.
  That loader is the missing piece.
- The extension API is data-level only. SDK nodes are `{name, transform, property-bag}` with
  **zero geometry/asset/interaction surface**. `UiContribution` (`fe-sdk/src/ui/mod.rs:28`) is
  a behaviorless label; `FractalExtension` (`fe-plugin/src/lib.rs:138-189`) exposes
  `on_scene_change`/`on_tick` + lifecycle + four transaction ops. The plugin transaction
  handlers at `fe-plugin/src/lib.rs:285-380` are **placeholder stubs** ("Phase 9A.2 will
  forward to DB thread") and must be finished before extensions can mutate nodes end-to-end.

# Constraints (fixed user decisions — non-negotiable)

- **C1 — API-first, no gizmo subsystem in v1.** Do NOT build an interactive pointer / hit-test
  / gizmo event pipeline. Extensions and callers drive geometry over the existing API. This is
  intended and explicitly in scope ("people can likely interact over api just fine").
- **C2 — Extensions = geometric transforms + statistical analysis.** The extension surface
  targets transform ops (translate/rotate/scale/extrude/boolean-ish) and stats over node/geometry
  sets — not interactive editing tools. Defer extension-registered *primitive types* and
  interactive tools entirely.
- **C3 — Merge with GPX.** Primitives compose with the GPX path layer: a `gpx_points` polyline
  extrudes into a wall; a building is a set of wall primitives (+ optional layered GLTF models
  under a petal) for remodel design. This is the reason for the dependency on
  path_node_binding_hardening.
- **C4 — First-party primitives ship before any extension surface.** Geometry + texture area
  land as engine code; extension access comes last (P4).
- **C5 — Params ride on the property bag.** Primitive descriptors serialize through the
  **`fe-runtime` `PropertyValue`** (`shared_node.rs:107`, the Tauri↔Bevy bridge struct) using
  the `Json` variant. The `fe-sdk` `PropertyValue` (`property.rs:13`) is the extension-facing
  mirror; keep the two shapes convertible but do NOT introduce a third. Record any divergence
  as a follow-up, do not fork geometry semantics across the two.
- **C6 — v1 textures are hexon-installed blobs only.** Extensions append textures by
  referencing content-addressed `MaterialHandle` blobs already installed via a hexon package —
  NOT raw byte upload from a sandboxed plugin. Raw upload (capability-gated + format-validated)
  is deferred; it is the expensive/unsafe path and out of scope for v1.

# Functional Requirements

- **FR-1 — Primitive descriptor + render branch.** A node carrying a `primitive` JSON property
  (`{kind: cube|plane|cylinder|sphere, dims:[..], texture_ref:Option<String>}`) materializes as
  the corresponding Bevy mesh entity. Add a third branch to `petal_respawn.rs:58-95` and a
  `spawn_primitive_entity` in `spawn.rs` mirroring `spawn_fallback_sign`. Entities carry
  `SpawnedNodeMarker` for despawn/keep.
- **FR-2 — Live dimension edits.** Editing a primitive's `dims`/`kind`/`texture_ref` via the
  existing `PropertyChanged` inspector channel updates the rendered mesh/material without a
  respawn stutter (reconcile by marker, mirror the splat visibility/reconcile discipline).
- **FR-3 — Texture area (MaterialHandle → StandardMaterial loader).** Build the missing loader:
  resolve a `MaterialHandle`'s role→blob-hash map from `FsBlobStore`, load blobs into Bevy
  `Image`s, assemble a `StandardMaterial`. A primitive's `texture_ref` resolves through this
  loader. One shared default material when `texture_ref` is `None`.
- **FR-4 — Texture registry (append-only, per-plugin).** A `TextureRegistry` copy-adapted from
  `UiExtensionRegistry` (`register` / `unregister_all(plugin_id)` / `get`) that lists available
  textures (hexon-installed blobs, per C6). Primitives reference registry entries by id.
- **FR-5 — Path → wall extrusion (the GPX merge; requires Track 1).** A `gpx_points` polyline
  extrudes into a wall mesh (quad strip along the polyline × a height param). A wall is itself a
  primitive-kind node bound to the source path node; when the path changes (post-hardening
  events), the wall re-projects. `bake_splat_mesh` (`splat/render.rs:313`) is the raw-mesh
  reference for hand-built geometry.
- **FR-6 — Building composition.** A "building" is a set of wall primitives (+ optional layered
  GLTF model nodes) grouped under a petal, so a remodel = layering a model over/next to the
  extruded walls. No new grouping primitive beyond the existing node hierarchy is required.
- **FR-7 — API-first geometric-transform surface.** Expose transform ops (translate, rotate,
  scale, extrude-path-to-wall, set-texture) to callers/extensions over the existing API
  (`ApiExtensionHandle` routes and/or the `PluginTransaction` ops). No interactive input.
- **FR-8 — API-first statistical-analysis surface.** Expose read-side stats over node/geometry
  sets (counts, bounds, path length, wall area/volume) via the existing query/API surface.
- **FR-9 — Finish stubbed plugin transaction wiring.** Before FR-7/FR-8 can be relied on
  end-to-end, complete the placeholder handlers at `fe-plugin/src/lib.rs:285-380` so
  create/delete/set-property/commit actually forward to the DB thread.

# Out of scope (deferred)

- Interactive gizmos / pointer-driven editing tools (no input-event subsystem — C1).
- Extension-registered *new primitive types* (needs geometry-model extension + finished plugin
  wiring — C2).
- Raw texture-byte upload from sandboxed plugins (C6 — hexon blobs only in v1).
- Boolean/CSG solid modeling, curved walls, roofs — v1 is straight-extrusion walls only.

# Phasing

1. **P1 — First-party primitives.** FR-1, FR-2. Third render branch + `spawn_primitive_entity` +
   property-driven descriptor. Ships primitives on screen, editable via inspector.
2. **P2 — Texture area.** FR-3, FR-4. `MaterialHandle`→`StandardMaterial` loader + `TextureRegistry`.
   The cheapest, highest-value extension seam.
3. **P3 — Path → wall (GPX merge; requires Track 1 landed).** FR-5, FR-6. Extrude polyline → wall,
   compose buildings, layer models for remodels.
4. **P4 — API-first extension surface.** FR-7, FR-8, FR-9. Geometric-transform + stats ops over
   the existing API; finish the stubbed plugin transaction handlers.

# Verification

- In-app: spawn each of cube/plane/cylinder/sphere; edit dims live; apply a hexon texture to a
  primitive; extrude a GPX path into a wall and confirm it re-projects when the path changes;
  drive a transform + a stats query over the API.
- Full serial test sweep ONCE at end (see Environment/build discipline in the plan), including
  `-p fe-sdk -p fe-plugin -p fe-hexon`.
