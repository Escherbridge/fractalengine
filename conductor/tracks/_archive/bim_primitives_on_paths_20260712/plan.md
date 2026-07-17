---
type: Implementation Plan
title: BIM Primitives on Paths
tags: [bim_primitives_on_paths_20260712]
resource: ./spec.md
---

# Implementation Plan: BIM Primitives on Paths

## Phase status (2026-07-14 alignment, recorded 2026-07-15)

- **P1 — IMPLEMENTED** (FR-1 closed petal-wide via `PrimitiveDescriptorCache` +
  `materialize_cached_primitives` + third `spawn_branch` in `petal_respawn.rs`; FR-2 live
  reconcile in place).
- **P2 — IMPLEMENTED** (FR-3 loader + FR-4 `TextureRegistry` landed).
- **P3 — SUPERSEDED** — GPX rip-walls landed separately in commit `1b39af3`; path→wall
  extrusion here is redundant. Do not execute.
- **P4 — DEFERRED** — blocked on the placeholder plugin transaction handlers
  (`fe-plugin/src/lib.rs:294` stub); FR-7/FR-9 park with it.
- **FR-8 stays OPEN** as the analytics seam (feeds the iot/analytics + BI-egress roadmap work).

## Overview

First-party-first, four phases. Geometry + the texture area land as engine code (P1–P2)
before anything extension-facing; the GPX merge (P3) depends on
[path_node_binding_hardening](../path_node_binding_hardening_20260712/spec.md) being landed
and verified; the API-first extension surface (P4) lands last and requires finishing the
stubbed plugin transaction handlers.

Each task is one TDD cycle (Red → Green → Refactor): write the named failing test first,
implement the minimum, refactor. Each phase ends with a `[checkpoint marker]`. Quality gates
every task: `cargo fmt`, `cargo clippy -- -D warnings`, `///` on public fns, no
`unwrap`/`expect` in prod paths. See [spec.md](./spec.md) for requirements, constraints
(C1–C6), and code seams.

## Build discipline (machine-specific, critical)

Never run concurrent cargo (crashes rustc 0xc0000409). `fe-ui` needs `-j 1`; others `-j 2`.
Always `$env:RUST_MIN_STACK="134217728"` and `$env:CARGO_TARGET_DIR="c:/tmp/fe-sweep-target"`.
Apply all fixes → run the full sweep ONCE at the end → rebuild → relaunch → require in-app
user verification before claiming done.

Sweep (add this track's crates to the second command):
```
cargo test -j 1 --no-fail-fast -p fe-ui
cargo test -j 2 --no-fail-fast -p fe-query -p fe-database -p fe-terrain -F fe-terrain/render \
  -p fe-renderer -p fractalengine -p fe-sdk -p fe-plugin -p fe-hexon
```

## Phase 1 — First-party primitives (FR-1, FR-2) — DONE

- **P1.1** Define the primitive descriptor: a `{kind, dims, texture_ref}` shape serialized as
  `PropertyValue::Json` on the `fe-runtime` bridge struct (`shared_node.rs:107`, per C5).
  Test: descriptor round-trips through the property bag.
- **P1.2** Add `spawn_primitive_entity` in `fe-ui/src/verse_manager/spawn.rs` mirroring
  `spawn_fallback_sign:39-75` — maps `kind` → `Cuboid`/`Sphere::new`/`Cylinder`/`Plane3d`, adds
  `Mesh3d`+`MeshMaterial3d(shared default)`+`Transform`+`SpawnedNodeMarker`. Test: each kind
  spawns the expected mesh handle.
- **P1.3** Add the third branch in `petal_respawn.rs:58-95`: node has `primitive` prop →
  `spawn_primitive_entity`; else existing GLTF/fallback branches. Test: a primitive node
  materializes; a non-primitive node is unaffected.
- **P1.4** FR-2 live edits: reconcile primitive entities by `SpawnedNodeMarker` on
  `PropertyChanged` (mirror splat reconcile discipline in `splat/render.rs:119-181`) so
  dims/kind changes re-mesh without respawn stutter. Test: changing `dims` updates the mesh.
- `[checkpoint marker]`

## Phase 2 — Texture area (FR-3, FR-4) — DONE

- **P2.1** `MaterialHandle` → `StandardMaterial` loader near `fe-hexon/src/handlers/material.rs`:
  resolve role→blob-hash, load blobs from `FsBlobStore` into Bevy `Image`s, assemble a
  `StandardMaterial`. Test: a handle with an albedo blob yields a material with a base-color
  texture.
- **P2.2** `TextureRegistry` copy-adapted from `UiExtensionRegistry` (`fe-sdk/src/ui/mod.rs:43`):
  `register`/`unregister_all(plugin_id)`/`get`, entries are hexon-installed blob refs (C6). Test:
  register/unregister lifecycle.
- **P2.3** Wire a primitive's `texture_ref` through the registry+loader; `None` → shared default
  material. Test: a primitive with a `texture_ref` renders with the loaded material.
- `[checkpoint marker]`

## Phase 3 — Path → wall (FR-5, FR-6) — SUPERSEDED by GPX rip-walls (commit `1b39af3`)

- **P3.1** Wall mesh builder: extrude a `gpx_points` polyline into a quad-strip wall of a given
  height (`bake_splat_mesh` at `splat/render.rs:313` is the raw-mesh reference). Pure function,
  test on a known polyline → expected vertex/index counts + area.
- **P3.2** Wall primitive node bound to its source path node; re-projects on the hardened path
  lifecycle events (from Track 1's change-notification backbone). Test: editing the path
  re-projects the wall.
- **P3.3** Building composition: group wall primitives (+ optional layered GLTF model nodes)
  under a petal; a remodel = a model layered over the walls. Uses the existing node hierarchy —
  no new grouping primitive. Test: a building = N walls under one petal materializes as N wall
  entities.
- `[checkpoint marker]`

## Phase 4 — API-first extension surface (FR-7, FR-8, FR-9) — DEFERRED (except FR-8, open as the analytics seam; see `fe-plugin/src/lib.rs:294` stub)

- **P4.1** Finish the stubbed plugin transaction handlers at `fe-plugin/src/lib.rs:285-380` so
  create/delete/set-property/commit forward to the DB thread. Test: a transaction actually
  persists.
- **P4.2** Geometric-transform ops (translate/rotate/scale/extrude-path-to-wall/set-texture)
  over `ApiExtensionHandle` routes and/or `PluginTransaction`. Test: an API transform mutates
  the node and re-projects render.
- **P4.3** Statistical-analysis reads (counts, bounds, path length, wall area/volume) over the
  existing query/API surface. Test: stats over a known building return expected metrics.
- `[checkpoint marker]`

## Final

Run the serial sweep ONCE (with this track's crates), rebuild, relaunch in background, and
require in-app user verification per the spec's Verification section before claiming done.
