---
type: Track Retro
title: Terrain Scale Controls — Retrospective
tags: [retro, terrain, terrain_scale_controls_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Retro: Terrain Scale Controls

## What shipped

Commit `66395c5` — `world_scale` (world units per real meter, additive serde
field) in `TerrainConfig`, pure scale math in `fe-terrain/src/scale.rs`,
scale-aware camera (`CameraScaleSettings`: far plane, zoom-out ceiling,
proportional log zoom) in `fe-renderer/src/camera.rs`, and a "Terrain scale"
row (presets 1:1 → 1:1000 + log slider, live camera preview, persistence via
`SetPetalTerrain`) in the Hexon Manager Installed tab. 21 new tests.

## Verification

User confirmed in-app 2026-07-11 ("this is far better") with a region-overview
screenshot of the Pacific-NW tileset at reduced scale — the goal (region
overview and human scale coexisting per petal) is met.

## What went well

- Pure-math module (`scale.rs`) kept every scale rule unit-testable without
  Bevy, and the compose-both-terms invariant (anchor and mesh Y scaled
  together) landed correctly on the first sweep.
- Reusing the `SetPetalTerrain → PetalTerrainLoaded → TerrainAssignmentMsg`
  round-trip meant live scale changes needed zero new plumbing.

## What it surfaced (now tracked separately)

Operating at region scale exposed pre-existing renderer gaps that 1:1 scale
had hidden — inter-tile seam gaps, zoom-out clipping/holes, and blocky
close-range terrain past the tileset's max zoom. These moved to
`terrain_lod_hardening_20260711` rather than scope-creeping this track.

## Lessons

- A scale knob is also a magnifying glass: any feature that changes the
  viewing envelope needs a follow-up QA pass at the envelope's extremes.
- E0382 gotcha: overriding config fields from a borrowed source struct needs
  `.clone()` on non-Copy enums (partial-move out of a borrowed `TilesetMeta`).
