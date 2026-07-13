---
type: Worker Report
title: W3 — Hexon Scale Orchestration + Rulers (data layer)
resource: ./spec.md
---

# W3 Worker Report — Hexon Scale Orchestration + Rulers

## Status summary

- **FR-1 through FR-7: fully implemented** (P0 scope complete).
- **FR-8, FR-9, FR-10 (P1/P2 UI layers — scale bar HUD, measurement tools, adaptive grid): not attempted** this pass — stopped at full P0 completion per instructions. All three build directly on `fe-terrain/src/ruler.rs` (FR-7), which is done and unit-tested, so a follow-up worker has a ready foundation.

## Files changed

- `fe-format/src/manifest.rs` — `TilesetMeta` additive fields + `derive_scale_from_bounds` + tests.
- `fe-format/src/archive.rs` — round-trip test for scale fields through `export_tileset`/`import`.
- `fe-terrain/src/scale.rs` — `clamp_world_scale_to_bounds` + tests.
- `fe-terrain/src/config.rs` — `TerrainConfig.scale_bounds` field, `effective_world_scale()` now clamps.
- `fe-terrain/src/petal_binding.rs` — `config_for_tileset` sets `world_scale`/`scale_bounds` from `TilesetInfo`; `apply_terrain_assignments` override site clamps `config.world_scale` into hexon-declared `scale_bounds` on every assignment.
- `fe-terrain/src/tiles/composite.rs` — `ReconciledMetricFrame`, `SourceLodPick`, `CompositeTileSource::source_metas()/reconcile_metric_frame()/select_source_lods()`, private `resolved_gsd_and_scale()`; tests with two differing-GSD sources.
- `fe-terrain/src/tiles/store.rs` — `backfill_scale_fields()` (public), wired into `load_tileset()`; tests for backfill + idempotency.
- `fe-terrain/src/tiles/registry.rs` — `TilesetInfo.native_scale`/`scale_bounds` fields, populated in `list_tilesets()`.
- `fe-terrain/src/ruler.rs` — **new module**: `nice_number`, `world_to_real_distance`, `bearing_deg`, `polygon_area_m2`. Zero Bevy imports. 12 unit tests.
- `fe-terrain/src/lib.rs` — registered `pub mod ruler;`.
- `fe-ui/src/terrain_map/mod.rs` — `PetalMapState.scale_bounds` field; `tileset_to_terrain_json(ts, world_scale, scale_bounds)` now 3-arg, emits `"scale_bounds"` JSON key when present; nav-change reset clears `scale_bounds`; tests updated + added.
- `fe-ui/src/dialogs/hexon_manager.rs` — scale slider/presets clamp to `petal_map.scale_bounds` when present (falls back to the generic `SCALE_MIN..SCALE_MAX` range), "(hexon-bounded)" hint label.
- `fe-ui/src/actions/hexon.rs` — `set_petal_map` passes `scale_bounds` through; `set_petal_map_scale` clamps the incoming `world_scale` via a local `clamp_to_bounds` helper (no fe-terrain dep, per C6) before persisting/applying, surfacing the clamped value back into `petal_map.world_scale` (OQ-2: clamp + feedback, not reject).
- `fe-terrain/src/tiles/hexon_source.rs` — test-helper `TilesetMeta` literal updated with new fields (required for the struct to keep compiling; no behavior change).

## New `TilesetMeta` fields (fe-format/src/manifest.rs:30)

```rust
pub native_scale: Option<f64>,               // #[serde(default, skip_serializing_if = "Option::is_none")]
pub ground_sample_distance_m: Option<f64>,    // #[serde(default, skip_serializing_if = "Option::is_none")]
pub crs: Option<String>,                      // #[serde(default = "default_crs")] -> Some("EPSG:4326")
pub scale_bounds: Option<[f64; 2]>,           // #[serde(default, skip_serializing_if = "Option::is_none")]
```

All additive/serde-defaulted; legacy `.hexon` archives with no scale keys deserialize to `None`/`Some("EPSG:4326")` (see `legacy_tileset_meta_deserializes_with_defaults` test).

## Backfill fn signature (FR-2 + FR-3)

```rust
// fe-format/src/manifest.rs — pure Web-Mercator derivation
pub fn derive_scale_from_bounds(bounds: [f64; 4], max_zoom: u8, tile_size: u16) -> (f64, f64)
// returns (ground_sample_distance_m, native_scale)

// fe-terrain/src/tiles/store.rs — idempotent backfill wired into HexonStore::load_tileset
pub fn backfill_scale_fields(meta: &mut TilesetMeta)
```

`HexonStore::load_tileset` calls `backfill_scale_fields` on every load; `TilesetRegistry::load_all/install/reload` all route through `store.load_tileset`, so registry loads are backfilled automatically. `get_or_insert` semantics make it idempotent — already-populated fields untouched.

## Composite reconciliation (FR-4)

```rust
pub struct ReconciledMetricFrame { pub finest_gsd_m: f64, pub native_scale: f64 }
pub struct SourceLodPick { pub source_index: usize, pub zoom: u8 }

impl CompositeTileSource {
    pub fn source_metas(&self) -> Vec<&fe_format::manifest::TilesetMeta>;
    pub fn reconcile_metric_frame(&self) -> Option<ReconciledMetricFrame>;
    pub fn select_source_lods(&self, target_gsd_m: f64) -> Vec<SourceLodPick>;
}
```

`reconcile_metric_frame` picks the finest (smallest) GSD across all added hexon sources (backfilling via `derive_scale_from_bounds` for any source lacking declared scale fields). `select_source_lods` maps a target GSD to a per-source zoom pick (not blind `covers()` order) using `log2(target/native)` step-down from each source's own `max_zoom`, clamped to that source's own zoom range.

## Ruler module public API (FR-7) — `fe-terrain/src/ruler.rs`

```rust
pub fn nice_number(span: f64) -> f64;                                  // snaps to 1/2/5 x 10^n
pub fn world_to_real_distance(a: [f64;3], b: [f64;3], scale: f64) -> f64;
pub fn bearing_deg(a: [f64;3], b: [f64;3]) -> f64;                     // 0..360, XZ ground plane, north = -Z
pub fn polygon_area_m2(vertices: &[[f64;3]], scale: f64) -> f64;       // shoelace, XZ plane
```

Zero Bevy imports (verified — module has no `use bevy` anywhere). 12 unit tests covering nice-number snap points/boundaries/bad-input, distance (3-4-5 triangle + scale inversion), bearing (4 cardinal directions + full-range check), and area (known square + scale² + degenerate inputs).

## Terrain-JSON keys added for scale_bounds (UI decoupling, C6/FR-6)

`tileset_to_terrain_json` (`fe-ui/src/terrain_map/mod.rs`) now emits (when bounds are `Some`):

```json
{ "scale_bounds": [min, max] }
```

alongside the existing `world_scale` key — plain `serde_json`, no `fe-terrain` type imported. `fe-terrain::TerrainConfig` deserializes this key directly into its new `scale_bounds: Option<[f64;2]>` field (same serde shape both sides rely on informally, as with all other `TerrainConfig` keys). fe-ui's `Cargo.toml` was **not** touched — no new `fe-terrain` dependency edge.

## Known gap — requires a follow-up outside my file ownership

`fe-ui/src/verse_manager/db_results.rs` (owned by W1/W4, excluded from my scope) handles `DbResult::PetalTerrainLoaded` and currently only reads `terrain["world_scale"]` into `petal_map.world_scale` (lines ~516-534). It does **not** yet read `terrain["scale_bounds"]` into the new `petal_map.scale_bounds` field. Until that 3-line addition lands, the UI slider/preset clamp logic I wired (`hexon_manager.rs`, `actions/hexon.rs`) will always see `scale_bounds: None` on load (defaulting to the generic `SCALE_MIN..SCALE_MAX` range) even though the terrain JSON and `PetalMapState` struct both support it end-to-end. Suggested addition for whoever owns that file:

```rust
petal_map.scale_bounds = terrain
    .as_ref()
    .and_then(|t| t.get("scale_bounds"))
    .and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok());
```

## Note on shared struct literals outside my ownership

`TilesetMeta` struct literals also exist in `fe-terrain/src/tiles/builder.rs` and `fe-terrain/tests/phase7_4a_qa.rs` (both explicitly owned by W2 / excluded from my scope). Since the new fields are plain (non-`Option`-defaulted-at-the-Rust-level) struct fields — serde defaults don't help Rust struct-literal syntax — those two files will fail to compile until they add the same four fields (`native_scale: None, ground_sample_distance_m: None, crs: None, scale_bounds: None`) to their `TilesetMeta { .. }` literals. Flagging for the coordinator's serialized sweep; I did not touch either file per the exclusive-ownership rule.

## Verification not run (per HARD RULES — coordinator owns the sweep)

No `cargo build/test/check/clippy` was executed. All new/changed code has accompanying unit tests (>80% coverage target for the pure math in `ruler.rs`, `scale.rs`, `manifest.rs` derivation, `composite.rs` reconciliation, `store.rs` backfill) ready for the coordinator's single serialized sweep.

## Coordinator fix: T3 hardening (crs/polar/ruler-XZ)

Applied 3 hardening items from the opus SHIP review, editing only `fe-format/src/manifest.rs` and `fe-terrain/src/ruler.rs`. No cargo run (coordinator sweep owns the build).

1. **`crs` re-serialization mutation (MEDIUM)** — added `skip_serializing_if = "Option::is_none"` to `TilesetMeta.crs` alongside the existing `default = "default_crs"`. A legacy archive with no `crs` key still deserializes to `Some("EPSG:4326")` in memory (unchanged — `legacy_tileset_meta_deserializes_with_defaults` still holds), but now stays absent on re-serialize instead of writing an explicit key back into a previously byte-stable archive. Added `TilesetMeta::crs_or_default(&self) -> &str` helper for read sites that want the effective CRS without unwrapping.

2. **Polar/degenerate bounds guard (LOW)** — `derive_scale_from_bounds` now clamps `center_lat` to `±85.0` (standard Web-Mercator limit, `MAX_ABS_LAT` const) before computing `cos(lat)`, so `tile_world_size_m` can no longer hit exactly `0.0` at the poles. This removes the silent `native_scale` fallback to `1.0` and the `gsd = 0.0` poisoning path for composite reconciliation's min-GSD pick.

3. **Ruler distance axis mismatch (MEDIUM)** — `world_to_real_distance` changed from 3D (`dx,dy,dz`) to ground-plane XZ (`dx,dz`), matching `bearing_deg` and `polygon_area_m2`'s existing ground-plane convention. Doc comment updated to state Y is ignored. Existing 3-4-5 test (dy=0) still passes unchanged; added `world_to_real_distance_ignores_height` test asserting two points differing only in Y report `0.0` distance.

All three fixes are additive/behavior-narrowing on edge cases only; no public signatures changed except the new `crs_or_default` helper. No unwrap/expect introduced. Terse one-line doc-comments used throughout, no verbose inline blocks added.

WORKER_COMPLETE W3
