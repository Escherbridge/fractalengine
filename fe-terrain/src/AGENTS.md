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
3. Rebuilds the `LayerStack` from `config.layers` by name: `"satellite"` →
   `LayerType::Satellite`, `"terrain"` → `LayerType::Terrain`, anything else
   is skipped with a warn (honest minimal mapping — GPX/GeoJSON layers are
   driven by their own entities, not petal config). `z_order` = config index,
   opacity defaults to 1.0.
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
- `sync_layer_visibility` ran every frame; now gated on
  `layer_stack.is_changed()`, and opacity < 1.0 also sets
  `AlphaMode::Blend` (alpha alone doesn't blend on `StandardMaterial`).

**LOD hardening (terrain_lod_hardening_20260711).** Three region-scale
(`world_scale < 1`) rendering defects were fixed. The pure math lives in
`lod_ring.rs` (no `bevy`, always compiled + unit-tested) and `mesh/skirt.rs`
+ `mesh/interp.rs` (pure grid helpers under the non-gated `mesh` module):

1. **Seams (thin black lines between tiles).** `terrain_mesh` now grows
   **downward skirt walls** around each tile's four edges (`mesh/skirt.rs`).
   Edge vertices are duplicated and dropped by `skirt_depth = SKIRT_TEXELS ×
   cell_size` (world units — the cell size is already scaled, so the skirt
   scales with the tile). Each wall segment is emitted with **both windings**
   (two-sided) so it never culls to the background regardless of view angle;
   skirt UVs copy the edge UV so the satellite texture / terrain colour
   continues down the wall. This hides both float-precision gaps at
   `scaled_tile_size` edges and small edge-height disagreements between
   adjacent tiles (which decode their borders from separate tiles). Skirt
   vertices/indices are appended **after** the base normals are finalised so
   base lighting is unaffected. Base normals are still computed only over base
   indices.

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
