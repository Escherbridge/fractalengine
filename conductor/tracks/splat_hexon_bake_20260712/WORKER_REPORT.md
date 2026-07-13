---
type: Worker Report
title: Splat Hexon Bake — W2 Report
tags: [worker-report, splat_hexon_bake_20260712]
timestamp: 2026-07-12T00:00:00Z
---

# W2 Worker Report — splat_hexon_bake_20260712

## Files changed

- `fe-terrain/src/splat/bake.rs` (new) — FR-1. Bake-time coverage-fill,
  ported from `archive/splat-coverage-experiment-20260712:fe-terrain/src/splat/interpolate.rs`
  (commits `9839094`, `4be8ec4`). Pure/bevy-free, unit-tested (all archived
  tests ported + 2 new: `bakes_large_field_without_hanging`,
  `spatial_grid_finds_same_neighbors_as_brute_force`). Public entry points:
  `bake_splat_coverage(&SplatBuffer) -> Option<SplatBuffer>` and
  `bake_splat_coverage_within(&SplatBuffer, TileFootprint) -> Option<SplatBuffer>`.
  **Perf fix**: replaced the archived O(n²) all-pairs neighbor scan
  (`nearest_neighbors`) and O(n²) cluster-proximity guard with a uniform
  `SpatialGrid` (3x3 cell-block queries) — `nearest_neighbors_grid` and the
  cluster check in `push_midpoint` are now O(1) amortized per query, O(n)
  total per pass. `median_neighbor_spacing` also grid-accelerated (was
  O(n²)). Dropped the ceiling-gated (`splat_needs_interpolation` /
  zoom-based) code path entirely — this track only needs the
  coverage-driven fill, not the LOD-zoom variant (out of scope per spec).
- `fe-terrain/src/splat/format.rs` (new) — FR-2. `BakedSplatBuffer` serde
  type (`{version, positions, colors, scales, normals}`, versioned via
  `BAKED_SPLAT_FORMAT_VERSION = 1`), `encode_baked_splats`/`decode_baked_splats`
  (bincode), and hexon-archive embedding:
  `append_baked_splats_to_archive(archive_bytes, &[(cache_key, encoded_bytes)]) -> Result<Vec<u8>>`
  and `read_baked_splats_from_archive(archive_bytes) -> Result<Vec<(cache_key, BakedSplatBuffer)>>`.
  These re-open the `.hexon` ZIP (built by `fe_format::HexonArchive::export_tileset`,
  which I did NOT modify — out of scope/W3's `fe-format`) and append/read
  `terrain/splats/{cache_key}.bin` entries directly via the `zip` crate.
  Round-trip tested (encode/decode, archive append/read, empty-entries
  no-op, malformed/absent-section → empty not error).
- `fe-terrain/src/splat/mod.rs` — added `pub mod bake;` / `pub mod format;`
  + re-exports (`bake_splat_coverage`, `TileFootprint`, `BakedSplatBuffer`).
- `fe-terrain/src/tiles/builder.rs` — FR-3. New `TilesetBuilder::bake_splat_tiles`
  (decode elevation PNG → `synthesize_splats` at native density
  (`BAKE_SPLAT_STRIDE = 1`) → `bake_splat_coverage` → `encode_baked_splats`),
  called from both `package_hexon` and `package_chunked` after
  `HexonArchive::export_tileset`, piping the result through
  `append_baked_splats_to_archive`. Best-effort: unsupported encodings
  (`Raw16`) or per-tile decode/synth failures are skipped, never fail the
  whole build (tile just has no baked entry → FR-5 fallback). Added a small
  `parse_cache_key` helper (inverse of `TileCoord::cache_key`).
- `fe-terrain/src/tiles/hexon_source.rs` — FR-4 read side.
  `HexonTileSource` gained `baked_splats: HashMap<String, BakedSplatBuffer>`,
  populated in `from_archive` via `read_baked_splats_from_archive` (always
  `Ok`/degrades to empty, never fails the hexon load — FR-5). New getter
  `get_baked_splats(coord) -> Option<&BakedSplatBuffer>`. `from_directory`
  (dev-only pre-packaging path) leaves the map empty — no baked-splats
  convention there yet. Tests added: `loads_baked_splats_when_present`,
  `no_baked_splats_falls_back_to_empty_map`.
- `fe-terrain/src/splat/render.rs` — FR-4 runtime consume + FR-5 fallback.
  `build_tile_splat_mesh` now tries the baked buffer first (via
  `find_baked_splats`, see **SEAM** below) and bakes the mesh directly with
  `bake_splat_mesh` — **no `augment_splat_buffer_coverage` call anywhere in
  this file or in `reconcile_splat_chunks`'s hot path**, confirmed by
  inspection (that function doesn't exist in this tree; it was never
  reintroduced). Falls through to the pre-existing live `synthesize_splats`
  path, byte-for-byte unchanged, when no baked data is found.
- `fe-hexon/src/package.rs` — FR-2, per spec's explicit instruction to add
  the hexon-format field there. Added `BakedSplatTileRecord` (serde struct
  mirroring `BakedSplatBuffer` + a `cache_key` field) and
  `BAKED_SPLAT_TILE_RECORD_VERSION`. **Not wired through `HexonPackage::build/open`**
  — that struct builds a different archive shape (generic `.fecrate` model/
  material/skybox ZIPs, not the `TerrainTileset` ZIP `fe_format::HexonArchive`
  produces). `fe-hexon` has no dependency on `fe-terrain` or `fe-format`
  (non-dev), so this type is a standalone canonical serde description for
  hexon-format consumers outside `fe-terrain`, documented to point at the
  real on-disk owner (`fe-terrain/src/splat/format.rs`). Round-trip tested
  (`test_baked_splat_tile_record_json_roundtrip`,
  `test_baked_splat_tile_record_defaults_empty`).
- `fe-terrain/Cargo.toml` — added `zip = { version = "2", default-features = false, features = ["deflate"] }`
  and `bincode = "1"` (unconditional deps — needed by `hexon_source.rs`'s
  read path outside the `fetch` feature, not just `builder.rs`'s write path).

## New hexon format field / storage

- **Where it lives**: NOT a `TilesetMeta` manifest field. It's a ZIP-section
  convention: `terrain/splats/{cache_key}.bin` entries appended onto the
  existing `.hexon` archive, alongside `terrain/tiles/{cache_key}.png` and
  `terrain/satellite/{cache_key}.jpg`. `cache_key` = `{zoom}/{x}/{y}`
  (matches `TileCoord::cache_key()`).
- **Encoding**: bincode of `BakedSplatBuffer { version: u16, positions: Vec<[f32;3]>, colors: Vec<[f32;4]>, scales: Vec<[f32;2]>, normals: Vec<[f32;3]> }`.
  `version` is currently always `1` (`BAKED_SPLAT_FORMAT_VERSION`).
- **Optional/versioned**: absence of the whole `terrain/splats/` section, or
  of an individual tile's entry, or a decode failure on one entry, are all
  non-fatal — `read_baked_splats_from_archive` returns an empty/partial vec,
  never an `Err` that would sink the archive load. This is the FR-5
  contract.
- **Why not a `fe-format`/`TilesetMeta` change**: my file ownership excludes
  `fe-format/*` (W3 owns it, and `composite.rs`/`store.rs`/`registry.rs`).
  Embedding as an additional ZIP section that `fe-terrain` writes/reads
  directly (via the `zip` crate, now a direct `fe-terrain` dependency)
  avoids any edit to `fe-format::HexonArchive`/`TilesetMeta` while still
  round-tripping through the same `.hexon` bytes.

## Where the bake runs

`TilesetBuilder::bake_splat_tiles` (`fe-terrain/src/tiles/builder.rs`),
called from `package_hexon` and `package_chunked`, right after
`HexonArchive::export_tileset` produces the base archive bytes. Per tile:
decode elevation PNG → decoder selected from `TilesetMeta`'s
`ElevationEncoding` (`TerrainRgb`/`Terrarium`; `Raw16` is skipped, not
wired) → `synthesize_splats` at stride 1 (denser starting field than the
runtime default, cheap since this is offline/unconstrained) →
`bake_splat_coverage` (FR-1 fill) → `encode_baked_splats` → appended via
`append_baked_splats_to_archive`. Sequential per tile, no batching/threading
added (spec says build-time budget is fine as-is; parallelism can be a
follow-up if build times matter).

## How runtime chooses baked vs fallback

`HexonTileSource::from_archive` eagerly loads every `terrain/splats/*.bin`
entry into `baked_splats: HashMap<cache_key, BakedSplatBuffer>` at hexon-load
time (not per-frame). `render.rs::build_tile_splat_mesh` looks up the coord's
baked buffer first; if present and non-empty, converts to `SplatBuffer` and
bakes the mesh directly (zero fill cost, zero synth cost). If absent (or the
lookup path isn't wired yet — see SEAM below), it falls through to the
pre-existing live `synthesize_splats` call, completely unchanged from before
this track.

## SEAM — needs one more line, not currently reachable end-to-end

`CompositeTileSource` (`fe-terrain/src/tiles/composite.rs`, **W3's file, out
of my ownership**) keeps its `hexon_sources: Vec<HexonTileSource>` private
and exposes no `get_baked_splats_sync(coord)` passthrough — only
`get_tile_sync`/`get_satellite_tile_sync` exist for the elevation/satellite
PNG paths. Because of that, `render.rs::find_baked_splats` (the single named
call site standing in for this lookup) is currently a documented no-op
(`always returns None`) and every tile still takes the FR-5 live-synth path
at runtime, even for hexons that do have baked data.

**What's needed to close the loop**: a one-method, purely-additive add to
`composite.rs` (mirrors the existing pattern exactly):

```rust
/// Baked splat coverage buffer for a tile, if any hexon source carries one (FR-4).
pub fn get_baked_splats_sync(&self, coord: TileCoord) -> Option<&crate::splat::format::BakedSplatBuffer> {
    for src in &self.hexon_sources {
        if src.covers(coord) {
            if let Some(buf) = src.get_baked_splats(coord) {
                return Some(buf);
            }
        }
    }
    None
}
```

Once that lands, swap `find_baked_splats(composite, coord)` in
`render.rs::build_tile_splat_mesh` for `composite.get_baked_splats_sync(coord)`
directly (one-line change, function signature already matches).

Everything else (bake step, format, round-trip, fallback safety) is
complete and independently testable/verifiable without this seam — the seam
only affects whether the runtime *actually uses* baked data yet, not
whether the write/read/fallback machinery is correct. FR-5 (no crash, stable
holey rendering) holds regardless, since the fallback is exactly the
pre-existing code path.

## Other cross-worker note — RESOLVED

`fe-format::manifest::TilesetMeta` grew four new fields (`native_scale`,
`ground_sample_distance_m`, `crs`, `scale_bounds`, W3's scale-orchestration
track). Per coordinator steer, updated every `TilesetMeta { .. }` struct
literal in my owned files to name them (`None` for all four — this track
doesn't derive scale data):
- `fe-terrain/src/tiles/builder.rs` — both literals (`package_hexon`,
  `package_chunked`).
- `fe-terrain/tests/phase7_4a_qa.rs` — all four literals (`make_meta` +
  three inline ones).
No `fe-format` edits made (not my file).

## Verification status

Per instructions I did NOT run cargo/build/test — coordinator owns the
serialized sweep. All new logic (`bake.rs`, `format.rs`) has unit tests
written; `hexon_source.rs`, `fe-hexon/src/package.rs` tests added/extended.
Please run the full sweep and let me know if anything needs follow-up.

WORKER_COMPLETE W2.

## Coordinator fix: spatial-grid kNN correctness

`spatial_grid_finds_same_neighbors_as_brute_force` was failing —
`nearest_neighbors_grid` diverged from brute-force k-NN. Two independent
root causes in `fe-terrain/src/splat/bake.rs`, confirmed by Python simulation
against `grid_buffer(5,5,10.0)` before touching Rust:

1. **Completeness.** `nearest_neighbors_grid` only ever queried the fixed 3x3
   cell block (`SpatialGrid::nearby`). For edge/corner query points in a
   sparse region, the true k-th nearest neighbor can lie just outside that
   block while a closer-looking-but-farther point inside it gets kept
   instead. Reproduced concretely: corner splat 23 at `(40,30)` in the 5x5
   test grid picked `{13,16,17,18,19,21,22,24}` from the 3x3 block, but
   brute force's true 8-NN was `{12,13,17,18,19,21,22,24}`.
   Fix: added `SpatialGrid::nearby_ring(x, z, ring)` (generalizes `nearby`,
   which now delegates to `ring=1`), and rewrote `nearest_neighbors_grid` to
   expand the ring outward until it has `>= k` candidates **and** the k-th
   nearest distance found is `<= ring * cell_size` — the radius that ring is
   provably guaranteed to cover. Still O(1) amortized per query in the
   common case (ring 1 suffices at design density); only sparse/edge points
   pay for extra rings, and the loop terminates once a ring sweeps every
   remaining point.
2. **Tie-breaking.** Even after fixing completeness, one mismatch remained:
   a 3-way exact distance tie among candidates at the k-th cutoff resolved
   differently between the grid path (candidate order depends on
   `HashMap` bucket iteration, which is unspecified) and the brute-force
   path (stable sort over `0..n`, ties keep ascending index order). Fixed
   by sorting grid candidates on `(dist2, index)` instead of `dist2` alone,
   matching brute force's natural tie order.

Verified both fixes against 30 randomized point-set/cell-size/k trials in a
standalone Python model (0 mismatches) before applying to Rust, plus the
exact failing case from the Rust test. Did not run cargo per instructions —
edits are logic-equivalent to the verified Python model.

Files touched: `fe-terrain/src/splat/bake.rs` (`SpatialGrid::nearby_ring`,
`nearest_neighbors_grid`), `fe-terrain/src/splat/AGENTS.md` (new §bake
section documenting the two failure modes and fix rationale). No other
files touched.

FIX_COMPLETE grid

## Coordinator fix: build-time O(n²) elimination

An opus review found the bake reintroduced O(n²)/O(m²) work at BUILD time
(FR-1's whole reason for existing). At the builder's real stride
(`BAKE_SPLAT_STRIDE=1`, ~65k splats/tile × hundreds of tiles sequential)
these hang the build for hours; the 576-splat test fixture is too small to
surface them. Edit-only, `fe-terrain/src/splat/bake.rs` (+ AGENTS.md §bake).

**FIX 1 (HIGH) — push_midpoint this-pass self-scan was O(m²).** The second
cluster-rejection test scanned `self.positions` linearly
(`.iter().any(|p| xz_dist2(*p, pos) < min_sep2)`) over every midpoint emitted
so far *this pass*. The pass-start `SpatialGrid` only indexes `src`, so it
couldn't cover this-pass emissions. Replaced with a new `InsertGrid`: an
incremental XZ index of this-pass emissions with cell size = `min_sep`, so any
prior emission within `min_sep` of a candidate falls in its 3×3 cell block.
`any_within` checks the 3×3 block (O(1) amortized); each accepted midpoint is
`insert`ed as it's committed. Rejection semantics (same `min_sep2` threshold,
same accept/reject) are byte-identical to the linear scan → baked output
unchanged. `src` is still checked via the pass-start grid.

**FIX 2 (MEDIUM) — nearest_neighbors_grid full sort per ring + O(n) sweep.**
Was re-`sort`ing the entire candidate list on every ring iteration.
Replaced with `select_nth_unstable_by` at `k-1` (partial selection, O(m) not
O(m·log m)) to evaluate the covered-radius condition, then sorts only the
leading k for the ordered return. The expanding-ring correctness condition is
intact (ring grows until `≥ k` candidates AND k-th nearest `≤ ring·cell_size`)
and the `(dist2, index)` tie-break is preserved → still matches brute force.

**FIX 3 (MEDIUM) — SKIPPED (documented).** Cannot reuse/append the per-pass
`SpatialGrid`: its cell size is `spacing · EDGE_SPACING_TOLERANCE` and `spacing`
shrinks every pass as the field densifies. Appending to a stale grid leaves
cells sized for the prior coarser spacing, breaking the "3×3 block covers the
search radius" invariant both the k-NN query and the `src` cluster guard rely
on (silent neighbor divergence). Rebuild is O(n)/pass, passes capped at 8, so
total grid-build is O(8n) = O(n) — no asymptotic term to remove. Correctness
over a non-asymptotic constant win.

**Result:** no O(n²)/O(m²) term remains in the bake. Fill result is unchanged
(pure perf fix), so all existing correctness tests
(`coverage_fill_closes_holes`, `no_fill_when_already_covered`,
`fill_radius_scales_with_gap_and_shrinks`, `no_synthesized_point_crosses_seam`,
`no_degenerate_overlapping_clusters`, `deterministic_across_calls`,
`spatial_grid_finds_same_neighbors_as_brute_force`) still apply. Bake stride /
native splat cap untouched (output quality preserved). NOT verified with cargo
per coordinator instruction (serialized build owned by coordinator).

Files: `fe-terrain/src/splat/bake.rs`, `fe-terrain/src/splat/AGENTS.md`.
No other files touched.
