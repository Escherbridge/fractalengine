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

**Chunk lifecycle shadows the mesh.** `reconcile_splat_chunks` watches the same
`ActivePetalTerrain` / `ActiveTileSource` resources and the live `TerrainChunk`
set: for each mesh chunk lacking a splat shadow it re-fetches the tile (from the
composite cache) and bakes a `SplatChunk` at the mesh chunk's **own** anchor
`Transform` (guaranteeing alignment without recomputing the projection); splats
whose mesh chunk despawned (LOD / revision reset) are dropped. Spawns are budgeted
per frame. We deliberately **replicate the fetch** rather than edit
`terrain_plugin` (owned elsewhere) — the LOD ring/despawn/hysteresis math stays
solely in `terrain_plugin`, and splats inherit it for free by shadowing.

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

## §bake (`bake.rs`) — SpatialGrid k-NN correctness

`nearest_neighbors_grid` cannot stop at the query point's fixed 3x3 cell block:
for edge/corner points (or any locally-sparse region) the true k-th nearest
neighbor can sit outside that block even though more distant points inside it
get counted. Symptom is silent divergence from brute force, not a crash — the
neighbor set is just wrong, breaking hole-fill pairing.

Fix is a classic expanding-ring guarantee: grow the search ring (`nearby_ring`,
generalizes the old fixed `nearby`/3x3) until we have `>= k` candidates **and**
the k-th nearest distance found is `<= ring * cell_size` — the radius that ring
of cells is provably guaranteed to cover. Below that bound a nearer point could
still be hiding one ring further out. Amortized O(1) per query in the common
case (ring 1 almost always suffices at the grid's designed density); only
pathologically sparse regions pay for extra rings, and the loop always
terminates once a ring has swept every remaining point (`n - 1` candidates).

Second, independent bug: equal-distance ties. `HashMap` bucket iteration order
is unspecified, so sorting candidates by distance alone let a k-th-place tie
resolve to a different winner than brute force's stable sort over `0..n`.
Fixed by tie-breaking on index (ascending) in the same sort, matching brute
force's natural tie order exactly.

### Build-time O(n²) elimination (coordinator fix)

The bake runs offline, but at the builder's real stride (`BAKE_SPLAT_STRIDE=1`,
~65k splats per 256×256 tile, ×hundreds of tiles built sequentially) any O(n²)
term turns a "some constant offline cost is fine" step into an hours-long hang —
the exact failure class this track exists to remove. Two hot spots survived the
first port because the largest test fixture is only 576 splats:

- **This-pass cluster guard was O(m²).** `push_midpoint`'s second rejection test
  scanned `self.positions` linearly (`.iter().any(...)`) — a full sweep over
  every midpoint emitted so far *this pass*. The pass-start `SpatialGrid` indexes
  only `src` (the pass-start buffer), not this-pass emissions, so it couldn't
  cover them. In a sparse tile with big holes `one_pass` emits O(m) midpoints, so
  the guard was O(m²) per pass. Fixed with `InsertGrid`: an incremental XZ index
  of this-pass emissions whose cell size **is** `min_sep`, so any prior emission
  within `min_sep` of a candidate lands in the candidate's 3×3 block. Each
  accepted midpoint is inserted as it's committed; the check is O(1) amortized.
  The rejection semantics (same `min_sep2` threshold, same accept/reject
  decision) are identical to the linear scan, so the baked output is unchanged —
  a pure performance fix. `src` is still checked via the pass-start grid.

- **k-NN tail did a full sort per ring + could sweep O(n).** `nearest_neighbors_grid`
  re-`sort`ed the whole candidate list on every ring expansion. Replaced with
  `select_nth_unstable_by` at `k-1` (partial selection, O(m) not O(m·log m)) to
  test the covered-radius condition, then sorts only the leading k for the
  ordered return. The expanding-ring correctness condition is untouched — the
  ring still grows until `≥ k` candidates AND the k-th nearest distance is
  `≤ ring · cell_size`, with the `(dist2, index)` tie-break preserved so the
  result still matches brute force.

Grid reuse across passes (FIX 3) was intentionally **skipped**: the per-pass
`SpatialGrid` cell size is `spacing · EDGE_SPACING_TOLERANCE` and `spacing`
(median neighbor distance) shrinks every pass as the field densifies. Appending
to a stale grid would leave cells sized for the previous, coarser spacing,
breaking the "3×3 block covers the search radius" invariant that both the k-NN
query and the `src` cluster guard depend on — silent neighbor-set divergence.
Rebuilding is O(n) per pass and passes are capped at `MAX_INTERPOLATION_PASSES`
(8), so total grid-build cost is O(8n) = O(n) — no asymptotic term to remove.
Correctness beats a constant-factor win that isn't even asymptotic.
