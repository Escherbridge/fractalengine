---
type: Implementation Plan
title: Hexon Scale Orchestration + Rulers
tags: [hexon_scale_orchestration_20260712]
resource: ./spec.md
---

# Implementation Plan: Hexon Scale Orchestration + Rulers

## Overview

Data-layer-first, six phases. Scale must be present in the format and reconciled through the
data layer before any UI renders it, so Phases 1-2 land the metric substrate, Phase 3 lands
the pure ruler math, and Phases 4-6 build the three UI layers on top.

Each task is one TDD cycle (Red → Green → Refactor): write the named failing test first,
implement the minimum to pass, refactor. Each phase ends with a `[checkpoint marker]`
verification per [workflow.md](../../workflow.md). Quality gates every task: `cargo fmt`,
`cargo clippy -- -D warnings`, `///` on public fns, no `unwrap`/`expect` in prod paths, git
note per task. See [spec.md](./spec.md) for requirements and code seams.

**Dependencies:** builds on `terrain_scale_controls_20260711` (done — per-petal world_scale).
**Coordination:** `terrain_lod_hardening_20260711` is in-flight in the same crates
(fe-terrain composite/LOD, config) — coordinate feature flags and rebase order before touching
`composite.rs` in Phase 2.

## Phase 1: Format — additive scale metadata + backfill (FR-1, FR-2)
Goal: `TilesetMeta` carries scale/GSD/CRS/bounds fields that round-trip and default cleanly,
plus a pure derivation from bounds+zoom. Existing `.hexon` archives keep parsing.

Tasks:
- [ ] Task: Add serde-defaulted scale fields to `TilesetMeta` (`fe-format/src/manifest.rs`):
  `native_scale`, `ground_sample_distance_m`, `crs` (default EPSG:4326),
  `scale_bounds: Option<[f64;2]>`. (TDD: write `manifest.rs` test deserializing a legacy
  JSON blob with no scale fields and asserting defaults; then add fields.)
- [ ] Task: Round-trip scale fields through archive export/import
  (`fe-format/src/archive.rs` `export_tileset` ~:199, `HexonArchiveData.tileset_meta` ~:36).
  (TDD: write archive round-trip test asserting scale fields survive export→import; then wire.)
- [ ] Task: Pure `derive_scale_from_bounds(bounds, min_zoom, max_zoom)` deriving GSD +
  native_scale via Web-Mercator (mirror `tile_world_size_m` constant `40_075_016.686`).
  Place in `fe-format` (or a small pure helper) so it has no Bevy dep. (TDD: write test with a
  real bounds+zoom asserting derived GSD ≈ `tile_world_size_m` at center latitude; then implement.)
- [ ] Verification: legacy `.hexon` fixture parses; new fields round-trip; derivation matches
  Web-Mercator within tolerance. `cargo test -p fe-format` green, fmt+clippy clean. [checkpoint marker]

## Phase 2: Data-layer reconciliation + authoritative binding (FR-3, FR-4, FR-5)
Goal: installed tilesets get backfilled on load, composites reconcile per-source GSD into one
metric frame, and the hexon becomes authoritative for world_scale with exposed clamp bounds.
(Coordinate with `terrain_lod_hardening_20260711` before editing `composite.rs`.)

Tasks:
- [ ] Task: Backfill on load in `HexonStore`/`TilesetRegistry` (`fe-terrain/src/tiles/store.rs`,
  `tiles/registry.rs:24/44`) — apply Phase-1 derivation when scale fields absent; idempotent.
  (TDD: write registry test loading a no-scale `TilesetInfo` and asserting backfilled non-default
  GSD, plus re-load leaves populated fields untouched; then implement.)
- [ ] Task: `CompositeTileSource` (`fe-terrain/src/tiles/composite.rs:17`) retains each
  `HexonTileSource.tileset_meta` and adds `reconcile_metric_frame()` computing a common
  real-meter frame across sources. (TDD: write composite test with two sources at differing GSD
  asserting a single reconciled frame; then implement.)
- [ ] Task: Per-source GSD → LOD/zoom selection in composite (replace blind first-hit `covers()`
  where GSD should decide). (TDD: write test asserting the two sources select different LODs per
  their GSD; then implement.)
- [ ] Task: Hexon-authoritative world_scale in `config_for_tileset`
  (`fe-terrain/src/petal_binding.rs:29`) + `apply_terrain_assignments` override site (`:159-173`):
  set `TerrainConfig.world_scale` from `native_scale`, expose `scale_bounds`, clamp user nudge via
  `scale::sanitize_world_scale`-style logic (`config.rs:74/79`). (TDD: write petal_binding test —
  bind tileset with native_scale sets world_scale; user value outside bounds is clamped; then implement.)
- [ ] Verification: mixed-GSD compose reconciles; binding clamps user nudge to hexon bounds;
  installed-tileset backfill non-default. `cargo test -p fe-terrain` green, fmt+clippy clean.
  [checkpoint marker]

## Phase 3: Ruler pure math (FR-7)
Goal: a Bevy-free `fe-terrain/src/ruler.rs` with all measurement/snap math, fully unit-tested.

Tasks:
- [ ] Task: `nice_number(span)` — snap to 1/2/5×10ⁿ round spans. (TDD: write `ruler.rs` test
  covering several spans and boundary cases; then implement.)
- [ ] Task: `world_to_real_distance` + `bearing_deg` (0-360). (TDD: write tests for known
  distance and known bearing; then implement.)
- [ ] Task: `polygon_area_m2` (planar metric area). (TDD: write test for a known-area polygon;
  then implement.)
- [ ] Verification: full `ruler.rs` unit suite green; module has zero Bevy imports; fmt+clippy
  clean; add `AGENTS.md` pointer for rationale. [checkpoint marker]

## Phase 4: Scale bar HUD (FR-8)
Goal: `RulerPlugin` renders a nice-number scale bar reflecting the hexon-bounded metric frame.

Tasks:
- [ ] Task: `RulerPlugin` scaffold + system reading camera height via `world_to_real_height`
  (`fe-terrain/src/scale.rs`) and snapping via `ruler::nice_number`. (TDD: write test on the
  pure height→scale-bar-length helper asserting nice-number output at sample heights; then wire
  plugin.)
- [ ] Task: egui overlay (bevy_egui 0.39) drawing the scale bar; label from reconciled metric
  frame. (TDD: unit-test the label/format helper; egui draw stays thin.)
- [ ] Verification: scale bar shows nice-number real distance consistent with the reconciled
  frame and updates on zoom. `cargo test -p fe-terrain` green, fmt+clippy clean. [checkpoint marker]

## Phase 5: Measurement tools — tape / area / bearing + GPX path length (FR-9)
Goal: interactive metric anchors giving distance/area/bearing, plus GPX path-length readout.

Tasks:
- [ ] Task: metric anchor placement + tape (point-to-point) using `ruler::world_to_real_distance`
  + `bearing_deg`. (TDD: write test on the anchor→(distance,bearing) reducer; then wire interaction.)
- [ ] Task: area tool over a closed polygon of anchors via `ruler::polygon_area_m2`. (TDD: write
  test asserting real-meter area for a placed polygon; then wire.)
- [ ] Task: GPX path-length readout reusing existing GPX paths (from `terrain_gpx_maps` /
  path-editor). (TDD: write test summing segment distances over a GPX fixture; then wire.)
- [ ] Verification: two anchors report distance+bearing; polygon reports area; GPX path reports
  total length. `cargo test -p fe-terrain` green, fmt+clippy clean. [checkpoint marker]

## Phase 6: Adaptive world grid + dimensioned annotations (FR-10)
Goal: a metric graticule that snaps to round distances and re-subdivides/fades with zoom, plus
CAD-style dimension callouts wired into the existing annotation/property system.

Tasks:
- [ ] Task: grid spacing selection via `ruler::nice_number` + re-subdivision by camera height.
  (TDD: write test on the height→spacing/subdivision helper; then implement grid system.)
- [ ] Task: grid fade by camera-height threshold. (TDD: write test on the fade-alpha helper; then
  wire rendering.)
- [ ] Task: CAD-style dimension callouts persisted via the existing annotation/property store
  (resolve OQ-3 shape first). (TDD: write test round-tripping a dimension annotation through the
  property store; then wire.)
- [ ] Verification: grid snaps to round metric spacing, re-subdivides on zoom, fades beyond
  threshold; a dimension callout persists. Full `cargo test` sweep green, fmt+clippy clean.
  [checkpoint marker]
