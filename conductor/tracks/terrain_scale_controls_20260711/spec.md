---
type: Track Spec
title: Terrain Scale Controls — Multi-Scale Space Operation
tags: [feature, terrain, camera, ui, terrain_scale_controls_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: Terrain Scale Controls

**Track ID:** `terrain_scale_controls_20260711`
**Type:** Feature
**Crates:** `fe-terrain`, `fe-renderer`, `fe-ui` (+ petal terrain JSON)

## Overview

Terrain renders at real-world meters: a zoom-8 tile is ~110 km wide while the
camera's far plane is 1 km and the orbit controller is tuned for human-scale
scenes. Users cannot "operate at several different scales of space" — region
overview and human-scale placement are mutually exclusive. This track adds a
per-petal **world scale** for the terrain plus scale-aware camera zoom
controls and a settings UI.

## Functional Requirements

### FR-1: `world_scale` in TerrainConfig
`TerrainConfig` gains `world_scale: f64` (serde default `1.0`; additive —
existing stored petal terrain JSON keeps parsing). Semantics: world units per
real meter (0.001 → 1 unit per km). Applied in chunk spawning (anchor
transform, tile mesh size, decoded elevation heights) and inverted for
camera→wgs84 zoom selection (`desired_zoom` sees real-world height).
Acceptance: unit tests for scaled mesh size, scaled anchor, inverse camera
mapping; scale change on a live assignment respawns chunks (revision bump).

### FR-2: Scale-aware camera
Orbit camera zoom-out limit and far plane must follow scale so the whole
region fits in view at small scales and human scale still works at 1:1.
Zoom (scroll) speed proportional to current distance (log zoom).
Acceptance: at 0.001 scale the full PNW tileset fits the frustum; at 1.0 the
existing near-scene behavior is unchanged.

### FR-3: Terrain settings UI
A terrain settings surface in fe-ui (with the map controls, near the Hexon
manager / active-map affordances): scale presets ("1:1 human", "1:10",
"1:100", "1:1000 region") + log-scale slider, persisted into the petal's
terrain JSON through the existing SetPetalTerrain flow so it round-trips via
`PetalTerrainLoaded` → `apply_terrain_assignments`. fe-ui must NOT depend on
fe-terrain (JSON via serde_json — standing boundary rule).
Acceptance: changing scale updates the live terrain without restart and
persists across app restarts.

### FR-4 (stretch): zoom-range override
Optional min/max zoom override in the same settings (clamped to the tileset's
available range) for tilesets with multiple zoom levels.

## Out of scope
Dynamic LOD streaming beyond the existing 3×3 ring; multi-tileset blending;
camera collision.
