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
- [x] Task: Add serde-defaulted scale fields to `TilesetMeta` (`fe-format/src/manifest.rs`):
  `native_scale`, `ground_sample_distance_m`, `crs` (default EPSG:4326),
  `scale_bounds: Option<[f64;2]>`. (TDD: write `manifest.rs` test deserializing a legacy
  JSON blob with no scale fields and asserting defaults; then add fields.)
- [x] Task: Round-trip scale fields through archive export/import
  (`fe-format/src/archive.rs` `export_tileset` ~:199, `HexonArchiveData.tileset_meta` ~:36).
  (TDD: write archive round-trip test asserting scale fields survive export→import; then wire.)
- [x] Task: Pure `derive_scale_from_bounds(bounds, min_zoom, max_zoom)` deriving GSD +
  native_scale via Web-Mercator (mirror `tile_world_size_m` constant `40_075_016.686`).
  Place in `fe-format` (or a small pure helper) so it has no Bevy dep. (TDD: write test with a
  real bounds+zoom asserting derived GSD ≈ `tile_world_size_m` at center latitude; then implement.)
- [x] Verification: legacy `.hexon` fixture parses; new fields round-trip; derivation matches
  Web-Mercator within tolerance. `cargo test -p fe-format` green, fmt+clippy clean. [checkpoint marker]

## Phase 2: Data-layer reconciliation + authoritative binding (FR-3, FR-4, FR-5)
Goal: installed tilesets get backfilled on load, composites reconcile per-source GSD into one
metric frame, and the hexon becomes authoritative for world_scale with exposed clamp bounds.
(Coordinate with `terrain_lod_hardening_20260711` before editing `composite.rs`.)

Tasks:
- [x] Task: Backfill on load in `HexonStore`/`TilesetRegistry` (`fe-terrain/src/tiles/store.rs`,
  `tiles/registry.rs:24/44`) — apply Phase-1 derivation when scale fields absent; idempotent.
  (TDD: write registry test loading a no-scale `TilesetInfo` and asserting backfilled non-default
  GSD, plus re-load leaves populated fields untouched; then implement.)
- [x] Task: `CompositeTileSource` (`fe-terrain/src/tiles/composite.rs:17`) retains each
  `HexonTileSource.tileset_meta` and adds `reconcile_metric_frame()` computing a common
  real-meter frame across sources. (TDD: write composite test with two sources at differing GSD
  asserting a single reconciled frame; then implement.)
- [x] Task: Per-source GSD → LOD/zoom selection in composite (replace blind first-hit `covers()`
  where GSD should decide). (TDD: write test asserting the two sources select different LODs per
  their GSD; then implement.)
- [x] Task: Hexon-authoritative world_scale in `config_for_tileset`
  (`fe-terrain/src/petal_binding.rs:29`) + `apply_terrain_assignments` override site (`:159-173`):
  set `TerrainConfig.world_scale` from `native_scale`, expose `scale_bounds`, clamp user nudge via
  `scale::sanitize_world_scale`-style logic (`config.rs:74/79`). (TDD: write petal_binding test —
  bind tileset with native_scale sets world_scale; user value outside bounds is clamped; then implement.)
- [x] Verification: mixed-GSD compose reconciles; binding clamps user nudge to hexon bounds;
  installed-tileset backfill non-default. `cargo test -p fe-terrain` green, fmt+clippy clean.
  [checkpoint marker]

## Phase 3: Ruler pure math (FR-7)
Goal: a Bevy-free `fe-terrain/src/ruler.rs` with all measurement/snap math, fully unit-tested.

Tasks:
- [x] Task: `nice_number(span)` — snap to 1/2/5×10ⁿ round spans. (TDD: write `ruler.rs` test
  covering several spans and boundary cases; then implement.)
- [x] Task: `world_to_real_distance` + `bearing_deg` (0-360). (TDD: write tests for known
  distance and known bearing; then implement.)
- [x] Task: `polygon_area_m2` (planar metric area). (TDD: write test for a known-area polygon;
  then implement.)
- [x] Verification: full `ruler.rs` unit suite green; module has zero Bevy imports; fmt+clippy
  clean; add `AGENTS.md` pointer for rationale. [checkpoint marker]

## Phase 4: Scale bar HUD (FR-8)
Goal: `RulerPlugin` renders a nice-number scale bar reflecting the hexon-bounded metric frame.

Tasks:
- [x] Task: `RulerPlugin` scaffold + system reading camera height via `world_to_real_height`
  (`fe-terrain/src/scale.rs`) and snapping via `ruler::nice_number`. (TDD: write test on the
  pure height→scale-bar-length helper asserting nice-number output at sample heights; then wire
  plugin.)
- [x] Task: egui overlay (bevy_egui 0.39) drawing the scale bar; label from reconciled metric
  frame. (TDD: unit-test the label/format helper; egui draw stays thin.)
- [x] Verification: scale bar shows nice-number real distance consistent with the reconciled
  frame and updates on zoom. `cargo test -p fe-terrain` green, fmt+clippy clean. [checkpoint marker]

## Phase 5: Measurement tools — tape / area / bearing + GPX path length (FR-9)
Goal: interactive metric anchors giving distance/area/bearing, plus GPX path-length readout.

Tasks:
- [x] Task: metric anchor placement + tape (point-to-point) using `ruler::world_to_real_distance` *(DONE 2026-10-11, commit pending — `Tool::Measure` (key M) + pure reducer `fe-ui/src/node_manager/measure.rs::{reduce, readout}` over the `geometry.rs` mirror; viewport `handle_measure_interaction` claims `ClickPriority::Measure` right after Brush; Options Measure arm shows copyable Distance/Bearing rows. Tests `reducer_table`, `tape_reports_real_distance_and_compass_bearing_at_both_scales` (world_scale 1 and 0.01, bearings 0/90/180/270), `viewport_clicks_place_tape_anchors_and_claim_the_press`. See `fe-ui/src/node_manager/AGENTS.md` §measure-tool.)*
  + `bearing_deg`. (TDD: write test on the anchor→(distance,bearing) reducer; then wire interaction.)
- [x] Task: area tool over a closed polygon of anchors via `ruler::polygon_area_m2`. (TDD: write *(DONE 2026-10-11, commit pending — Measure tool Area mode: append anchors, Close polygon (needs 3+), Perimeter + Area rows. Test `closed_square_reports_area_and_perimeter_at_both_scales`: 100 m square = 10 000 m² at world_scale 1 and 0.01.)*
  test asserting real-meter area for a placed polygon; then wire.)
- [x] Task: GPX path-length readout reusing existing GPX paths (from `terrain_gpx_maps` / *(AUDIT 2026-10-10: landed via path_interaction_20260716 FR-3 @906d9cc — `fe-ui/src/node_manager/path_segment_interaction.rs:289 sync_path_measurements` keeps `PathEditorState.total_length_m` / `selected_segment_length_m` (real meters via world_scale) live for the Paths tab; tests `ground_len_ignores_height`, `ground_len_scaled_to_real_meters`; API twin `GET /api/v1/nodes/{track_id}/stats`.)*
  path-editor). (TDD: write test summing segment distances over a GPX fixture; then wire.)
- [x] Verification: two anchors report distance+bearing; polygon reports area; GPX path reports
  total length. `cargo test -p fe-terrain` green, fmt+clippy clean. [checkpoint marker] *(2026-10-11: `cargo check --tests` + `clippy --all-targets -D warnings` clean on fe-ui/fe-terrain/fractalengine; test execution is in the orchestrator's single end sweep. In-app tape/area at a real petal scale is USER_GATED.)*

## Phase 6: Adaptive world grid + dimensioned annotations (FR-10)
Goal: a metric graticule that snaps to round distances and re-subdivides/fades with zoom, plus
CAD-style dimension callouts wired into the existing annotation/property system.

Tasks:
- [x] Task: grid spacing selection via `ruler::nice_number` + re-subdivision by camera height. *(DONE 2026-10-11, commit pending — `fe-terrain/src/ruler.rs::grid_spacing_for_height` (major = nice_number(2h / target), minor = next ladder step down, `minor_alpha` fades the minors before each re-subdivision) + `ruler_plugin.rs::draw_world_grid` (y = 0 plane around the orbit focus, at most 2·64 lines per tier, needs an enabled map). Opt-in via the Settings "Metric grid" checkbox (`AppSettings.show_world_grid`, default off), mirrored by `fractalengine/src/terrain_bridge.rs::sync_ruler_settings`. Tests `grid_spacing_majors_sit_on_the_ladder_at_site_to_region_heights`, `grid_spacing_is_monotone_in_height_and_minor_fades_before_resubdivision`, `settings_toggles_reach_the_ruler_overlays`.)*
  (TDD: write test on the height→spacing/subdivision helper; then implement grid system.)
- [x] Task: grid fade by camera-height threshold. (TDD: write test on the fade-alpha helper; then *(DONE 2026-10-11, commit pending — `ruler::grid_fade_alpha`, wired at 50 km to 200 km camera height; test `grid_fade_alpha_table`.)*
  wire rendering.)
- [ ] Task: CAD-style dimension callouts persisted via the existing annotation/property store *(AUDIT 2026-10-10: OPEN + BLOCKED on ratification of outstanding_decisions D-11 (callout persistence shape, [USER]).)*
  (resolve OQ-3 shape first). (TDD: write test round-tripping a dimension annotation through the
  property store; then wire.)
- [ ] Verification: grid snaps to round metric spacing, re-subdivides on zoom, fades beyond
  threshold; a dimension callout persists. Full `cargo test` sweep green, fmt+clippy clean.
  [checkpoint marker]
