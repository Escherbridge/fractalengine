---
type: Track Spec
title: Terrain Splat View — Synthesized 3D Splats from Hexon Data
tags: [feature, terrain, rendering, hexon, terrain_splat_view_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: Terrain Splat View

**Track ID:** `terrain_splat_view_20260711`
**Type:** Feature (phased)
**Crates:** `fe-terrain`, `fe-renderer` (phase 1); `fe-format`, `fe-hexon`, sibling `gis-tile-etl` (phase 2)

## Overview

Render terrain environments as 3D splats generated from hexon tile data, as
an alternative view mode beside the existing mesh renderer. Scoping honesty:
photogrammetric Gaussian-splat *training* requires multi-view imagery and is
out of scope — hexon tiles carry one orthographic satellite view. What this
track builds is **synthesized splats**: one splat per elevation texel
(position from tile lat/lon + decoded elevation, color sampled from the
satellite tile, scale/orientation from local slope), rendered via instanced
quads/point sprites with soft falloff. Visual result: a soft, fast,
LOD-friendly environment representation that degrades gracefully at distance
— a natural companion to the multi-scale work in
`terrain_scale_controls_20260711`.

## Functional Requirements

### FR-1: Tile → splat synthesis (runtime, phase 1)
Pure function: `(elevation pixels, satellite pixels, tile geo-params,
world_scale) → SplatBuffer { positions, colors, scales, normals }`, with
configurable stride (1 = per-texel, N = decimated). Slope-aware anisotropy
(flat ground → wide flat splats; cliffs → oriented ellipses). Unit-tested on
synthetic gradients. Respects `world_scale` and the grounded origin exactly
like `terrain_mesh`.

### FR-2: Splat rendering (phase 1)
Instanced rendering in fe-renderer or fe-terrain's render module — no external
splatting crate dependency unless one is verified compatible with Bevy 0.18
(prefer built-in instancing + a small custom material; alpha-blended soft
discs is the v1 bar, depth-sorted per-tile). Budget: 3×3 ring at stride 2
(~150k splats) ≥ 60fps on the dev machine.

### FR-3: View modes (phase 1)
`TerrainViewMode { Mesh, Splats, Hybrid }` resource + UI toggle beside the
terrain settings from `terrain_scale_controls_20260711`. Hybrid = mesh near /
splats far (distance threshold scale-aware). Mode persists in the petal
terrain JSON (additive field, serde default `Mesh`).

### FR-4: Pre-optimized splat buffers in hexons (phase 2)
Additive hexon entry type (`splats/{z}/{x}/{y}.bin`, packed
pos+color+scale+normal, quantized) + manifest flag `splat_ready`. Install/load
path: `HexonTileSource` serves precomputed buffers when present, falls back to
FR-1 runtime synthesis when absent (format stays backward-compatible both
directions). `gis-tile-etl` gains a splat-bake stage. Bake tool also runnable
locally: `cargo run -p fe-hexon --example bake_splats <hexon>`.

## Out of scope
Multi-view 3DGS training; spherical-harmonics view-dependent color; splats for
non-terrain hexon types (Scene/GpxCollection) — future track.

## Dependencies
Builds on `terrain_scale_controls_20260711` (world_scale + settings UI
surface). Phase 2 relates to `hexon_delta_format_20260710` /
`hexon_p2p_bucket_20260710` (additive archive entries are the same mechanism
the bucket vision uses).
