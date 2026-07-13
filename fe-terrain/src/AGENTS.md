# fe-terrain/src — module notes

Design rationale for fe-terrain source modules. Code carries terse one-line doc
comments; the "why" lives here.

## §petal_binding

Runtime support for binding a terrain configuration to the active petal.

**Assignment flow.** The main binary bridges `DbResult::PetalTerrainLoaded`
(fe-runtime/fe-database) into a `TerrainAssignmentMsg` Bevy message (same
`Message` derive + `add_message` idiom as `DbResult` in fe-runtime). The
render-gated `apply_terrain_assignments` system consumes the **last** message
per frame and, in one pass:

1. Updates `ActivePetalTerrain` (petal_id, config, `revision += 1`). The
   revision counter is the cheap cross-system invalidation signal — e.g.
   `FailedTiles` in terrain_plugin clears itself when it observes a new
   revision.
2. Rebuilds `ActiveTileSource`: a fresh `CompositeTileSource` populated from
   `config.tileset_hexon_uris` via `SharedTilesetRegistry` (an
   `Arc<TilesetRegistry>` inserted by the main binary; each URI is loaded
   through `store().load_tileset`, failures warn and continue), the config's
   `tile_source_mode`, a `DiskTileCache` rooted at `config.cache_dir`
   (512 MB budget; skipped with a warn when the dir string is empty), and
   `projection = config.origin`. A `None`/disabled config clears both fields.
3. Rebuilds the `LayerStack` from `config.layers` via the pure
   `layers::layer_type_from_config_name(name, source_url)`: `"satellite"` →
   `Satellite`, `"terrain"` → `Terrain`, `"gpx_track"` → `GpxTrack { node_id, .. }`,
   `"geojson_overlay"` → `GeoJsonOverlay { source_path }`, anything else warns and
   skips. The identifier-bearing variants take their `node_id` / `source_path` from
   the config's `source_url` (empty when absent) so the layer entry binds to the
   matching track/overlay entity in `render_gpx_tracks` / `render_geojson_overlays`
   — this is what lets fe-ui's GPX/GeoJSON visibility checkboxes drive those layers
   (previously they were disabled because no `LayerStack` entry existed). `z_order`
   = config index, opacity defaults to 1.0.
4. Despawns every `TerrainChunk`, `GpxTrackLine`, `GeoJsonOverlay`, and
   `GeoJsonProcessed` entity so content respawns under the new config.

**Config reconciliation (2026-07).** The stored petal config is not trusted
for two fields, because fe-ui's `tileset_to_terrain_json` cannot know them
(the store/DTO layer doesn't carry encoding, and origin elevation is unknown
at write time):

- `elevation_source` is overridden from the **loaded hexon's**
  `tileset_meta.elevation_encoding` (first source wins). Decoding Terrarium
  bytes with the Terrain-RGB formula puts vertices ~900 km up — invisible
  past the camera's 1000 m far plane, which read as "terrain doesn't load".
  `Raw16` has no decoder yet → `None` (flat).
- `origin.origin_ele == 0.0` is treated as "unset" and grounded via
  `sample_center_elevation` (mean of the origin tile at min zoom), because
  mesh world-Y = absolute elevation − origin_ele; real terrain (e.g. Pacific
  NW at 500–1500 m ASL) would otherwise float far above the camera.

The reconciled config is written back to `ActivePetalTerrain` so
`fetch_and_spawn_terrain_chunks` and the projection agree.

**Non-gated helpers.** `terrain_config_from_petal_json` parses a petal
record's `terrain` JSON property (null/invalid → `None`, invalid warns).
`config_for_tileset` builds an enabled default config from a `TilesetInfo`:
origin at the bounds center (bounds are `[min_lat, min_lon, max_lat,
max_lon]`, per `HexonTileSource::bounds`/`covers`), zooms from the tileset
range, `tileset_hexon_uris = [tileset_id]`, Hybrid mode, satellite+terrain
layers. `elevation_source` maps `ElevationEncoding::TerrainRgb` →
`TerrainRgb`, anything else (or unknown) → `None` (flat). To make the
encoding reachable, `TilesetInfo` gained an additive, serde-defaulted
`elevation_encoding: Option<ElevationEncoding>` field populated from the
registry's in-memory sources cache.

## §terrain_plugin

Bevy plugin for terrain chunks, GPX tracks, waypoints, GeoJSON overlays, and
layer visibility. System chain order matters: `apply_terrain_assignments`
runs first so the frame's assignment is visible to everything downstream.

**Offline-first chunk pipeline** (`fetch_and_spawn_terrain_chunks`):
- Early-outs: no enabled config, no composite/projection, no camera, or at
  `max_chunks`.
- Camera local position → `local_to_wgs84` → desired zoom via
  `desired_zoom(cam_height, min_zoom, max_zoom)` (one zoom step out per
  doubling of height above a 200 m base; monotonic + clamped).
- 3×3 tile ring around `TileCoord::from_lat_lon(lat, lon, zoom)`; tiles
  already spawned (by `(zoom, x, y)`) or previously failed are skipped.
- Per tile: elevation PNG via `CompositeTileSource::get_tile_sync` decoded by
  the config's `ElevationSourceKind` decoder → `terrain_mesh`; satellite via
  the new `get_satellite_tile_sync` (hexon → disk cache namespace
  `composite_sat`; mirrors `get_tile_sync`) decoded to an RGBA texture. No
  elevation but satellite → flat 16×16 grid; neither → skip + record in
  `FailedTiles` (a `HashSet<(u8,u32,u32)>` cleared on revision change) so a
  missing tile warns once instead of every frame.
- Geometry: `terrain_mesh` anchors at local (0,0,0) extending +x/+z with row
  index driving +z. World axes are x=east / z=north, while tile image row 0
  is the north edge — so elevation rows are flipped (`flip_rows`), satellite
  images v-flipped, and the chunk transform anchors at the tile's **SW
  corner** (`wgs84_to_local(south_lat, west_lon, 0.0)`; passing ele=0 makes
  mesh y, absolute meters, land at `ele - origin_ele`). Tile edge length =
  `tile_world_size_m(center_lat, zoom)` (equator zoom 0 ≈ 40,075 km circumference).
- Chunks link to the Satellite (textured) or Terrain layer via `LayerEntity`
  when present in the stack.

**Bug fixes hardened in this pass:**
- `render_waypoint_markers` leaked a mesh + material asset every frame when
  there were no waypoints; now early-returns on an empty query.
- `render_geojson_overlays` respawned overlay meshes every frame: the source
  query was `Without<Mesh3d>` and children carried a cloned `GeoJsonOverlay`,
  so nothing ever stopped matching. Now sources are marked `GeoJsonProcessed`
  up-front (also on read/parse failure, with a warn, to avoid retry spam) and
  children carry `GeoJsonProcessed` + optional `LayerEntity` instead of the
  overlay component.
- `update_terrain_lod` had two empty if/else branches (dead code); now it
  despawns chunks whose zoom differs from the camera's desired zoom, plus the
  original despawn-too-far (max distance never shrinks below ~2 tile widths
  so large low-zoom tiles don't churn).
- `fetch_and_spawn_terrain_chunks` was a placeholder; now the offline-first
  pipeline above.
- `render_gpx_tracks` skips non-finite points and attaches `LayerEntity` for
  a matching `LayerType::GpxTrack` (by `node_id`).

**§track-styling (track_styling_20260713).** Per-track color / thickness /
visibility. `TrackStyle { color:[f32;4], width, visible }` (plain struct, no
Bevy dep — `iot/animation.rs`) + `TrackStyleMap` resource (render-only, keyed
by track node id) live next to `TrackRouteMap`; `parse_track_color_hex` /
`track_color_to_hex` handle the `#rrggbbaa` persistence format (invalid → the
caller's default, never panics — FR-4). `render_gpx_tracks` now builds the
width-aware ribbon via `mesh::track::track_mesh(points, style.width,
ColorMode::Solid(color))` instead of a 1px `LineStrip`, so thickness is real;
the material is `base_color: WHITE, unlit, cull_mode: None` because the solid
color is baked into the mesh's vertex colors (same idiom as `splat/render.rs`).
Visibility is applied at build time as `Visibility::Hidden`/`Inherited` (there
is no separate in-place visibility fast path — a `gis.track.visible` toggle
persists and re-runs the whole build, see `terrain_plugin.rs`). **LOW-1 accuracy
note:** absent style ⇒ `TrackStyle::default()` — so the default *style values*
(cyan `#00ccff`, `width 2.0`, visible) match the historic look, but the
**rendering changed intentionally**: every track — even one with no persisted
`gis.track.*` props — is now an unlit width-`2.0` ribbon, NOT the old lit 1-px
`LineStrip`. So there is a deliberate visual delta on pre-existing tracks; the
values are historic, the geometry/material are new. The map is populated by the
fractalengine gpx bridge (`advance_path_materialization`) from `gis.track.*`
node props; live restyle is a despawn+respawn of the `GpxTrackLine` (same
`force_line_redraw` discipline point-edits use — see
`fractalengine/src/AGENTS.md`).
- `sync_layer_visibility` ran every frame; now gated on
  `layer_stack.is_changed()`, and opacity < 1.0 also sets
  `AlphaMode::Blend` (alpha alone doesn't blend on `StandardMaterial`).

**LOD hardening (terrain_lod_hardening_20260711).** Three region-scale
(`world_scale < 1`) rendering defects were fixed. The pure math lives in
`lod_ring.rs` (no `bevy`, always compiled + unit-tested) and `mesh/skirt.rs`
+ `mesh/interp.rs` (pure grid helpers under the non-gated `mesh` module):

1. **Seams (thin black lines between tiles).** `terrain_mesh` grows skirt walls
   around each tile's four edges (`mesh/skirt.rs`). Edge vertices are duplicated;
   the copy drops by `depth` **and flares outward** (away from the tile centre)
   by `overlap` in the XZ plane. Each wall segment is emitted with **both
   windings** (two-sided) so it never culls to the background; skirt UVs copy the
   edge UV so the satellite texture / terrain colour continues down the wall.
   Skirt vertices/indices are appended **after** the base normals are finalised,
   and base normals are computed only over base indices, so base lighting is
   unaffected.

   **Actual high-altitude seam cause (round-2, `terrain_seams_hi_alt_20260711`).**
   Round-1's straight-down skirt only covers *vertical* elevation-step seams. The
   residual thin dark lines seen zoomed-out were **horizontal XZ gaps**: each tile
   is anchored at its true projected SW corner (`FlatProjection`, cosine at the
   *origin* latitude) but meshed as a square of side `tile_world_size_m` (mercator,
   cosine at the *tile* latitude). Those two formulas disagree away from the origin
   latitude, so neighbour tiles don't perfectly abut — a sub-tile sliver of dark
   background shows between them, growing with latitude offset and tile size (low
   zoom / altitude), and a **vertical** skirt cannot cover a **horizontal** gap.
   Fixes:
   - **Outward flare (`SKIRT_OVERLAP_FRACTION` = 2% of tile size).** The skirt
     bottom is pushed outward so the wall becomes a down+outward apron that laps
     over the gap onto the neighbour. Where the neighbour surface exists it
     occludes the apron from above (no fringe); through the gap the apron fills
     the background. This is the primary residual-seam fix.
   - **Depth floor (`SKIRT_MIN_FRACTION` = 1.5% of tile size).** `skirt_depth`
     now returns `max(SKIRT_TEXELS × cell_size, SKIRT_MIN_FRACTION × tile_size)`.
     A 256-wide tile's texel-based depth is only ~0.78% of the tile; the floor
     keeps low-zoom skirts visually significant versus edge-height disagreements.
   - **Satellite sampler clamp-to-edge + linear** (`decode_satellite_image`). Set
     explicitly on the `Image` so a tile's UV 0/1 borders can never wrap-bleed into
     the opposite edge under minification — a classic thin dark tile-border line at
     distance — regardless of the app's global default sampler.

   Both `depth` and `overlap` are fractions of the **already-scaled** tile size, so
   they scale with the tile and 1:1 behaviour is unchanged (the base mesh, and thus
   the world-Y = `(ele − origin_ele) × scale` invariant, is untouched — the skirt
   only appends geometry below the edges). **Hypothesis 2 (decoded-edge harmonisation
   cache)** was weighed and **deferred**: the apron + depth floor cover the visible
   defect without a stateful per-`(z,x,y)` border cache and its spawn-order coupling.
   **Hypothesis 4 (cross-zoom T-junctions)** is covered — skirts are built for every
   tile regardless of zoom, and the deeper/flared skirt on the *coarser* tile (the
   one that matters during a LOD transition) spans the finer neighbours.

2. **Zoom-out clipping / holes.** The fetch ring is no longer a fixed 3×3:
   - **Adaptive radius.** `view_radius_world(cam_height_world, far_world,
     VIEW_RADIUS_FACTOR)` = camera world-height × 3, capped by the camera's
     **actual perspective far plane** (read from the bevy `Projection`
     component — the single source of truth that fe-renderer's
     `scaled_far_plane` already sizes to the scale). `ring_radius_tiles`
     converts that world radius into a tile radius, clamped to
     `max_ring_radius_for_budget(max_chunks)` so `(2r+1)² ≤ max_chunks`.
     At 1:1 this collapses back to radius 1 (≈ the old 3×3) — no regression.
   - **Nearest-first, budgeted.** `ring_offsets(radius)` yields offsets sorted
     by squared distance; spawning stops at `max_chunks` or
     `MAX_SPAWNS_PER_FRAME` (16) so a large ring fills the visible centre first
     and never hitches (it accumulates across frames).
   - **Despawn hysteresis.** `spawn_despawn_radii` returns a despawn radius
     strictly greater than the spawn radius (`× DESPAWN_HYSTERESIS = 1.5`,
     floored to ~2 tiles + the LOD base), so a tile that is still visible is
     never despawned — no flicker, no edge holes.
   - **Hole-free wrong-zoom despawn.** A chunk at the wrong zoom is despawned
     **only once its desired-zoom replacement fully covers it**
     (`wrong_zoom_replacement_present` → `covering_tiles`: parent for
     zoom-out, the `f×f` children for zoom-in, bounded by `MAX_COVER_DZ`).
     `update_terrain_lod` runs before `fetch_and_spawn`, so on a zoom change
     the old coverage is kept until the new tiles exist — brief overlap
     instead of a gap.

3. **Close-range quality (blocky terrain).** When the camera descends below the
   served max zoom's natural height (`desired_zoom` is clamped to
   `config.max_zoom`, so `zoom >= config.max_zoom` means "can't fetch finer"),
   `upsample_factor_for_height(cam_real_height, ZOOM_BASE_HEIGHT_M,
   MAX_UPSAMPLE)` returns a power-of-two factor (≤ 4×; each halving below the
   200 m base doubles it). `spawn_chunk` then `upsample_bilinear`s the decoded
   elevations onto a denser grid **before scaling**, so the world-Y invariant
   `(ele − origin_ele) × scale` is preserved (bilinear samples are convex
   combinations of the originals, and original samples land exactly on grid
   nodes). Geometry stays smooth up close.
   **Honest limitation:** only the *elevation mesh* is densified. Satellite
   **texture** sharpness is capped by the source hexon's max-zoom tiles —
   higher-zoom imagery is a **gis-tile-etl** concern (produce deeper-zoom
   hexons), not a renderer one. The upsampled mesh drapes the same texels more
   smoothly but cannot invent texture detail.

`ZOOM_BASE_HEIGHT_M` (200 m) is the single source for `desired_zoom`'s base and
the close-range trigger, keeping the two in lock-step.

## §scale

`TerrainConfig.world_scale` (serde default `1.0`, additive — pre-feature petal
JSON keeps parsing) is **world units per real meter** (`0.001` → 1 unit per km).
It lets the user operate at several scales of space: a zoom-8 tile is ~110 km,
so at 1:1 you only ever see a sliver, while at 1:1000 the whole region fits.

Pure math lives in `scale.rs` (not render-gated, unit-tested without `bevy`):
`sanitize_world_scale` (finite & > 0, else 1.0), `scaled_tile_size`,
`world_to_real_height` (the inverse used for zoom selection), `scale_local`,
`scale_elevations`. `TerrainConfig::effective_world_scale()` wraps the sanitizer
so bad JSON never divides by zero or flips signs.

Applied in `terrain_plugin`:
- `spawn_chunk` scales the **tile mesh size**, the **decoded elevations**, and
  the **anchor transform** by `scale`. Composition is the load-bearing invariant:
  anchor.y = `-origin_ele * scale` (from `wgs84_to_local(_, _, 0.0)` then
  `scale_local`) and mesh y = `ele * scale`, so world-Y = `(ele - origin_ele) *
  scale`. `origin_ele` is still grounded in **real meters** in `petal_binding`
  (pre-scale) — the subtraction composes cleanly because both terms are scaled
  together.
- `fetch_and_spawn_terrain_chunks` and `update_terrain_lod` **invert** scale: the
  camera is projected/zoom-selected in real meters (`cam / scale`,
  `world_to_real_height`). LOD despawn distance stays in world units — the tile
  term is scaled (`scaled_tile_size`) while the `~2 tile` floor is a world-unit
  heuristic left as-is so 1:1 behavior is unchanged.

A live scale change round-trips as a `SetPetalTerrain` → `PetalTerrainLoaded` →
`TerrainAssignmentMsg`, bumping `ActivePetalTerrain.revision` and respawning
chunks (no restart). Camera limits adapt via fe-renderer's `CameraScaleSettings`.
