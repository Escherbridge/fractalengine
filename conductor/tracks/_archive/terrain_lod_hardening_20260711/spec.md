---
type: Track Spec
title: Terrain LOD Hardening — Seams, Clipping, Close-Range Quality
tags: [bug, terrain, rendering, terrain_lod_hardening_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: Terrain LOD Hardening

**Track ID:** `terrain_lod_hardening_20260711`
**Crates:** `fe-terrain`, `fe-renderer`

## Problem (user report 2026-07-11, screenshot on file)

At region scale (world_scale < 1) three defects show:
1. **Seams** — thin black vertical lines between adjacent tile chunks.
2. **Zoom-out clipping** — holes/clipped chunks at the view edge when zoomed
   far out (fetch ring / despawn distance / far plane disagree).
3. **Close-range quality drop** — blocky terrain at the lowest level; the
   camera outruns the tileset's max zoom and the mesh/texture stay coarse.

## Functional Requirements

- **FR-1 Seam removal:** adjacent chunks share edge elevations (tile border
  row/column sampling must agree) and/or chunks grow mesh skirts so no
  background shows through. Pure geometry helpers unit-tested.
- **FR-2 Fetch/despawn coherence:** the spawn ring must cover the visible
  frustum implied by the scale-aware far plane; despawn uses hysteresis
  (despawn radius > spawn radius) so tiles never flicker or leave holes while
  still visible. Ring size adapts to camera height/scale instead of fixed 3×3.
- **FR-3 Close-range quality:** when desired zoom exceeds the tileset max,
  serve max-zoom tiles with bilinear-interpolated elevation upsampling and a
  denser mesh so geometry stays smooth; document (AGENTS.md) that satellite
  texture sharpness is capped by the source hexon — higher-zoom hexons are a
  gis-tile-etl concern.
- **FR-4 No regressions at 1:1** — existing behavior and tests keep passing.

## Out of scope

New hexon formats, splat rendering (separate track), gis-tile-etl changes.
