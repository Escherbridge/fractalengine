---
type: Track Spec
title: Splat LOD Zoom — Camera-Distance-Driven Splat Resolution
tags: [feature, terrain, splat, rendering, lod, splat_lod_zoom_20260712]
timestamp: 2026-07-12T00:00:00Z
resource: ./metadata.json
---

# Specification: Splat LOD Zoom

**Track ID:** `splat_lod_zoom_20260712`
**Crates:** `fe-terrain` (splat + tiles + lod_ring)

## Problem (user, 2026-07-12)

After `terrain_splat_view_20260711` phase 1 (mesh alignment) and round-3
polish (jitter/overlap tuning to fix the "dot-grid" look,
`fe-terrain/src/splat/synth.rs`), splats still read as low-resolution.
Root cause: `reconcile_splat_chunks` (`fe-terrain/src/splat/render.rs:119-
181`) builds each splat chunk from `chunk.tile_coords` — the **mesh**
chunk's already-chosen `(zoom, x, y)` — with no independent splat-specific
zoom selection. Splat density is therefore capped by whatever zoom level
the mesh LOD system (`update_terrain_lod` + `lod_ring.rs`) currently has
that chunk bound to, regardless of how close the camera actually is.

User's explicit direction (2026-07-12, choosing between two candidate
fixes): prioritize the **tile/LOD zoom pipeline** (Esri-style "zoom in for
higher-resolution imagery/elevation") over a marching-squares-style splat
interpolation/upscaling approach. The latter remains a valid future
follow-up if this track's ceiling (tileset's actual max zoom) is reached
and more density is still wanted.

## Grounding facts (research, 2026-07-12 — see conversation for full report)

- `CompositeTileSource::get_tile_sync(coord: TileCoord)` /
  `fetch_tile(coord)` (`fe-terrain/src/tiles/composite.rs`) take an
  arbitrary `TileCoord` per call — the composite source itself is not
  pinned to one zoom. Fetching a *higher* zoom than the mesh chunk's
  current zoom is already structurally supported, bounded only by
  whatever `min_zoom..max_zoom` the active hexon source(s) cover
  (`hexon_source.rs::covers`) or by disk/online fallback availability.
- `desired_zoom(cam_height_m, min_zoom, max_zoom)`
  (`fe-terrain/src/terrain_plugin.rs:147-158`) is the existing "zoom for
  camera distance" function used by the mesh LOD system — one zoom step
  out per doubling of height above `ZOOM_BASE_HEIGHT_M` (200m). This is
  the natural model to mirror (or literally reuse) for a splat-specific
  desired zoom.
- `lod_ring.rs::covering_tiles(coord, target_zoom)` already computes the
  self/children/parent tile coordinates needed to cover one tile's
  footprint at a *different* target zoom — this is exactly the coordinate
  math needed when a splat chunk wants to fetch at a higher zoom than its
  shadowed mesh chunk's `tile_coords`.
- `TerrainConfig.min_zoom`/`max_zoom` (`petal_binding.rs`) bound the
  allowed range per petal's tileset — a splat-specific desired-zoom
  function must clamp into the same range (there is no "splat max zoom"
  concept separate from the tileset's own bounds; requesting beyond
  `max_zoom` should fall back to the tileset's max, not fail).

## Functional Requirements

- **FR-1 Splat-specific desired zoom:** add a `splat_desired_zoom(
  cam_height_m, mesh_zoom, min_zoom, max_zoom) -> u8` (pure, unit-tested,
  bevy-free per the crate's existing pure/render split convention) that
  can request a *higher* zoom than the mesh chunk's own `mesh_zoom` when
  the camera is close — e.g. one or two extra zoom steps at close range,
  clamped to `max_zoom`. At far range it should not exceed `mesh_zoom`
  (no reason to over-fetch splats for terrain that's barely on screen).
- **FR-2 Multi-tile splat coverage:** when `splat_desired_zoom(...) >
  chunk.tile_coords.0` (the mesh zoom), one mesh chunk's footprint now
  needs *multiple* higher-zoom splat sub-tiles to cover it (a zoom step up
  roughly quadruples tile count per the standard slippy-tile scheme).
  Use `lod_ring::covering_tiles` (or extend it if it doesn't already
  produce "children at a higher zoom" — confirm during implementation)
  to compute which sub-tile coords to fetch, and adjust `SplatChunk`
  (`fe-terrain/src/splat/render.rs:79-84`) to own a `Vec` of baked
  sub-meshes anchored within the parent mesh chunk's transform, instead
  of assuming a 1:1 chunk-to-splat-mesh mapping.
- **FR-3 Fetch + bake at the higher zoom:** `build_tile_splat_mesh`
  (`render.rs:256-309`) currently derives `TileCoord` straight from
  `chunk.tile_coords` — extend the call site in `reconcile_splat_chunks`
  to pass the FR-1/FR-2-computed sub-tile coords instead, fetching via
  `composite.get_tile_sync`/`get_satellite_tile_sync` at the higher zoom
  (already supported per the grounding facts above — no composite/cache
  changes needed).
- **FR-4 Despawn/respawn coherence on zoom change:** as the camera moves
  closer/farther, previously-baked splat sub-tiles at a stale zoom must
  be despawned and re-baked at the new `splat_desired_zoom`, budgeted per
  frame (mirror the existing spawn budget pattern already in
  `reconcile_splat_chunks`) so a rapid zoom doesn't spike frame time.
  Mirror `lod_ring::wrong_zoom_replacement_present`'s hole-free-transition
  gating if applicable (don't despawn the stale splat tile until its
  higher-zoom replacement is ready).
- **FR-5 Graceful ceiling:** when `splat_desired_zoom` would exceed the
  tileset's actual `max_zoom` (i.e. no higher-resolution source tiles
  exist), clamp to `max_zoom` and keep current density — this is the
  hand-off point to a future marching-squares/interpolation follow-up
  track, not a failure state. No error, no retry storm.

## Out of scope (v1)

Marching-squares-style splat interpolation/upscaling beyond the tileset's
native max zoom (explicitly deferred per user direction — separate future
track if this one's ceiling is reached); per-splat individual LOD
(splats within one baked chunk mesh stay uniform resolution, only the
chunk's own zoom/tile granularity changes); mesh LOD changes (this track
is splat-only; `terrain_lod_hardening_20260711` owns mesh seam/clipping/
close-range work).

## Verification

Unit tests for FR-1 (`splat_desired_zoom` monotonicity/clamping, mirroring
the existing `desired_zoom_monotonic_in_height`/`desired_zoom_clamped_to_
range` test style in `terrain_plugin.rs`) and FR-2's coverage math. In-app
verification: user zooms in close on a splat-view petal and confirms
visibly denser/sharper splat coverage at close range, with no seam/hole
regressions and acceptable frame time during rapid zoom.
