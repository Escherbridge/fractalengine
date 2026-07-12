# fe-terrain/src/splat — module notes

Design rationale for the terrain **splat view** (track
`terrain_splat_view_20260711`, phase 1, FR-1..FR-3). Code carries terse one-line
doc comments; the "why" lives here. Splats are a soft, fast, LOD-friendly
alternative representation rendered *beside* the classic mesh (`terrain_plugin`),
not a replacement.

Layout mirrors the rest of the crate: pure math is always compiled and unit-tested
without `bevy` (`synth.rs`, `view_mode.rs`), the Bevy plugin is render-gated
(`render.rs`). `lib.rs` gains a single `pub mod splat;` line; everything else in
`splat/` is self-owned.

## §synthesis (FR-1, `synth.rs`)

`synthesize_splats(elevations, w, h, satellite, tile_world_size, world_scale,
stride) -> SplatBuffer{positions, colors, scales, normals}` — one splat per
stride-decimated elevation texel.

**Position mirrors `terrain_mesh` exactly** so a splat chunk drops onto the same
SW-corner anchor transform as its mesh chunk and aligns 1:1:

- `x = col * cell_x`, `cell_x = tile_world_size / (w-1)` (world units, already scaled).
- `z = (h-1-row) * cell_z` — the **row flip**: decoded row 0 is the tile's north
  edge, world +z is north, and the anchor sits at the SW corner, so north lands at
  max +z (identical to `flip_rows` + `terrain_mesh` in `terrain_plugin::spawn_chunk`).
- `y = ele * world_scale`. Combined with the caller's anchor
  (`anchor.y = -origin_ele * scale`) this yields the invariant
  `world-Y = (ele - origin_ele) * scale` — the same composition the mesh uses
  (see crate `src/AGENTS.md` §scale). Synthesis intentionally does **not**
  upsample (that is a mesh-only close-range concern); stride only decimates.

**Slope-aware anisotropy.** Central differences on the (scaled) height field give
the world-space gradient `(gx, gz)`; the surface normal is
`normalize(-gx, 1, -gz)`. The splat's minor radius is a fixed fraction of the
decimated texel spacing (soft overlap) and the major radius elongates along the
slope by `sqrt(1+|g|^2)` (capped): flat ground → a wide flat isotropic disc,
cliffs → an ellipse elongated down-slope. The major-axis *direction* is not stored
— it is the steepest-descent direction, recovered from the normal alone at bake
time (`SplatBuffer` stays exactly `{positions, colors, scales, normals}`).

Color is a nearest-neighbor RGBA sample of the satellite tile at the texel
(north-up; no v-flip, because synthesis indexes by decoded row directly, unlike
the mesh path which v-flips the texture). Tiles with no satellite fall back to a
neutral terrain color.

**Anti-moiré polish (visual quality pass).** A perfectly regular grid of
uniform-size discs reads as a texture/dot-grid pattern rather than organic
coverage, so three deterministic (hash-based, no RNG crate) perturbations are
applied per splat, keyed on `(row, col)`:

- `SPLAT_COVERAGE` raised to `0.8` so the minor radius overlaps a neighbor's by
  ~1.4-1.8x the decimated spacing (was exact/near-exact tiling).
- Positional XZ jitter up to `±JITTER_FRACTION` (35%) of spacing, so splats
  don't sit on a perfectly regular lattice.
- Radius variation up to `±RADIUS_VARIATION_FRACTION` (20%) of the base minor
  radius, applied to both major/minor so isotropy on flat ground is preserved.

`hash01`/`hash_signed` are a pure xorshift-style integer mix — deterministic
(same `(row, col)` always jitters identically, so re-synthesis/mode-switch
re-bakes are stable) and dependency-free. Distance-aware sizing (larger/fewer
splats far from camera) was scoped out: `render.rs`'s bake path has no camera
distance threaded through per-splat (only `apply_view_mode_visibility` reads
camera position, at the whole-chunk level for Hybrid mode); adding it would
mean a new per-chunk or per-frame resource dependency into the bake path,
which is out of scope for this pass. Follow-up if revisited.

## §interpolation (`interpolate.rs`, track `splat_marching_squares_20260712`)

Density **fallback** for when the splat LOD zoom pipeline
(`splat_lod_zoom_20260712`) hits the tileset `max_zoom` ceiling: no higher-res
tile exists to fetch, so instead of holding flat density we synthesize
intermediate splats by upscaling the already-baked `SplatBuffer` (no new
network/tile fetches — operates purely on resident data). Pure, bevy-free,
always compiled, unit-tested — same pure/render split as `synth.rs`.

- **Ceiling detection (FR-1):** `splat_needs_interpolation(requested, actual)` =
  `requested > actual`. `requested` is the sibling track's `splat_desired_zoom`;
  `actual` is the zoom the sub-tile was actually baked at. Never fires when real
  tile data already satisfies the request → zero behavior change off the ceiling.
- **Marching-squares synthesis (FR-2):** operates on the exact same `SplatBuffer`
  shape `synth.rs` produces (parallel `positions/colors/scales/normals`), so the
  output composes straight into `bake_splat_mesh` with no translation. The buffer
  carries no grid topology, so the implicit XZ grid is reconstructed per pass from
  nearest-neighbor distances; each cell **edge** (a close neighbor pair within
  `EDGE_SPACING_TOLERANCE` × current spacing) gets one midpoint splat whose
  position/normal/color/scale are the interpolated average of its two endpoints.
- **Coverage-driven hole fill (re-aimed 2026-07-12):** the primary goal is to
  leave **no untouched background** between splats. The visible defect was dark
  holes where neighboring splat discs fail to touch (jitter + irregular spacing
  open gaps even *below* the max_zoom ceiling), so the fill is now driven by
  **coverage, not zoom**: `augment_splat_buffer_coverage(baked)` runs whenever
  holes exist, ungated by the zoom ceiling. `augment_splat_buffer_if_needed`
  (ceiling-gated) remains for the complementary real-higher-res path.
  - For each near neighbor pair, the **uncovered gap** is `dist − (minor_a +
    minor_b)`. Pairs whose discs already touch (`gap ≤ 0`) are skipped — nothing
    to fill. Pairs with a real gap get one midpoint splat.
  - **Proximity-driven scale:** the new splat's minor radius is
    `gap × GAP_FILL_RADIUS_FRACTION (0.6)` — sized to bridge *that specific gap*
    and overlap both endpoints. Big holes → big fill splats; as passes densify the
    field, residual gaps (and thus fill radii) shrink logarithmically on their own.
    Discs are round (major ≈ minor) — a hole-filler has no slope to elongate along.
  - **Coverage-driven termination (whichever first):** **(a)** a pass adds nothing
    (every pair now touches → no holes); **(b)** the bridging radius drops below
    `base_spacing × MIN_FILL_RADIUS_FRACTION (0.08)` (residual gap is sub-splat,
    already effectively touched); **(c) seam** — a midpoint outside this tile's
    `TileFootprint` is dropped so neighbors never double-fill the shared edge.
    `MAX_INTERPOLATION_PASSES = 8` is a pure degenerate-input backstop.
  - **Anti-cluster floor:** a midpoint within `base_spacing ×
    MIN_MIDPOINT_SEP_FRACTION (0.4)` of any existing/just-emitted splat is dropped
    (no coincident points → no moiré / z-fight).
- **Anti-dot-grid (FR-3):** synthesized points reuse synth's deterministic
  hash-based jitter — a self-contained mirror of `hash01`/`hash_signed` (those are
  private in `synth.rs`; duplicated with the identical mix rather than editing that
  file, since a sibling worker owns the module this session), `±JITTER_FRACTION`
  XZ position jitter + `±RADIUS_VARIATION_FRACTION` radius variation, keyed on the
  edge and salted per pass so successive generations don't stack identical offsets.
  Each generation's radius scales toward the native so doubled density keeps
  proportional overlap instead of fat blobs. Result: upscaled and native points are
  statistically indistinguishable in density pattern.
- **Integration (FR-4):** `build_tile_splat_mesh` calls
  `augment_splat_buffer_coverage(baked) -> Option<SplatBuffer>` per sub-tile
  before `bake_splat_mesh`; `None` means "no holes, use the baked buffer
  unchanged". The ceiling-gated `augment_splat_buffer_if_needed(baked,
  requested_zoom, actual_zoom)` / `_within` variants remain in the API for the
  complementary real-higher-res path but are not the primary hole-fill driver.

## §rendering (FR-2, `render.rs`)

Pragmatic v1 — **no custom render pipeline**. Per tile, every splat is baked into
**one** `Mesh` (a normal-oriented quad per splat, vertex colors from the satellite
sample) sharing a single procedurally-built `StandardMaterial`:

- **Soft disc** = a small radial-falloff alpha texture (`make_soft_disc_image`),
  `AlphaMode::Blend`, `unlit`, `cull_mode: None` (draped quads may face away).
  The white texture is tinted per-vertex, so one material serves every tile.
- **One draw call per tile** (distinct meshes, shared material — no per-frame
  billboarding, no re-bake; the quad is oriented by the surface normal once).
- Each quad's in-plane basis is `(t = downhill, b = n×t)` scaled by
  `(major, minor)`; `t` is derived from the normal so orientation needs no extra
  vertex data.

**Chunk lifecycle shadows the mesh, with an independent splat zoom.**
`reconcile_splat_chunks` watches the same `ActivePetalTerrain` / `ActiveTileSource`
resources and the live `TerrainChunk` set. Each `SplatChunk` spawns at the mesh
chunk's **own** anchor `Transform` (guaranteeing alignment without recomputing the
projection) but is a *container*: its child entities are the actual baked splat
sub-tile meshes. Splat resolution is decoupled from the mesh LOD via
`lod_ring::splat_desired_zoom(cam_height_m, mesh_zoom, min_zoom, max_zoom)` (the
splat-track "zoom in for higher-res imagery" pipeline, `splat_lod_zoom_20260712`):
when the camera is close it requests up to `SPLAT_CLOSE_ZOOM_BONUS` extra zoom
steps beyond the mesh chunk's zoom, clamped to the tileset's `max_zoom` (FR-5
graceful ceiling — no error past the top source zoom, splats just stop densifying).

At a higher splat zoom one mesh chunk's footprint needs several sub-tiles
(`lod_ring::covering_tiles`, ~4× per zoom step); each sub-tile is fetched at that
zoom via the composite (already accepts arbitrary `TileCoord` — no cache changes)
and baked in tile-local space, then re-anchored inside the parent via
`sub_tile_local_offset` (projection SW corner minus the parent world anchor).

Despawn/respawn coherence (FR-4): a `SplatChunk` records the `splat_zoom` it was
baked at; when the camera-driven `splat_desired_zoom` changes, the whole chunk
(container + children) is despawned and re-baked at the new zoom. Per-frame budget
counts baked *sub-tile meshes* (`MAX_SPLAT_SPAWNS_PER_FRAME`), so a rapid zoom-in
that quadruples tile count still spreads the bake cost across frames. A chunk is
only committed once ≥1 sub-tile baked (no empty container entities). Splats whose
mesh chunk despawned (LOD / revision reset) are still dropped. We deliberately
**replicate the fetch** rather than edit `terrain_plugin` (owned elsewhere) — the
mesh LOD ring/despawn/hysteresis math stays solely in `terrain_plugin`.

Splats are only baked in `Splats`/`Hybrid` mode; `Mesh` mode drops them all, so
the default view carries zero splat cost.

## §view_mode (FR-3, `view_mode.rs` + `render.rs`)

`TerrainViewMode { Mesh, Splats, Hybrid }` — serde snake_case, default `Mesh`,
and a Bevy `Resource` under the `render` feature (conditional derive so the pure
build stays bevy-free). Parsed from the petal terrain JSON's additive
`"view_mode"` field via `view_mode_from_terrain_json` (missing/null/unknown →
`Mesh`, matching serde's graceful default).

**Ingestion.** `TerrainConfig` (owned by another worker) drops unknown JSON keys,
so `view_mode` cannot ride the existing `TerrainAssignmentMsg`. Instead the main
binary's `PetalTerrainLoaded → TerrainAssignmentMsg` bridge also parses the raw
terrain JSON and emits `TerrainViewModeMsg` (see the track's INTEGRATION_REQUEST);
`apply_view_mode_msgs` applies the latest to the resource. Until that one bridge
line is wired the resource stays `Mesh` — safe graceful degradation (mesh view
unchanged, splats simply never appear).

**Visibility.** `apply_view_mode_visibility` sets `Visibility` every frame:

- `Mesh`: mesh chunks visible, splats hidden.
- `Splats`: mesh chunks hidden, splats visible.
- `Hybrid`: per chunk by camera distance vs a **scale-aware** threshold
  (`hybrid_mesh_distance_m * world_scale`, world units) — mesh within, splats
  beyond.

Mesh-chunk visibility is composed with the chunk's `LayerEntity` layer visibility
(`mode_wants_mesh && layer_visible`) so this system is the single authority for
terrain-chunk visibility and does not fight `terrain_plugin::sync_layer_visibility`
(which runs only on layer change and still owns per-layer opacity/alpha). Writes
are diffed so no needless change-detection churn. Splat chunks are governed by mode
alone (no layer binding in v1).

## Known limitations / caveats (v1)

- **Intra-mesh blend order.** Alpha-blended discs within one tile mesh are not
  depth-sorted against each other; the soft falloff hides most ordering artifacts.
  Per-tile entities are sorted by Bevy's transparent pass. A proper sorted/OIT
  pass is future work.
- **Re-decode on spawn.** Splat chunks re-fetch + re-decode their tile (from the
  cache) rather than sharing the mesh path's decode. Bounded by the per-frame
  spawn budget; a shared decode cache would need `terrain_plugin` changes.
- **Mode-switch bake cost.** Switching into `Splats`/`Hybrid` bakes the whole
  visible ring over a few frames (budgeted); a one-time cost, not a steady-state
  one. Texture sharpness is still capped by the source hexon's max-zoom tiles
  (a `gis-tile-etl` concern, same as the mesh).
- **Phase 2 (`splat_ready` precomputed buffers) is out of scope here** — FR-1
  runtime synthesis is the fallback that phase 2's install path will defer to.
