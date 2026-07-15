---
type: Track Spec
title: Hexon Scale Orchestration + Rulers
description: Hexon-authoritative real-world scale in the GIS data layer, multi-source metric-frame reconciliation, and a rulers/measurement/grid/annotation layer built on fe-terrain scale math.
tags: [feature, hexon_scale_orchestration_20260712, in_progress]
timestamp: 2026-07-12T00:00:00Z
resource: ./metadata.json
---

# Overview

Push real-world scale into the hexon GIS data layer so that the *hexon is authoritative*
for how large its terrain renders, reconcile multi-source composites into a single common
metric frame using per-source ground-sample-distance (GSD), and build a rulers layer
(scale bar, measurement tools, adaptive world grid, dimensioned annotations) on top of the
existing pure-math seam in `fe-terrain/src/scale.rs`.

Scale must travel through the data layer *before* anything renders it, so the plan is
strictly **data-layer-first**: format fields (Phase 1) → composite reconciliation +
authoritative binding (Phase 2) → ruler pure math (Phase 3) → the three UI layers
(Phases 4-6).

Related context: [product.md](../../product.md) (digital-twin / GIS petals),
[tech-stack.md](../../tech-stack.md) (Bevy 0.18, bevy_egui 0.39), and
[workflow.md](../../workflow.md) (TDD red→green, >80% coverage, fmt + clippy -D warnings).

# Background

- The `.hexon` terrain tileset format already carries geographic bounds and zoom range but
  has **no notion of real-world scale, CRS, or resolution**. Render size is currently a
  free-form per-petal `world_scale` slider shipped by the prior track
  `terrain_scale_controls_20260711` (now archived).
- The engine already computes real meters from tiles via Web-Mercator
  (`tile_world_size_m(lat, zoom)` at `fe-terrain/src/terrain_plugin.rs:169`,
  `40_075_016.686 * cos(lat) / 2^zoom`), and `fe-terrain/src/scale.rs` already holds pure
  scale math (`sanitize_world_scale`, `scaled_tile_size`, `world_to_real_height`,
  `scale_local`, `scale_elevations`).
- There is a precedent for hexon authority: loaded hexons already override the
  elevation-encoding at `fe-terrain/src/petal_binding.rs:159-173`. Scale should follow the
  same "hexon is authoritative, user gets a clamped nudge" pattern.
- Multi-source composites (`CompositeTileSource`) currently pick the first source whose
  `covers()` returns true and **discard every source's `TilesetMeta`**, so mixed-resolution
  sources are never reconciled into a common metric frame.

# Constraints (fixed user decisions — non-negotiable)

- **C1 — Hexon-authoritative scale.** The hexon's declared native scale binds the render.
  The per-petal user `world_scale` becomes a *clamped nudge within hexon-declared bounds*,
  not free-form. Mirrors the existing elevation-encoding override.
- **C2 — Common metric frame + per-source GSD reconciliation.** When a composite pulls from
  multiple hexon sources at different native resolutions, `CompositeTileSource` must carry
  each source's `TilesetMeta` and reconcile them into one real-meter frame, using per-source
  GSD to drive LOD/zoom selection.
- **C3 — Full four-capability scope in this track:** (a) scale metadata in format + data
  layer, (b) scale bar HUD, (c) measurement tools (tape / area / bearing), (d) adaptive
  world grid + dimensioned annotations.
- **C4 — Home for the math.** A new `fe-terrain::ruler` pure-math module next to `scale.rs`;
  format fields go in `fe-format`. **No new crate.**
- **C5 — Update the EXISTING hexon format.** The authoritative struct is `TilesetMeta` in
  `fe-format/src/manifest.rs:30`. New fields must be **additive serde fields with defaults**
  so existing `.hexon` archives on disk keep parsing, AND there must be a **backfill/migration
  path** deriving sensible scale/GSD from existing bounds+zoom (Web-Mercator) for
  already-installed tilesets rather than silently defaulting to 1.0. Do **NOT** touch the
  parallel `fe-hexon/src/manifest.rs` `HexonManifest` — it is the wrong struct.
- **C6 — fe-ui must not depend on fe-terrain.** Clamping bounds reach the UI via the terrain
  JSON / API surface, never a direct `fe-terrain` dependency.

# Functional Requirements

### FR-1 — Additive scale metadata on `TilesetMeta`
Add optional, serde-defaulted scale fields to `TilesetMeta` (`fe-format/src/manifest.rs:30`):
`native_scale: Option<f64>`, `ground_sample_distance_m: Option<f64>`, `crs: Option<String>`
(default `EPSG:4326`), and scale bounds (`scale_bounds: Option<[f64;2]>`, i.e. [min,max]).
- **Acceptance:** old `.hexon` archives (no scale fields) deserialize without error; new
  fields round-trip through `export_tileset` (`fe-format/src/archive.rs:199`) and
  `HexonArchiveData.tileset_meta` (~:36). Serde default keeps `None`/EPSG:4326.
- **Priority:** P0

### FR-2 — Backfill / derivation of scale + GSD from bounds+zoom
A pure fn derives `ground_sample_distance_m` and `native_scale` from existing
`bounds` + `min/max_zoom` via standard Web-Mercator math (reuse the constant/logic behind
`tile_world_size_m`) when the fields are absent.
- **Acceptance:** given a real `TilesetMeta` with bounds + zoom but no scale fields, the
  derivation returns a real-meter GSD/native_scale (not 1.0); result matches
  `tile_world_size_m` at the tileset's center latitude within tolerance.
- **Priority:** P0

### FR-3 — Migration of already-installed tilesets on load
On load, `HexonStore` / `TilesetRegistry` (`fe-terrain/src/tiles/store.rs`,
`tiles/registry.rs:24/44`) upgrade tilesets missing scale fields by applying FR-2, so
installed hexons get real values.
- **Acceptance:** loading an installed tileset with no scale fields yields a
  `TilesetInfo`/`TilesetMeta` whose scale/GSD are backfilled (non-default); idempotent on
  re-load (already-populated fields untouched).
- **Priority:** P0

### FR-4 — Composite carries per-source meta + reconciles to a common metric frame
`CompositeTileSource` (`fe-terrain/src/tiles/composite.rs:17`) retains each
`HexonTileSource`'s `TilesetMeta` and exposes a reconciliation that (a) computes a common
real-meter frame across sources and (b) selects LOD/zoom per source from its GSD.
- **Acceptance:** a composite of two sources at differing GSD reports a single reconciled
  metric frame; per-source LOD selection differs according to GSD; selection no longer blindly
  first-hits `covers()` where GSD should decide resolution.
- **Priority:** P0

### FR-5 — Hexon-authoritative world_scale + exposed clamp bounds
`config_for_tileset` (`fe-terrain/src/petal_binding.rs:29`) and `apply_terrain_assignments`
(`:112`, override site `:159-173`) set `world_scale` from the hexon's `native_scale` and
expose `scale_bounds` so downstream can clamp the user nudge. User `world_scale` is clamped
into `scale_bounds` via `scale::sanitize_world_scale`-style logic.
- **Acceptance:** binding a tileset with `native_scale` sets `TerrainConfig.world_scale`
  (`config.rs:74`) to the derived value; a user value outside `scale_bounds` is clamped;
  `effective_world_scale()` (`config.rs:79`) respects the clamp.
- **Priority:** P0

### FR-6 — Clamp bounds surfaced to the UI without a fe-terrain dependency
`tileset_to_terrain_json` (`fe-ui/src/terrain_map/mod.rs:103`, emits `world_scale` :133)
carries `scale_bounds`; the scale slider/presets in
`fe-ui/src/dialogs/hexon_manager.rs:359-413` and `PetalMapState.world_scale` (:25) clamp to
those bounds; `UiAction::PetalSetMapScale` (`actions/mod.rs:60,331` →
`actions/hexon.rs:181`) rejects/clamps out-of-range values.
- **Acceptance:** the UI cannot set `world_scale` outside the hexon's `scale_bounds`; the
  renderer mirror `CameraScaleSettings.world_scale` (`fe-renderer/src/camera.rs:23`) receives
  only clamped values. No new fe-ui → fe-terrain dependency edge.
- **Priority:** P0

### FR-7 — Ruler pure-math module
New `fe-terrain/src/ruler.rs` (next to `scale.rs`) with Bevy-free functions: nice-number /
round-span selection, world↔real distance, bearing, polygon area (planar metric).
- **Acceptance:** unit tests for nice-number snapping (1/2/5×10ⁿ), distance, bearing
  (0-360°), and area of a known polygon; zero Bevy imports in the module.
- **Priority:** P0

### FR-8 — Scale bar HUD (`RulerPlugin`)
A `RulerPlugin` reads camera height via `world_to_real_height`, snaps to a nice number
(FR-7), and renders an egui overlay (bevy_egui 0.39). Reflects hexon-bounded scale.
- **Acceptance:** at a given camera height the scale bar shows a nice-number real distance
  consistent with the reconciled metric frame; updates as the camera zooms.
- **Priority:** P1

### FR-9 — Measurement tools (tape / area / bearing) + GPX path length
Interaction system placing metric anchors: tape (point-to-point distance), area (polygon),
bearing (heading). Reuse existing GPX paths (from `terrain_gpx_maps` / path editor) for a
path-length readout.
- **Acceptance:** placing two anchors reports real-meter distance and bearing; a closed
  polygon reports real-meter area; a loaded GPX path reports total path length.
- **Priority:** P1

### FR-10 — Adaptive world grid + dimensioned annotations
A 3D graticule snapping to round real distances (FR-7), re-subdividing with zoom and fading
by camera height; CAD-style dimension callouts integrated into the existing
annotation/property system.
- **Acceptance:** grid spacing snaps to round metric values and re-subdivides on zoom; grid
  fades out beyond a camera-height threshold; a dimension callout persists via the existing
  annotation/property store.
- **Priority:** P2

# Non-Functional Requirements

- **NFR-1 — Backward compatibility.** No existing `.hexon` archive fails to parse after
  FR-1. Backfill is idempotent (FR-3).
- **NFR-2 — Purity / testability.** All measurement/scale math (`scale.rs`, `ruler.rs`,
  reconciliation, backfill) is Bevy-free and unit-tested; UI layers stay thin. >80% coverage
  on new code.
- **NFR-3 — Quality gates (workflow.md).** `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `///` on all public fns, no `unwrap()`/`expect()` in production paths, TDD red→green per task,
  git note per task, checkpoint per phase.
- **NFR-4 — Layering.** fe-ui must not depend on fe-terrain (C6). Reconciliation math lives in
  fe-terrain, format fields in fe-format (C4).
- **NFR-5 — Performance.** Reconciliation and grid re-subdivision run per-frame-safe (no
  allocation storms); scale bar/grid updates do not stall the Bevy schedule.

# User Stories

**US-1 — GIS operator loads a real-world hexon.**
As an operator, I want a loaded terrain hexon to render at its true real-world size, so my
digital twin is metrically correct without me guessing a scale.
- Given a hexon with a declared native scale, When I bind it to a petal, Then the terrain
  renders at the hexon's authoritative scale and my slider only nudges within its bounds.

**US-2 — Operator combines mixed-resolution sources.**
As an operator, I want a composite of several sources at different resolutions to line up in
one real-meter frame, so tiles from different GSDs don't mismatch.
- Given two sources at differing GSD, When they compose, Then they share one metric frame and
  each contributes at the LOD its GSD supports.

**US-3 — Operator measures the world.**
As an operator, I want a scale bar, tape/area/bearing tools, and a metric grid, so I can read
real distances, areas, and headings directly in-world.
- Given a rendered petal, When I place two anchors, Then I see the real-meter distance and
  bearing; When I draw a polygon, Then I see its real-meter area.

**US-4 — Existing installs upgrade silently.**
As an existing user, I want my already-installed hexons to gain real scale values
automatically, so old data isn't stuck at scale 1.0.
- Given an installed tileset with no scale fields, When it loads, Then its GSD/native_scale are
  backfilled from bounds+zoom.

# Technical Considerations / Code Seams

- **Format:** `TilesetMeta` — `fe-format/src/manifest.rs:30`; round-trip via
  `export_tileset` (`fe-format/src/archive.rs:199`) and `HexonArchiveData.tileset_meta` (~:36);
  `HexonType::TerrainTileset` signals presence.
- **Real-meter math:** `tile_world_size_m(lat, zoom)` — `fe-terrain/src/terrain_plugin.rs:169`;
  `TileCoord` (`tiles/source.rs:13`, `to_lat_lon:48`, `from_lat_lon:35`).
- **Composite:** `CompositeTileSource` — `fe-terrain/src/tiles/composite.rs:17`;
  `HexonTileSource` (`tiles/hexon_source.rs:20`, holds `.tileset_meta`, `bounds()/zoom_range()/covers()`).
- **Config/scale:** `TerrainConfig` — `fe-terrain/src/config.rs:50` (`world_scale:74`,
  `effective_world_scale():79`); pure math in `fe-terrain/src/scale.rs`.
- **Binding seam:** `config_for_tileset` — `fe-terrain/src/petal_binding.rs:29`;
  `apply_terrain_assignments` (`:112`, override site `:159-173`); `TilesetInfo`/`TilesetRegistry`
  (`tiles/registry.rs:24/44`); `HexonStore` (`tiles/store.rs`).
- **UI:** `PetalMapState.world_scale` (`fe-ui/src/terrain_map/mod.rs:25`),
  `tileset_to_terrain_json` (`:103`, world_scale `:133`); slider/presets
  (`fe-ui/src/dialogs/hexon_manager.rs:359-413`); `UiAction::PetalSetMapScale`
  (`fe-ui/src/actions/mod.rs:60,331` → `actions/hexon.rs:181`); renderer mirror
  `CameraScaleSettings.world_scale` (`fe-renderer/src/camera.rs:23`).
- **Related tracks:** builds on `_archive/terrain_scale_controls_20260711`; coordinate flags
  with in-flight `terrain_lod_hardening_20260711` (same crates). GPX paths from
  `terrain_gpx_maps` / path-editor work are reusable for FR-9.
- **Docs convention:** rationale/"why" goes in directory-level `AGENTS.md` pointers, not
  verbose inline comments.

# Out of Scope / Non-Goals

- Photogrammetric / 3D Gaussian-splat scale ingestion.
- DataFusion / GeoParquet analytics (Phase 6.2 deferred elsewhere).
- CRS reprojection beyond WGS84 / Web-Mercator (the `crs` field is recorded, not reprojected).
- Any change to `fe-hexon/src/manifest.rs` `HexonManifest` (wrong struct — explicitly excluded).
- A new crate for the ruler math (lives in fe-terrain per C4).

# Open Questions

- OQ-1: Represent scale bounds as `scale_bounds: Option<[f64;2]>` vs separate
  `min_scale`/`max_scale` — spec assumes the array; confirm during Phase 1.
- OQ-2: Exact clamp semantics when the user value is outside bounds — clamp silently vs reject
  with feedback (FR-6). Default: clamp + surface the clamped value.
- OQ-3: Dimension-callout persistence shape in the existing annotation/property store (FR-10).
