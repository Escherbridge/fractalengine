---
type: Track Spec
title: Splat Hexon Bake — Precompute Coverage at Build Time, Load at Runtime
tags: [feature, terrain, splat, hexon, rendering, performance, splat_hexon_bake_20260712]
timestamp: 2026-07-12T00:00:00Z
resource: ./metadata.json
---

# Specification: Splat Hexon Bake

**Track ID:** `splat_hexon_bake_20260712`
**Crates:** `fe-terrain` (splat + tiles), `fe-hexon` (format field)

## Problem (user, 2026-07-12)

Splats render with dark holes between blobs at close zoom. The
coverage-driven hole-fill that closes them (from the reverted
`splat_lod_zoom_20260712`) is correct visually but ran **live** — O(n²) per
sub-tile, synchronously in `reconcile_splat_chunks` every time chunks bake.
A zoom-in burst spiked frame time / the allocator and **crashed the app**.

User's directive: *"offload the splat data ahead of time to the hexon file
and just load it."* The fill is expensive but **deterministic for a given
tile** — so compute it once at build time, store the dense result, and load
it at runtime with zero per-frame fill cost.

## Approach

Move the coverage fill from render-time to **hexon/tileset build time**:

1. **Bake step (offline / tileset build):** when a hexon/tileset is built
   (`fe-terrain/src/tiles/builder.rs`, `TilesetBuilder`), run the coverage
   hole-fill once per tile and store the resulting dense splat buffer
   (positions/colors/scales/normals) in the hexon file.
2. **Format field (`fe-hexon`):** add a hexon-format field/section holding
   the baked splat buffer per tile (`fe-hexon/src/package.rs` and the
   manifest). Versioned/optional so existing hexons without baked splats
   still load (fall back to live synth at baked density, no fill).
3. **Runtime load (`fe-terrain`):** `build_tile_splat_mesh` /
   `reconcile_splat_chunks` read the baked buffer from the hexon source
   (`fe-terrain/src/tiles/hexon_source.rs`) and bake the mesh directly —
   **no `augment_splat_buffer_coverage` call in the hot path.** If a tile
   has no baked splat data, fall back to the current live `synthesize_splats`
   at native density (holes visible but no crash, no fill cost).

## Functional Requirements

- **FR-1 Bake-time coverage fill:** lift the reverted coverage-fill
  algorithm (gap-sized fill radius, logarithmic shrink, tile-seam guard,
  degenerate-cluster floor — see `interpolate.rs` on branch
  `archive/splat-coverage-experiment-20260712`) and run it once per tile at
  tileset build time. Pure/bevy-free, unit-tested (the archived tests port
  over). Use a **spatial grid** for the neighbor / cluster-proximity lookups
  so the bake is ~O(n) not O(n²) — the archived live version's O(n²) cluster
  guard is the specific hot spot to replace here.
- **FR-2 Hexon format field:** store the baked splat buffer per tile in the
  hexon format (`fe-hexon`). Optional/versioned: absent field → runtime
  falls back to live synth. Round-trip (write at bake, read at load) tested.
- **FR-3 Build-time integration:** wire the bake into `TilesetBuilder`
  (`fe-terrain/src/tiles/builder.rs`) so building/publishing a hexon
  produces the baked splat coverage. Budget/parallelism at build time is
  fine (it's offline) — no per-frame constraint.
- **FR-4 Runtime load, no live fill:** `build_tile_splat_mesh` reads the
  baked buffer when present and bakes the mesh from it directly; the
  `augment_splat_buffer_coverage` live call is NOT reintroduced into
  `reconcile_splat_chunks`. Confirm no O(n²) work remains in the render hot
  path.
- **FR-5 Graceful fallback:** tiles/hexons without baked splat data render
  via the existing live `synthesize_splats` at native density (current
  behavior — holes, but stable). No crash, no live fill, no error.

## Out of scope

Live/runtime hole-filling of any kind (the whole point is to remove it);
LOD zoom-selection changes (`splat_desired_zoom` — complementary, revisit
separately if real higher-res tiles are wanted); marching-squares
interpolation beyond baked density; mesh LOD.

## Verification

Unit tests: FR-1 fill correctness + O(n) spatial-grid neighbor lookup
(port the archived coverage tests — closes-holes, no-fill-when-covered,
gap-scaled/log-shrink, seam, cluster floor). FR-2 hexon round-trip. In-app:
build a hexon with baked splats, load a splat-view petal, zoom in close —
holes are filled AND there is **no crash / no frame spike** during a rapid
zoom-in burst (the failure mode this track exists to fix). A hexon without
baked splats loads and renders (holey but stable) via the fallback.

## Provenance

Reverted predecessor `splat_lod_zoom_20260712` (in `conductor/tracks/_archive/`).
Reverted code on branch `archive/splat-coverage-experiment-20260712`
(commits `9839094`, `4be8ec4`) — `fe-terrain/src/splat/interpolate.rs` there
is the algorithm to lift and optimize for the bake step.
