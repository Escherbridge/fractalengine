# fe-terrain/src — module notes

Design rationale for fe-terrain source modules. Code carries terse one-line doc
comments; the "why" lives here.

## §earthwork-validity-and-strength

Ghost suppression requires a valid bake record, not merely a `material` key:
non-empty id/material, at least three finite footprint points with nonzero
shoelace area, sculpt vocabulary, and the op-required finite target/delta. Malformed
material-bearing proposals remain ghosts instead of disappearing.

Level and Smooth store their `[0,1]` strength compatibly in the region `delta` slot.
`EarthworkRegion::effective_strength`, bake preparation, and volume math all
consume that value; other operations keep the existing delta meaning.

## §camera-framing — agent vision pose math (`camera_framing.rs`, agent_vision_20261031 V3)

Pure `f64`, bevy-free pose resolution for the `tile`/`geo` capture pose kinds
(`fe-runtime::vision::PoseSpec`). Unconditional module (no `render` feature
gate) — V5's Bevy orchestrator calls it, but it has no bevy dependency itself,
so it unit-tests headless and compiles for every build of this crate.

- **Axis convention**: x = east, z = north, y = up — the same convention
  `terrain_plugin.rs` and `projection.rs` use. World = local × `world_scale`
  (`scale::scale_local`), always. There is no separate convention for capture
  poses; they go through the identical `Projection::wgs84_to_local ×
  scale_local` pipeline `terrain_plugin::spawn_chunk` uses for its tile
  anchor, which is *why* `frame_bounds_top_down`'s target lands in the same
  world-space frame as the chunk it is framing (the invariant
  `framed_tile_centre_is_the_chunk_anchor_space_point` checks).
- **The live projection rule**: callers MUST pass the live, runtime-grounded
  projection — `petal_binding::ActiveTileSource.projection` — never the
  persisted petal config's projection. Center-tile grounding
  (`fetch_and_spawn_terrain_chunks`) overwrites `origin_ele` at runtime, and a
  capture framed against the stale persisted value would float above or sink
  below the terrain that is actually resident. This module takes `&Projection`
  as a plain parameter and never reaches into petal state itself — the caller
  (V5, `fractalengine`) owns getting the live one.
- **`tile_bounds(zoom, x, y) -> [min_lat, min_lon, max_lat, max_lon]`**: a
  tile's full geographic extent (NW corner → NW corner of `(x+1, y+1)`), the
  same rule `tiles::hexon_source::HexonTileSource::covers` and
  `tiles::bundle::bounds_for_tile_range` use — not just the NW-corner point,
  which can wrongly call a coarse tile "outside" a tightly-drawn bounds box.
- **`coverage_check(tile_bounds, tileset_bounds: &[[f64; 4]]) -> Coverage`**
  takes plain bounds arrays, not `fe_format::manifest::TilesetMeta`, so this
  module never depends on `fe-format`. The caller passes
  `CompositeTileSource::source_metas().iter().map(|m| m.bounds).collect()`.
  Empty input is `Coverage::Unbounded` (online/demo mode with no installed
  tilesets — the capture proceeds, per spec §5.3), not `Outside`.
- **`union_bounds(&[[f64; 4]]) -> Option<[f64; 4]>`** (advocate pass,
  2026-11-01; moved out of `fractalengine`'s `vision_bridge.rs`): the box
  enclosing every bound tileset — the `focus {petal_id}` fallback frame for a
  petal with no visible nodes, and `GET /camera`'s `tileset_bounds`. `None`
  for no tilesets.
- **`frame_bounds_top_down`**: fits BOTH the lat and lon extents for the
  output aspect ratio via `tan(h_fov/2) = tan(v_fov/2) * aspect` — no separate
  horizontal-FOV input, just the standard aspect/vertical-FOV relationship.
  The ×1.1 margin is spec-mandated headroom, not a magic fudge; a degenerate
  (point-sized) bounds box is floored at `MIN_FRAME_DISTANCE_M * world_scale`
  so the camera can never land exactly on its target.
- **`geo_pose`**: `heading_deg` 0 = north (+z), clockwise toward +x (east);
  the target is one unit along the view direction (`pose_transform` in
  fe-renderer only needs a point ahead of `position`, not a framed distance —
  unlike `frame_bounds_top_down`, which frames a whole region).
  `ground_or_origin_m` is resolved by the caller (resident
  `TerrainHeightField` sample for the "terrain" altitude reference, or
  `origin_ele` for "origin") — this function only does the geometry once that
  choice is made.
- **`within_projection_range`**: the ~500 km accurate range `projection.rs`
  itself documents for `flat_projection`; a `geo` pose beyond it is refused
  (`geo_out_of_projection_range`, a V5/fe-api concern) rather than silently
  producing a distorted pose.
- Every entry point validates every float finite and every geometric
  degeneracy (non-positive `world_scale`, zero/negative FOV, non-positive
  aspect) before touching `Projection`, returning `None` rather than
  propagating a `ProjectionError` panic path — the same validate-before-math
  discipline `fe-renderer::camera::{orbit_from_look_at, pose_transform}`
  apply on the bevy side.

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
   `Arc<TilesetRegistry>` inserted by the main binary). Each URI resolves to
   the registry's OWN `Arc<HexonTileSource>` (`registry.source(uri)`); on a
   miss the tileset is indexed with `store().load_tileset` and registered with
   `registry.insert_source`, so the renderer and the fe-api tile routes share
   one source and one byte cache (§lazy-tiles). Failures warn and continue.
   The miss path's header-phase index open is the one file read left in this
   system — once per tileset not already indexed by `load_all`/`install`, never
   per frame and never a tile read.
   Before the rebuild the outgoing composite's pins are released
   (`release_all_pins`) because every chunk is about to despawn. Then the
   config's
   `tile_source_mode`, a `DiskTileCache` rooted at `config.cache_dir`
   (512 MB budget; skipped with a warn when the dir is empty or not a safe
   relative path — S8b: petal terrain replicates from peers, and the disk
   cache deletes `.tile`/`.hash` files it cannot verify, so `safe_cache_dir`
   only admits relative paths of `is_safe_path_id` components), and
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

   **Category layers (VIS-PH5).** A `gpx_track` / `geojson_overlay` layer with
   an EMPTY source is a CATEGORY layer: it governs every ribbon / overlay of that
   kind that has no per-source layer of its own. The binding is the pure
   `layers::gpx_layer_for(stack, node_id)` / `geojson_layer_for(stack, path)`
   (per-source first, else the category layer, else unbound = visible), used by
   `render_gpx_tracks` and `render_geojson_overlays`. That is what makes fe-ui's
   Layer Manager "GPX tracks" / "GeoJSON overlays" rows live (fe-ui writes the
   category layer by name, without a source). Two rules keep it honest:
   (a) visibility is applied AT BUILD TIME too (`style.visible && layer.visible`)
   because a terrain re-assignment despawns and rebuilds ribbons AFTER
   `sync_layer_visibility`'s change-gated pass for that stack has run;
   (b) a ribbon its own style hides (`gis.track.visible = false`) carries
   `StyleHidden`, and `sync_layer_visibility` never un-hides it — a layer toggle
   can only hide more, never override the per-track style.
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
- `origin.origin_ele == 0.0` is treated as "unset" and grounded from the
  mean elevation of the origin tile at min zoom (`mean_tile_elevation`),
  because mesh world-Y = absolute elevation − origin_ele; real terrain (e.g.
  Pacific NW at 500–1500 m ASL) would otherwise float far above the camera.
  **Grounding is deferred (S5):** tile bytes are no longer in RAM at
  assignment time, and reading one here would be main-thread I/O (NFR-2). So
  the assignment only names the center tile (`TileResidency::reset(revision,
  grounding)`); `fetch_and_spawn_terrain_chunks` loads it off-thread, spawns
  NO chunk until it resolves (pending), then writes `origin_ele` into both
  `ActivePetalTerrain.config` and `ActiveTileSource.projection` before the
  first chunk anchors. An absent/undecodable center tile ends grounding with
  the origin left at 0 (the pre-S5 behavior for a missing tile).

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

**Publisher pin (RP-4b).** `TerrainConfig::tileset_publisher_dids` maps a
bound tileset id to the Ed25519 `did:key` its streamed header must be signed
by — the petal-authored link of the host's pin chain (installed >
petal-authored > TOFU; `fractalengine/src/AGENTS.md` §tileset-p2p "Publisher
pin chain"). `validate_publisher_pins` refuses a pin for an unbound tileset
or one without the `did:key:z6Mk…` grammar, and
`terrain_config_from_petal_json` returns `None` for such a config (strictest
reader — never a silent drop). fe-terrain has no fe-identity dependency, so
this is grammar only; the DB and API writers also decode the key.

## §terrain_plugin

Bevy plugin for terrain chunks, GPX tracks, waypoints, GeoJSON overlays, and
layer visibility. System chain order matters: `apply_terrain_assignments`
runs first so the frame's assignment is visible to everything downstream.

`fetch_and_spawn_terrain_chunks` is `pub` (and `CompositeTileSource` exposes
`tile_source_mode()`) only so the main binary can order its global
Offline-mode override between `apply_terrain_assignments` and the first fetch
(fractalengine `src/AGENTS.md` §tile-mode).

**Offline-first chunk pipeline** (`fetch_and_spawn_terrain_chunks`):
- Early-outs: no enabled config, no composite/projection, no streaming focus
  (a `StreamingFocus` camera, or the lone-camera fallback), or (with no capture focus) at `max_chunks`. Every step below runs
  once per streaming focus — see "Multi-focus streaming" below.
- Camera local position → `local_to_wgs84` → desired zoom via
  `desired_zoom(cam_height, min_zoom, max_zoom)` (one zoom step out per
  doubling of height above a 200 m base; monotonic + clamped).
- 3×3 tile ring around `TileCoord::from_lat_lon(lat, lon, zoom)`; tiles
  already spawned (by `(zoom, x, y)`) or previously failed are skipped.
- Per tile: `TileResidency::ensure_resident(coord)` (§tile-residency)
  returns `Ready(LoadedTile)`, `Pending` or `Absent`. Pending tiles are
  skipped this frame and NOT recorded anywhere — a load in flight is never a
  failure. Ready tiles build the chunk from the `LoadedTile` bytes: elevation
  PNG decoded by the config's `ElevationSourceKind` decoder → `terrain_mesh`;
  satellite (hexon → disk cache namespace `composite_sat`) decoded to an
  RGBA texture. No elevation but satellite → flat 16×16 grid. Only `Absent`
  (in no covering index nor the disk cache) or an undecodable tile lands in
  `FailedTiles` (a `HashSet<(u8,u32,u32)>` cleared on revision change), so a
  missing tile warns once instead of every frame. A spawned chunk pins its
  tile bytes (`composite.pin_tile`); `update_terrain_lod` releases them when
  it despawns the chunk (exit ring).
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
visibility plus optional zones. `TrackStyle { color, width, visible, closed,
fill_color }` (plain struct, no
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
`fractalengine/src/AGENTS.md`). Closed tracks add a closing border segment and
earcut-triangulate their alpha fill into the same mesh entity; invalid polygons
keep the border and omit only the fill. Zone input is finite-filtered,
consecutive-deduplicated, and whole-ring sampled at 2,048 shared border/fill
points before simplicity validation and earcut; it is never prefix-truncated.
`prepare_track_zone` is the shared render/pick geometry authority. Defaults
(`closed=false`, transparent fill) preserve legacy open tracks.
Small and narrow rings below the cap remain untouched. The quadratic simplicity
check runs only on the final bounded polygon, whose fill must retain three
unique points and meaningful area relative to the original ring.
- `sync_layer_visibility` ran every frame; now gated on
  `layer_stack.is_changed()`, and opacity < 1.0 also sets
  `AlphaMode::Blend` (alpha alone doesn't blend on `StandardMaterial`).

**§ribbon-centroid-anchor (path_interaction_20260716, FR-4/FR-5).** Two changes
to the ribbon:

- **Default width 0.5 wu** (was 2.0). `TrackStyle::default().width` — the old
  2.0 read as a fat band that over-covered nearby objects. fe-ui's
  `TrackStyleFields` default + the Paths-tab slider floor (0.1) mirror this.
- **Centroid-anchored geometry.** `render_gpx_tracks` now computes
  `mesh::track::track_centroid(&positions)` (arithmetic mean of the SAME
  finite-filtered positions the mesh is built from), builds the ribbon with
  positions RELATIVE to that centroid, and spawns the entity
  `Transform::from_translation(centroid)`. Net world position is unchanged, but
  the whole-path gimbal (fe-ui) now has a real transform to grab (Move/Rotate/
  Scale pivot about the path center). The fractalengine bridge attaches a
  `fe_ui::node_manager::TrackPickShape` carrying the SAME `track_centroid`
  result, so the gimbal commit bakes its delta about the exact baseline the mesh
  renders at. `track_centroid` is a pure, unit-tested helper (`[0,0,0]` for an
  empty list); callers MUST pass the identical filtered position list the mesh
  uses or the bake drifts. IoT animation reads `TrackRouteMap` (world-space route
  points), NOT the mesh vertices, so it's unaffected by the re-anchor.

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

### Multi-focus streaming (agent_vision_20261031 V4, spec §5.4 / DD-7)

**Why.** Terrain used to load around `camera_query.single()` over every
`Camera`. An agent-vision scene capture spawns a second camera, so `.single()`
failed and BOTH `fetch_and_spawn_terrain_chunks` and `update_terrain_lod`
returned early — all streaming and eviction stopped silently. Even with that
fixed, nothing would ring a remote capture pose and the main camera's exit
ring would evict anything that did load there.

**Rule.** Streaming follows cameras carrying fe-renderer's `StreamingFocus`
marker (`StreamingFocusQuery`), not "the" camera. The orbit camera always
carries one; a `CaptureCamera` carries one for its job's lifetime. A camera
without the marker (a glb scene camera, a capture without one) streams nothing.
**Lone-camera fallback:** when NO entity carries `StreamingFocus` and exactly
one `Camera` exists, that camera is the main focus — a host that never added
fe-renderer's orbit camera (headless flows such as
`tests/lazy_tiles_test.rs`, a third-party host) keeps its pre-V4 streaming
instead of silently losing it. Two plain cameras are ambiguous and stream
nothing (the old `.single()` outcome). In the GUI the orbit camera always
carries the marker, so the fallback never engages there.
`CaptureFocusResidency` is an `Option<ResMut>` system param for the same
reason (`TerrainPlugin` always inits it).
- **Foci set.** `StreamingFoci::collect` keeps at most `MAX_STREAMING_FOCI`
  (2) in a fixed array, so there is no per-frame `Vec`: slot 0 = the first
  capture focus, slot 1 = the first non-capture (main) focus. Extra foci are
  ignored; capture jobs are single-flight (fractalengine §vision).
- **Fetch.** Foci stream in `fetch_order` — the user's view FIRST, then the
  capture — each with its own lat/lon, height-derived zoom, adaptive ring and
  upsample. Grounding still happens once (the centre-tile grounding is
  camera-free). The per-frame spawn cap (`MAX_SPAWNS_PER_FRAME`) is shared, so
  the view going first means a capture only ever gets the spawns the view did
  not need that frame. (`reaches` keeps slot order, capture = index 0.)
- **Budget (separate, critic pass 2026-11-01).** Each focus kind counts only
  the chunks it holds: a chunk belongs to the capture iff its `spawn_chunk`
  anchor (`chunk_anchor_world`) is nearest the capture focus
  (`nearest_focus_index == 0`), the rest belong to the view. The view may hold
  `max_chunks`; the capture may hold `capture_chunk_allowance(max_chunks)`
  (half the budget) and its ring radius is sized to that allowance. The total
  stays bounded by `max_chunks + allowance`, and a capture can never pause or
  shrink the user's ring fill (`a_capture_never_spends_the_user_views_chunk_budget`).
  Before the fix the capture streamed first against a shared total, so an agent
  looping scene captures could stall the user's view while it moved. Eviction
  is distance-only, so a capture can never evict the user's view either. The
  attribution pass runs only while a capture exists (one projection per chunk);
  with one focus the pass is the pre-V4 pipeline.
- **LOD.** `update_terrain_lod` despawns a chunk iff it is beyond EVERY
  focus's own hysteresis despawn radius (`chunk_retained_by_any_focus`), or it
  is at a stale zoom for its NEAREST focus and that focus's replacement fully
  covers it (`stale_for_nearest_focus`, the hole-free rule generalised).
  Removing the capture camera removes its focus, so the next LOD pass releases
  its chunks through the existing exit-ring eviction (no new eviction path).
- **No churn.** With two foci the fetch ring skips any tile that
  `stale_for_nearest_focus` would evict next frame, measured from the same
  SW-corner anchor `spawn_chunk` gives the chunk (`chunk_anchor_world`, cast
  to `f32` exactly like the `Transform`). Spawn and despawn therefore apply
  one predicate to identical inputs and cannot fight. With one focus the check
  is skipped and the pass is the pre-V4 pipeline: same ring, same order, same
  early-outs, same allocations.
- **`CaptureFocusResidency { capture_focus, pending }`** is rewritten every
  frame. `capture_focus` names the capture camera the count describes, so the
  V5 settle can ignore a count from before its camera existed. `pending`
  counts that capture's ring tiles that are still outstanding: an async load
  in flight, a streamed bundle not yet arrived (the bundle is still demanded),
  the origin-grounding tile (reported as 1), or a tile deferred by the
  chunk/spawn budget. Tiles already spawned, failed, outside the world, or
  ceded to the other focus are not pending. No enabled config or no tile
  source → 0: there is nothing to wait for.

**Narrowed single-camera sites.** Systems that mean "the user's camera" now
query `With<OrbitCameraController>`: `ruler_plugin.rs::draw_scale_bar` (and
`draw_world_grid`, which already read the controller),
`splat/render.rs::apply_view_mode_visibility` (Hybrid measures from the user's
camera), and fe-renderer's `grid.rs::draw_grid` and
`axis_gizmo.rs::draw_axis_gizmo`. Known benign site:
`fe-webview/src/petal_portal.rs::position_overlay_system` still calls
`.single()` over every `Camera`, but its body is a TODO no-op, so a second
camera only skips nothing.

**Tests.** `lod_ring` tables (`chunk_retained_by_any_focus_table`,
`nearest_focus_decides_stale_zoom_table`) plus flow tests in
`terrain_plugin.rs` `tests::multi_focus` that drive the real LOD → fetch chain
over an installed in-memory hexon (main area) and a P2P-streamed tileset
(capture area).

### Height-field feed (ux_interaction_hardening_20260718 FR-5)

`spawn_chunk` also publishes each tile's final scaled elevation grid —
bilinear-downsampled to ≤`MAX_HEIGHT_GRID_AXIS` (65) per side via
`resample_height_grid` (~16 KB/tile vs up to ~4.4 MB for the full mesh grid) —
into `fe_renderer::terrain_height::TerrainHeightField`, keyed like
`TerrainChunk.tile_coords`. The camera ground clamp in fe-renderer reads it.
Lifecycle is lockstep with chunk entities: `update_terrain_lod` removes the
entry on despawn, `apply_terrain_assignments` clears the field on
petal/assignment switch. The resource type deliberately lives in fe-renderer
(fe-ui ↛ fe-terrain boundary; same channel pattern as `CameraScaleSettings`)
and all fe-terrain systems take it as `Option<ResMut>` so headless tests run
without it. Rationale for the design (and rejected alternatives) in
fe-renderer `src/AGENTS.md` §terrain-height.

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

## §ruler

`ruler.rs` is the Bevy-free measurement-math home (hexon_scale_orchestration
FR-7/FR-8, C4: lives next to `scale.rs`, no new crate). All ground-plane math is
XZ-planar (Y ignored): `nice_number` snaps spans to 1/2/5×10ⁿ so scale bars and
grid spacings read cleanly; `world_to_real_distance`/`bearing_deg` (compass,
north = −Z, CW) / `polygon_area_m2` (shoelace) divide by the sanitized
`world_scale` so results are real meters regardless of render scale.

Scale-bar selection (FR-8) is also pure here so the egui pass stays thin and the
logic stays headless-testable: `meters_per_pixel` (visible ground span
`2·h·tan(fov/2)` over the viewport height — a down-looking approximation, good
enough for a HUD scale bar), `scale_bar_spec` (nice-number length nearest a
target pixel width), `format_distance_m` (m below 1 km, km above, one decimal
only when it carries signal). Degenerate inputs return `0.0`/`None`/`"0 m"`
rather than NaN so the HUD silently hides instead of drawing garbage.

Adaptive world grid (FR-10) is pure here too. `grid_spacing_for_height(h,
target)` takes the visible span as `2·h` (the 90° down-looking approximation,
no FOV input), snaps `span / target` with `nice_number` to the MAJOR spacing,
and takes the ladder step below it (`next_nice_down`: 1→0.5, 2→1, 5→2) as the
MINOR spacing. `minor_alpha` is the log-space position of the raw span inside
the band that `nice_number` maps onto that major (`nice_band`: [0.75, 1.5),
[1.5, 3.5), [3.5, 7.5) × 10ⁿ), inverted: minors are fully shown just after a
re-subdivision and fade to about 0 just before the next one, so zooming never
pops a whole tier in or out. `grid_fade_alpha(h, start, end)` fades the WHOLE
grid by camera height (1 below start, 0 above end, linear between, 0 on
non-finite input). Majors are monotone non-decreasing in height (asserted by a
sweep test).

## §ruler_plugin

`ruler_plugin.rs` (render-gated) draws the FR-8 scale bar as a bevy_egui 0.39
foreground layer-painter overlay in `EguiPrimaryContextPass` — same pattern as
fe-ui's viewport labels; no in-world meshes. It reads the *same* metric frame as
zoom selection (`ActivePetalTerrain.config.effective_world_scale()` +
`world_to_real_height`) so the bar always agrees with what the terrain systems
believe. `RulerSettings.show_scale_bar` (default on) is the reflection-free
toggle seam for fe-ui (C6: bounds/toggles reach UI via the API surface, not a
fe-terrain dep). Registered from `TerrainPlugin::build`, not `main.rs`. If
`EguiPlugin` is absent (headless), the pass never runs — the plugin is inert.

`draw_world_grid` (Update, default `Gizmos` group, depth-tested) draws the
FR-10 metric grid when `RulerSettings.show_grid` is on (default OFF — opt-in
layer, ui_ux §7). It needs an enabled terrain config (no map means no meters,
so no grid; the scale bar's "world units" chip already covers that state).
Spacing is `grid_spacing_for_height` on the real camera height
(`world_to_real_height` with the effective world scale), converted meters to
world units by the same scale. Lines lie on the y = 0 plane around the orbit
focus, snapped to the spacing so they do not swim while panning;
terrain-following is out of scope (they hide under raised terrain, like
fe-renderer's world-unit grid). Each tier draws at most 2·64 lines
(`GRID_HALF_LINES` each side per axis); minor lines that coincide with a major
line are skipped. The whole grid fades out between `GRID_FADE_START_M` (50 km)
and `GRID_FADE_END_M` (200 km) of camera height, where a planar metric grid
stops being useful. The toggles arrive from fe-ui's `AppSettings` via the main
binary's `terrain_bridge::sync_ruler_settings`.

## §ruler-hud-input

The host publishes `RulerHudInput` only after the actual shell has completed its
same-pass geometry. `RulerHudSet::Bridge` precedes `Draw`; the bridge consumes the
UI's current context/pass/pixels-per-point stamp and shared divider-safe overlay
rectangle. The terrain resource checks that same identity and pass again at use.
An absent, stale, invalid or out-of-viewport rectangle omits this cosmetic HUD;
neither producer falls back to whole-window geometry. Both the numeric bar and
fixed unscaled chip clip to that scene, inset their complete text mesh/layout and
stroke/chip bounds, and omit output when those bounds cannot fit.

Placement and projection calibration are distinct. The bar reads the unique
orbit Camera which owns the actual primary EguiContext. Only an active perspective
Camera supplies a metric bar. Its `physical_viewport_size` supplies the projection
height in physical pixels, including a real custom Camera viewport if one exists.
The preferred 120-point visual length is multiplied by current egui
pixels-per-point before nice-number selection; the physical result is divided by
that same value before painting. A narrow scene bounds the selected length but
does not change meters per physical pixel. Side-panel changes do not crop the
camera. Effective terrain world scale and the existing down-looking perspective
approximation remain unchanged; oblique ground-distance accuracy is not claimed.

This remains the separately audited fixed/numeric Foreground `ruler_scale_bar`
direct painter. It is not a new mandatory native surface producer, capture grant
or Areas registration. The existing end-to-end ruler journey includes actual
production calibration and paint functions under the render feature, across UI
scales, stale context/pass, panel-width changes and insufficient room.

QA41 L1 adds the actual same-pass bounded chrome exclusions to that placement
input. Both complete footprints start at the lower-left inset and move above
intersecting actual chrome with a four-point gap, or omit when no complete
supported placement fits. The search is bounded by the four owned exclusions;
it never changes camera calibration or paints beneath a hint. The host's existing
ruler journey now renders the real viewport header/long Select hint and actual
numeric/fixed painter in the same pass at ordinary/narrow scales with Results.

Standalone TerrainPlugin integrators must publish their own same-pass owned scene
anchor and actual occupied chrome in RulerHudSet::Bridge. Without that publisher,
both HUD variants intentionally remain absent even when show_scale_bar is true;
there is no whole-window fallback. They must also order their actual outer egui
pass after current camera computation and transform propagation, as the GUI host
does through install_ruler_hud. These are cosmetic integration contracts, not
capture authority or numerical ground-distance guarantees.

## §tile-budget — per-tile mesh + overlay feature caps (oom_guardrails_20260717)

Chunk COUNT is bounded (max_chunks=256, ring clamp, MAX_SPAWNS_PER_FRAME) but
per-tile mesh SIZE was not: `decode_png_pixels` has no dimension guard, and
close-range upsampling makes vertices `((w-1)·U+1)²` — a 512px tile at U=4 is
~4.2M verts (~234 MB), and a degenerate 4096px PNG would be ~268M verts
(host OOM from a single tile). Guards in `terrain_plugin.rs`:
- `MAX_TILE_DIMENSION = 1024`: larger decoded elevation tiles are rejected
  with a `warn!` (satellite-texture fallback still renders).
- `clamp_upsample_for_grid` lowers the upsample factor until the densified
  grid fits `MAX_TILE_VERTICES = 1,100,000` (256px keeps U=4; 512px clamps to
  U=2; 1024px to U=1; U floors at 1). The dimension cap guarantees U=1 always
  fits.
- `MAX_GEOJSON_FEATURES = 50,000` per overlay file — one shared budget across
  polygons/polylines/markers (the only otherwise-unbounded spawner in this
  crate), plus a single shared marker sphere mesh per overlay instead of one
  mesh asset per marker. Saturation logs a `warn!` with the true count.

## §terrain-proposals — analytics-first NON-destructive terrain edits (terrain_editor_overhaul_20260718 FR-5/FR-6, NFR-1)

The analytics contract (NFR-1, **sacred**): a terrain edit is a PROPOSED
OVERLAY, never a write to the true terrain. `fe_renderer::terrain_height::
TerrainHeightField` and the loaded tileset are the immutable ground truth so a
report can compare proposed-vs-true honestly. Every pure fn here takes the base
as `&impl Fn(f32,f32)->Option<f32>` — structurally read-only, so the type system
enforces the contract; `terrain_proposal.rs` tests assert byte-identity of the
base before/after apply (one closure-backed, one against a real
`TerrainHeightField` under the `render` feature) as the NFR-1 proof.

`terrain_proposal.rs` is **Bevy-free and always compiled** (like `ruler.rs`/
`scale.rs`) so the record model + geometry are headless-testable and the fe-ui
side can mirror them via JSON:

- `TerrainProposal { id, op, footprint: Vec<[f32;2]>, target_height: Option, delta: Option }`
  + `TerrainOp { Raise, Lower, Flatten, Ramp, Slope, Pad, Cut, Fill }` (serde
  snake_case — the persisted `terrain.proposals` contract). Footprint is an
  XZ-world polygon (need not be closed).
- Per-op proposed-height semantics (`proposed_height_at`, private): Raise/Lower
  `base ± delta` (relative → `None` if no base). Flatten `target` (absolute).
  Pad `max(base,target)` (fill-only platform). Cut `min(base,target)` or
  `base−delta` (remove-only). Fill `max(base,target)` or `base+delta` (add-only).
  Ramp `lerp(base,target)` across the footprint X extent (or `base+delta·frac`).
  Slope constant grade `base+delta·run` (rise per world-unit +X run).
- `apply_proposal_over_base(base, proposal, sample_pts) -> Vec<Option<f32>>`:
  per-point proposed heights; points outside the footprint gate to `None`
  (even-odd `point_in_polygon`).
- `proposal_overlay_mesh_data(base, proposal) -> Option<(positions, indices)>`:
  earcut-triangulated footprint lifted to proposed heights (relative-op-no-base
  falls back to `target` then `0.0` so intent still renders). `None` for <3
  verts / triangulation failure.
- `parse_proposals(&Value)` / `to_json(&[..])`: array round-trip; invalid
  records are dropped with a `warn!`, never fail the set (additive resilience).
- `proposal_report(proposal, world_scale) -> ProposalReport { extent_m, area_m2,
  volume_m3, slope_pct, bearing_deg, scaled }` (FR-6): reuses the tested `ruler`
  math (`polygon_area_m2`, `bearing_deg`). `scaled == false` when `world_scale`
  is non-finite/≤0 → values are WORLD UNITS, never fabricated meters (NFR-4).
  `slope_pct` is scale-invariant (world-unit rise/run). `volume_m3 = area × |delta|`
  is a representative cut/fill (true cut/fill needs base; fe-ui can pass a
  base-derived delta).

Persistence + rendering (render-gated):

- Proposals live on `TerrainConfig.proposals` (serde default `[]`,
  `skip_serializing_if empty` — additive, pre-feature petal JSON omits the key),
  so they round-trip through the existing `SetPetalTerrain` →
  `TerrainAssignmentMsg` path with no new config surface (spec: petal-config is
  the lower-risk v1 vs a hexon proposal-layer).
- `render_terrain_proposals` (terrain_plugin.rs) is the SOLE despawner/spawner of
  `ProposalOverlayEntity` ghosts — it is NOT in `apply_terrain_assignments`'
  despawn query (avoids a double despawn). `proposals_need_rebuild` rebuilds the
  whole set on exactly three triggers, otherwise the ghosts are left alone
  (no per-frame churn):
  1. `ActivePetalTerrain.revision` changed (a proposal edit bumps it via the
     round-trip).
  2. **Re-ground**: a relative-op ghost built before terrain covered its
     footprint sits on the `target`/`0.0` fallback. `ProposalRenderState`
     records `ungrounded` + the grounded-vertex count of the built set
     (`proposal_grounded_vertices`: vertices with a base sample, or all of them
     for an absolute Flatten/Level with a target). Only while `ungrounded` AND
     `TerrainHeightField::is_changed()` (deref'd mutably only on chunk
     insert/remove/clear) does it re-sample, and it rebuilds only when the
     grounded count GREW — so chunks streaming in elsewhere, or evictions, never
     churn a ghost. Mirrors the per-chunk `EarthworkBaked` late-chunk pattern.
  3. **Residency** (NFR-2): `ProposalOverlayBudget.allowance` is the headroom
     for NEW ghosts, written by the binary
     (`fractalengine/src/terrain_bridge.rs::sync_proposal_overlay_budget`, from
     fe-ui's `MeshInstanceBudget`: 0 once the watchdog trips, else
     `ceiling − last_count`). A rebuild may hold
     `proposal_overlay_capacity = min(allowance + resident ghosts,
     MAX_PROPOSAL_OVERLAYS)` because it despawns the residents first; a build
     that stops early sets `truncated`, and a truncated set rebuilds when the
     capacity exceeds what it holds. At the ceiling an edit re-spends the
     residents' slots but never spawns past them. Default allowance (no bridge)
     = `MAX_PROPOSAL_OVERLAYS` (4,096, the hard per-petal cap). T3: records that ARE earthwork regions (`is_earthwork_region`,
  `material` present) are skipped here — they bake instead (§sculpt). Each ghost is a `ProposalGhost` mesh with a per-op tinted, blended,
  unlit, double-sided material from `fe_renderer::terrain_overlay`, tagged to the
  `MapLayer::ProposalOverlay` layer.
- `MapLayer::ProposalOverlay` is parallel to satellite/terrain/gpx_track/
  geojson_overlay (`layers/stack.rs`, config name `"proposal_overlay"`);
  `apply_terrain_assignments` auto-adds one (topmost z) when a petal has
  proposals but declares no such layer, so the visibility toggle works
  out-of-the-box. Ghosts with a bound layer honor `sync_layer_visibility`.
- NFR-2: ghosts are bounded by the live residency budget (trigger 3 above);
  `MAX_PROPOSAL_OVERLAYS = 4,096` remains the hard per-petal cap.

## §sculpt — earthwork region engine (sculpt_earthwork_regions_20260725, D-A8)

`sculpt/mod.rs` is the BIM terraforming engine: area-selection footprints,
sculpt operations, the addressable modification-region record, and cut/fill
**volume in real units**. Bevy-free + always compiled (like `terrain_proposal`/
`ruler`/`scale`) so it is headless-testable and the fe-ui side mirrors the region
via JSON (fe-ui must NOT depend on fe-terrain). It **evolves the proposal path,
does not fork it** (FR-6): a region is persisted as an enriched record in the
same `terrain.proposals` block (adds a `material` tag).

**NFR-1 (sacred, shared with proposals):** every fn takes the base surface as a
read-only `&impl Fn(f32,f32)->Option<f32>`; the true `TerrainHeightField` is
never written. The `nfr1_base_sampler_unmutated_after_apply_and_volume` test is
the byte-identity proof. The "bake" is the non-destructive overlay recomputed
from the record set, so **delete reverts by dropping the record** (Q-2 ratified —
the region is the source of truth; the surface was never destructively edited).

- `Footprint { Brush, Circle, Rect, Polygon }` in petal-local scene units (N-1:
  raw geometry; `world_scale` enters only at the real-unit volume seam). `Brush`
  and `Circle` are the same disc, kept distinct so the UI labels the tactile vs
  the reportable origin (D-A8 "an actual shape you can report on"). `contains`
  uses an exact radial test for discs / rect bounds / even-odd for polygons;
  `to_polygon` tessellates discs (`DISC_SEGMENTS`); `bounds` is the AABB.
- `SculptOp { Raise, Lower, Level, Smooth }` (FR-2). `proposed_height`: Raise/
  Lower `base ± delta` (relative → `None` w/o base), Level `target` (absolute),
  Smooth relaxes toward `region_mean` weighted by `strength` (planning-grade).
  `from_snake` also accepts the terrain-proposal vocabulary (`flatten`==`level`,
  `cut`→lower, `fill`/`pad`→raise, `ramp`/`slope`→level) so an evolved proposal
  is a well-formed region without a fork.
- `cut_fill_volume(base, proposed, footprint, grid_step, world_scale) -> CutFill`
  (FR-4, Q-4 planning-grade): heightfield prism/grid integration. Each
  `grid_step` cell centre inside the footprint with both samples contributes
  `|dh_m| × cell_area_m²` to fill (`dh>0`) or **cut** (`dh<0`), where
  `dh_m = (proposed−base)/scale` and `cell_area_m² = grid_step²/scale²` — the
  SAME `/scale`, `/scale²` real-unit convention as `proposal_report` (evolve).
  **Documented tolerance:** O(grid_step) discretisation — converges as
  `grid_step→0`; use a step ≤ ¼ of the smallest relief feature. `scaled==false`
  when `world_scale` is unusable → figures are scene-unit³, never fake meters
  (NFR-4). Analytic tests: flat-fill (200 000 m³ @ 0.1 scale) and a ramp levelled
  to its midline (cut≈fill, within 5% of the ∫|target−x| analytic).
- `EarthworkRegion { id, op, footprint, material, target_height, delta }` (FR-3,
  D-A8): the persistent, addressable region record; serde matches the enriched
  `terrain.proposals` JSON (so fe-ui round-trips it via `SetPetalTerrain`).
  `material` defaults to `"earth"` (single-material this landing, Q-3; per-layer
  strata is the deferred phase). `parse_regions` drops malformed records with a
  `warn!` (additive resilience, like `parse_proposals`).
- `terrain_proposal::proposal_cut_fill(base, proposal, grid_step, world_scale)`
  bridges an existing proposal to separated cut/fill by reusing the proposal's
  own `apply_proposal_over_base` as the target surface — the FR-4/FR-6 evolution
  of the report for every op, no op-semantics duplication.
- `CompositeRegion` + `prepare_composite(regions, base)` +
  `composite_proposed_height(regions, base_y, x, z) -> f32` (delta_viz_20261025
  S1, FR-5): the ONE multi-region composite-surface walk, lifted out of what
  was `terrain_plugin`'s private `PreparedRegion`/`earthwork_surface_delta`
  pair (deleted; the bake, the baked-region heatmap AND `volume_report` all
  call this). Each region's Smooth mean is sampled at that region's OWN
  `region_grid_step` (`EARTHWORK_LATTICE` = 128, `lattice_grid_step` — both
  live here now, re-used by `terrain_plugin`), exactly what
  `EarthworkRegion::cut_fill` (the volume persisted on the region node) uses.
  S7 review fix: a set-wide union-bounds step made a Smooth region's baked
  relief shift whenever an unrelated far-away region was added, and drifted
  from its persisted volume (`composite_smooth_mean_matches_standalone_and_
  ignores_unrelated_regions`).
  Applies every containing region in RECORD ORDER, each seeing the prior
  regions' result as its own base, and returns the ABSOLUTE proposed height
  (not a delta — `earthwork_surface_delta` returned `h - base_y`;
  `composite_proposed_height` returns `h`, matching every other
  `*_height`/`*_proposed_height` fn in this module). `for_each_cell` is
  `pub(crate)` (was private) so other pure modules share the one lattice walk
  instead of re-deriving it. See `delta_viz.rs` §delta-viz below for the
  consumer.

The brush cursor overlay is `fe_renderer::terrain_overlay::{brush_ring,
brush_overlay_positions, BRUSH_OVERLAY_RGBA}` (folded into the existing overlay
module — no new fe-renderer module; T2 owns that `lib.rs` line; the `BrushOverlay`
entity marker was removed once fe-ui consumed the ring via immediate-mode
`Gizmos`). It grounds the ring READ-ONLY on `TerrainHeightField` (NFR-1). The
fe-ui sculpt UI (`panels/terrain_tools_panel.rs::render_brush_controls`)
configures meter-valued region params. The viewport snapshots them into
petal-local world units and queues one batched stroke action on release.
`process_ui_actions` dispatches `UiAction::SculptBrushStroke` to one terrain
document write and one endpoint-node creation for the stroke's single
swept-corridor polygon (see
`fe-ui/src/actions/AGENTS.md` §sculpt).

### Earthwork bake (T3 integration, terrain_plugin.rs)

Regions in `terrain.proposals` bake into resident chunk meshes; plain proposals
keep ghosting. `TerrainProposal::is_earthwork_region` requires non-empty
identity/material, finite nonzero-area geometry, sculpt
vocabulary, and valid op parameters; malformed records remain ghosts.
`render_terrain_proposals` skips valid regions and
`bake_earthwork_regions` handles only them. `TerrainOp` gained `Level`/`Smooth`
so a region-holding proposals block still parses as `TerrainConfig` (a Level
region must never invalidate the whole petal terrain JSON); at the proposal
seam Level == Flatten and Smooth is identity (real math lives in `sculpt`).

- **Pristine retention:** the first bake of a chunk snapshots its local vertex
  Y into a `PristineChunkHeights` component; every re-bake (per-chunk
  `EarthworkBaked{revision}` marker vs `ActivePetalTerrain.revision`, the same
  gate as the ghosts — plus late-arriving chunks, which the ghost path still
  lacks) restores the snapshot first, so edits/deletes revert cleanly (Q-2).
- **Bake = pristine + surface delta:** per vertex, `dh(x,z)` is the sequential
  `sculpt::proposed_height` of every containing region over the READ-ONLY
  `TerrainHeightField` sample using `EarthworkRegion::effective_strength`;
  the resulting delta is added to the vertex's pristine Y — mesh detail survives, and
  skirt walls keep their depth (top and bottom get the same `dh`). Normals are
  recomputed with the `terrain_mesh` accumulation.
- **The height field stays PRE-BAKE by design:** it is the pristine base for
  deltas, volumes, ghost grounding, and camera clamp; writing baked heights
  into it would corrupt the Q-2 revert and the cut/fill base.
- **Volumes:** each region uses the shared integration lattice with expected and
  finite sample counts. Reports publish complete finite pairs or unavailable,
  with exact assignment/height/region source identity, on revision or height
  generation change. See Complete earthwork measurements below.

## §delta-viz — relief-following delta lattice (terrain_proposal_delta_viz_20261025 S1, FR-1/FR-2/FR-5)

`delta_viz.rs` is the pure geometry/volume layer behind the in-app finding F5
("there is no visible transformation"). It is Bevy-free + always compiled
(same discipline as `terrain_proposal.rs`/`sculpt`), generic over any
base/proposed sampler pair — it does not know about proposals, regions or the
palette; `S2` (render integration) is the one that wires a `TerrainProposal`'s
`apply_proposal_over_base` or a region set's `sculpt::composite_proposed_height`
in as its `proposed` closure.

**Why a lattice, not a lid (R-1).** The old `proposal_overlay_mesh_data`
earcut-triangulated only the footprint PERIMETER, so the ghost was a flat
polygon that sliced through relief — a Raise over a hill looked identical to
a flat tinted decal. `DeltaLattice::build` instead samples a regular
`grid_step` lattice of INTERIOR points (not just the footprint's own
vertices) and triangulates only the quads whose all 4 corners are inside the
footprint and covered by both samplers. A non-rectangular footprint's
boundary quads are simply left untriangulated (a hole at the true edge),
never papered over with a flat lid — the mesh is honest about where it does
and doesn't have relief-following coverage.

- `DeltaLattice { ghost_positions, drape_positions, dh, indices, cut_fill }`:
  `ghost_positions[i] = [x, proposed(x,z), z]` (the FR-1 ghost — Y follows
  the PROPOSED surface, not a flat plane); `drape_positions[i] =
  [x, max(base,proposed)+lift, z]` (the FR-2 heatmap drape, lifted a fixed
  `DRAPE_LIFT` world units clear of both surfaces so nothing z-fights);
  `dh[i] = proposed(x,z) - base(x,z)` in SCENE units (fill > 0, cut < 0 — the
  same sign convention `cut_fill_volume` uses). `indices` is ONE triangle
  buffer shared by both position arrays (same topology, different Y — so
  ghost and drape are always built from identical quads). `cut_fill` is a
  literal `sculpt::cut_fill_volume` call on the same footprint/step/
  world_scale — never a second volume implementation (FR-5: a dedicated test,
  `lattice_integrated_volume_equals_cut_fill_volume`, asserts the two are
  bit-identical for the same inputs).
- **Vertex cap (NFR-2).** `DELTA_LATTICE_VERTEX_CAP` (16,384 grid corners)
  bounds one footprint's lattice. `effective_step` coarsens `grid_step` just
  enough to fit (geometric sqrt-area-ratio estimate, re-checked for up to 8
  passes to absorb ceiling-rounding — bounded, never an unbounded loop) and
  `tracing::warn!`s when it does, so a huge/degenerate footprint still
  renders a (coarser) ghost instead of nothing or a frame-budget violation.
  Both the mesh AND the paired `cut_fill` use the SAME effective step, so the
  volume's stated tolerance always matches what actually got sampled.
- **NFR-1.** `base`/`proposed` are `&impl Fn(f32,f32)->Option<f32>` —
  structurally read-only; `nfr1_base_untouched_by_delta_lattice` is the
  byte-identity proof (same pattern as `sculpt`/`terrain_proposal`).
- **S7 review invariants (render side).** Every overlay spawn site inserts
  its initial `Visibility` from the `ProposalOverlay` layer
  (`layer_initial_visibility`): `sync_layer_visibility` only runs on a
  LayerStack change, so a preview/ghost spawned under a hidden layer used to
  show. Baked-region drapes count against `ProposalOverlayBudget` (they used
  to bypass it) and re-ground when covering chunks arrive after the revision
  (startup order left them missing forever). Committed + baked drapes share
  `MAX_HEATMAP_VERTICES_PER_PETAL` via `HeatmapVertexUsage`; hitting it skips
  the drape with one warning and never sets the budget `truncated` flag (that
  flag re-arms a rebuild on any headroom → per-frame rebuild loop). The
  preview snapshots its draft only when the resource/petal changed (idle
  frames allocate nothing) and a `PreviewSource::SelectedProposal` draft is
  readout-only (no duplicate lattice over the committed one).
- **Consumers (S2):**
  `render_terrain_proposals` replaces the perimeter-lid ghost with
  `DeltaLattice::build`'s `ghost_positions`/`indices` for the FR-1 fix;
  `render_baked_region_heatmaps` (new) uses `drape_positions`/`dh` against the
  pristine `TerrainHeightField` for FR-4's baked-region heatmap;
  `render_proposal_preview` (new) is the FR-3 live pre-commit path, same
  builder, debounced (OD-3).

## §volume-report — selected-volume report request/reply (terrain_proposal_delta_viz_20261025 S4+S5, FR-6/7/8)

Two halves, split the same way as §delta-viz's math/render split.

- **Pure math (`volume_report.rs`, S4).** `selection_volume_report(base,
  proposals, regions, selection, step, world_scale) -> SelectionReport` is
  Bevy-free and always compiled (same NFR-1 discipline as `terrain_proposal.rs`/
  `sculpt`/`delta_viz.rs`). It clips BOTH the combined composite surface and
  every individual `proposals`/`regions` entry to `selection`, integrating
  everything through the ONE tested `sculpt::cut_fill_volume` (FR-5 extended to
  the report — never a second volume formula). The composite walk folds
  proposals (array order) over a running height, then hands that height to
  the SAME `prepare_composite`/`composite_proposed_height` the bake uses for
  the regions (S7: it used to re-implement the region walk with Smooth means
  at the selection's step — a third, drifting copy;
  `selection_total_and_items_share_the_bake_composite`). Known gap: palette
  proposals and regions are not interleaved in true record order (proposals
  first); only overlapping non-additive palette+region pairs can differ. Per-item figures are clipped via a reuse trick,
  not a new `Footprint` variant: handing `cut_fill_volume` a `proposed` sampler
  that no-ops outside one item's own footprint makes "item ∩ selection" fall
  out of "selection's own lattice walk" for free. A separate geometric pass (no
  heights) flags `overlap` and each item's `overlap_area_m2` — the practical
  reason a per-item sum can diverge from the composite total.
- **Wiring (`terrain_plugin.rs::answer_volume_report_requests`, S5).** Reads
  `VolumeReportRequest` (fe-renderer seam: correlation id, petal scope,
  footprint), answers with `VolumeReportResult`. Three rules, each load-bearing:
  1. **Petal-scope drop, not queue.** A request whose `petal_id` is not the
     ACTIVE petal is dropped silently (`continue`, no reply). This doubles as
     the staleness guard (`request_for_inactive_petal_is_dropped`): a petal
     switch moves `ActivePetalTerrain` out from under any request still in
     flight for the old one, so "wrong petal" and "stale" are the same check.
     fe-ui must not block forever on a reply that can legitimately never
     arrive (e.g. the user switched petals mid-request) — this is a conscious
     scope cut, not an oversight; a future retry/timeout is fe-ui's job.
  2. **Never double-count a region-shaped proposal.** `config.proposals` is
     split into `proposals` (`!is_earthwork_region()`) and
     `config_earthwork_regions(config)` — the SAME split `bake_earthwork_regions`
     uses — before either is handed to `selection_volume_report`'s two separate
     slices.
  3. **One resolution, not the preview's two.** Uses `region_grid_step`
     (shared [`EARTHWORK_LATTICE`] = 128 "full" policy) unconditionally — a
     one-shot request/reply has no OD-3 debounce to be coarse during.
  - **Seam kind mapping (undocumented at the type itself — read this before
    touching `fe-ui`'s S6 panel).** `fe_renderer::terrain_overlay::VolumeItem`
    has one `kind: String` slot, not `volume_report::VolumeItem`'s separate
    `kind: VolumeItemKind` + `op_snake` + `material`. S5's convention: a plain
    `Proposal` item's `kind` is its op tag (`"raise"`, `"cut"`, …); a baked
    `Region` item's `kind` is the literal `"earthwork_region"` (its op is a
    bake detail, not its reported identity — a region's material isn't
    carried across this seam at all). S6 must match strings, not op enums.

## §bundles — deterministic spatial bundler (`tiles/bundle.rs`, p2p_asset_streaming S1)

`bundle_tileset(source: &HexonArchiveData, max_bundle_bytes)` splits a
tileset into deterministic, Hilbert-ordered, size-capped bundles instead of
flat chunks (D-73 A4) — the unit P2P streaming fetches and the pointer-set
(S7/S8) demands. It is a pure function of the source bytes: identical input
gives identical bundle bytes, hence identical blake3 values, which is the
identity cross-hexon dedup and the tile pointer-set both depend on.

- **Grouping.** Elevation and satellite tiles are grouped by `(z, x, y)`
  first — a pair never splits across bundles — then coordinates are ordered
  by `(z, hilbert(x, y))` (PMTiles-style: `hilbert_distance` is Wikipedia's
  `xy2d`, reimplemented locally to avoid a new dependency) before the
  greedy size-fill, so spatially-adjacent tiles land in the same or
  adjacent bundles. `DEFAULT_BUNDLE_BYTES` is 1 MiB (evidence §3: a real
  elevation+satellite pair is ~110 KB).
- **One split, two callers.** `split_tiles_into_bundles` (grouping + Hilbert
  order + greedy fill) and `bounds_for_tile_range` (a bundle's real lat/lon
  extent from its tile range) are `pub(crate)` and shared with
  `TilesetBuilder::package_chunked` (`builder.rs`, `fetch` feature) — the
  only other place tiles get split into chunks. Before this, `package_chunked`
  stamped every chunk's `chunk_index.chunk_bounds` with the *whole tileset's*
  bounds and called `chrono::Utc::now()` once per chunk inside the loop; both
  now use the shared splitter's real per-chunk range and a single
  build-wide timestamp.
- **Each bundle is a self-contained `.hexon`** via `HexonArchive::export_tileset`,
  manifest `signature` cleared (the source manifest's signature, if any,
  covered different bytes) and timestamps copied from the source manifest —
  never `now()` — so bundling twice yields byte-identical archives.
- **Verification lives at the index, not the archive.** An archive can't
  embed its own hash, so `ChunkIndex.blake3` stays `None` on bundle
  archives; the verifiable digest is `BundleEntry.blake3_hex` in the
  `TilesetBundleIndex` (fe-format `manifest.rs`).
- **`assemble_bundles(store, expected_tileset_id, index, bundle_paths)`**
  delegates to `distribution::assemble_chunks` (which returns the installed
  `InstalledTileset` record, so one assembly implementation serves the legacy
  flat-chunk path too). Any mismatch fails before anything is written.
- **Assembly trust (S8b, mandatory finding #2).** The install is bound to the
  id the user REQUESTED: the index's `tileset_id` and every bundle's
  `ChunkIndex.tileset_id` must equal `expected_tileset_id`, so a provider
  advertising X with an index naming Y can never overwrite an installed Y
  (`fractalengine::terrain_bridge::assemble_and_install` passes the
  requested id). Each file is size-capped (`MAX_ASSEMBLE_BUNDLE_BYTES`, the
  total under the 1 GiB archive budget) and hash-verified against the index
  BEFORE its archive is parsed. Files may arrive in any order (each is
  matched to its entry by hash, positional first); its declared `chunk_seq`
  must equal that entry's seq, and a seq supplied twice is refused.
  `assemble_chunks` also caps the merged tile bytes at 1 GiB (what
  `install_tileset` re-imports) and refuses a `chunk_seq >= total_chunks`.
  P2P installs stay unsigned until PAS-P5 binds the publisher too. Tests:
  `a_peer_index_naming_another_tileset_never_overwrites_it` (fe-terrain) and
  `a_download_naming_another_tileset_never_overwrites_it` (fractalengine).
- **Path-traversal guard moved in from PAS-P5 (critic C1).** `HexonStore::
  install_tileset` (`store.rs`) is the single choke point every install path
  reaches — a fresh download, `assemble_chunks`/`assemble_bundles`, or a
  direct call — so `is_safe_hexon_id` runs there, before any path join,
  refusing with `UNSAFE_HEXON_ID_ERROR`. Since S8b it delegates to the
  shared `fe_format::is_safe_path_id` (fe-runtime `src/AGENTS.md` §safe-id),
  which also refuses Windows device stems and over-long ids;
  `remove_tileset` and `bundle_for_seeding` run the same guard. This has to land here, not in
  PAS-P5: S4 makes P2P-assembled installs — where `hexon_id` is the
  peer-advertised `tileset_id`, untrusted — reachable a full campaign phase
  before PAS-P5's signing lands.
- **One tile→bundle lookup.** `TilesetBundleIndex::bundle_for_tile(z, x, y)`
  (fe-format, pure, over `x_range`/`y_range`/zoom) is the only tile→bundle
  mapping; S7's `StreamedTileSource` and PAS-P5 both call it instead of
  re-implementing a chunk table (critic C4).
- **Seeding cache (`bundle_for_seeding`, S4).** The ONE "bundle an installed
  tileset for `PublishTileset`" implementation, shared by the GUI's
  `terrain_bridge` and the headless relay (`fractalengine/src/AGENTS.md`
  §tileset-p2p). Writes to `<hexon store>/bundles/<id>/{index.json,
  <seq>.bundle, source.blake3}`; a call whose `source.blake3` still matches
  the installed `.hexon` file's current hash reuses the cached bundles
  in-place instead of re-splitting. Touched outside S1's own owned-files list
  (S4 critic step 5 needs cross-binary reuse) — additive only, no existing
  function's behavior changed.
- **Seeding rule (PAS-P5 S5, owner-confirmed R7 — "seeders never re-bundle
  sealed tilesets").** `bundle_for_seeding` opens the installed file with
  `HexonArchiveIndex`. A SEALED container writes `index.json` =
  `HeaderDoc::to_json()` and each signed chunk exactly as the container holds
  it, as `<seq>.bundle`. A sealed MONOLITHIC archive writes the same
  `index.json` plus `0.bundle`, a hard link to the installed file that falls
  back to a copy; the store installs by rename, so the link keeps the bytes
  the header describes. Only a legacy UNSEALED tileset is still re-bundled
  deterministically. The seeding dir is cleared before a rebuild, so files
  from an earlier layout never linger, and cache reuse counts files from
  either child-0 shape. A sealed monolithic archive over
  `MAX_ASSEMBLE_BUNDLE_BYTES` (64 MiB, a peer's per-blob cap) is seeded with
  a warning to re-publish it as a container, because re-bundling would
  break the seal.
- **Sealed download install (`assemble_sealed_download`, S5).**
  `assemble_bundles_from_index_json` dispatches on child 0. A `HeaderDoc`
  must verify under `RequireSealed` and name the requested id. Container
  chunk files must then hash to their SIGNED digests (size-capped and
  read before parse) and are reassembled by S6's
  `assemble_sealed_container`, which keeps the seal. A monolithic file must
  pass `fe_format::verify_archive_against_header`. A legacy
  `TilesetBundleIndex` takes the unchanged `assemble_bundles` path. The
  publisher pin and the confirm-before-chunks gate are S7a's job.

## §lazy-tiles — offset-indexed `HexonTileSource`, one shared `Arc` (p2p_asset_streaming S5)

**Why.** Before S5 every installed tileset was decoded eagerly into
`HashMap<String, Vec<u8>>` — once by `TilesetRegistry::load_all` (for the fe-api
tile routes) and AGAIN by the active petal (`petal_binding` re-read the whole
`.hexon` into its own `CompositeTileSource`). Evidence §5: 14.6 MiB today,
~3.8 GB at projected z12, doubled. S5 makes residency proportional to what the
camera can see.

- **Two backings, one type** (`tiles/hexon_source.rs`). `HexonTileSource::open(path)`
  is the production path: it opens a `fe_format::HexonArchiveIndex` (header
  phase only — central directory + manifest/entries/tileset_meta JSON, fe-format
  `src/AGENTS.md` §archive-index) and builds an I/O-free membership set
  `HashSet<(TileLayer, (z,x,y))>` from the member names. `from_archive(bytes)` /
  `from_directory` stay eager (tests, bundle assembly, `PartialTileSource`
  chunks) — those bytes are already in RAM, so there is nothing to defer.
- **Resident-only getters.** `get_tile` / `get_satellite_tile` /
  `get_baked_splats` (and `CompositeTileSource::get_tile_sync` /
  `get_satellite_tile_sync` / `get_baked_splats_sync`) return
  `Option<Arc<..>>` from RAM only — they never touch the disk, so any Update
  system may call them (NFR-2). `probe(layer, coord)` distinguishes `Hit` /
  `Miss` (member exists, needs a read) / `Absent` (not in this archive)
  without I/O. The composite getters also no longer consult the disk tile
  cache: that is a file read; only the off-thread `TileLoader` does it. (The
  disk cache is not populated by anything in production today — the
  `fetch`-feature `fetch_tile` that writes it has no caller.)
- **Blocking loaders.** `load_layer_blocking` / `load_tile_blocking` /
  `load_satellite_tile_blocking` / `load_baked_splats_blocking` read ONE member
  through the index (zip-bomb caps per member; CRC-checked by the zip crate),
  decode baked splats (a corrupt/future-version buffer is "no baked data",
  FR-5), and insert the value into the source's byte cache UNPINNED. Callers:
  the residency `TileLoader` on `IoTaskPool`, and fe-api's tile routes inside
  `spawn_blocking` (the registry's `get_tile` / `get_satellite_tile` are now
  blocking by contract).
- **Byte cache** (`tiles/residency.rs` `ByteLru`): byte-bounded LRU at
  `TILE_BYTE_BUDGET` = 256 MiB per source (a constant — no knob was asked
  for; reversal = one `AppSettings` field). Pinned entries are never evicted
  (they belong to live chunks); only unpinned entries (fe-api reads, loads
  whose chunk never spawned) age out. Pinned bytes may overshoot the budget —
  the terrain `max_chunks` cap bounds them.
- **One owner, shared `Arc`s.** `TilesetRegistry.sources` is
  `HashMap<String, Arc<HexonTileSource>>`; `source(id)` hands out the `Arc`,
  `insert_source` registers one. `load_all` opens indexes only (cheap
  startup). `CompositeTileSource.hexon_sources: Vec<Arc<HexonTileSource>>`
  holds the same `Arc`s (`add_hexon_source` takes `impl Into<Arc<_>>`, so an
  owned test source still works). A composite never mutates a shared source
  beyond its cache pins. **No-tear rule:** `registry.install` / `reload`
  insert a NEW `Arc`; the active composite keeps the old one until the next
  assignment.
- **Reinstall is atomic (p2p_asset_streaming S6; corrects the earlier
  "rewrites in place" text).** `HexonStore::install_tileset` writes
  `{id}.hexon.tmp-{pid}-{nanos}` then `std::fs::rename`s it over `{id}.hexon`
  — never a direct `std::fs::write` to the live path — so a concurrent P2P
  install of the same id (reachable since S4) can never hand a lazy reader a
  half-written file. An old `Arc`'s open `HexonArchiveIndex`/`LazyFile`
  handle keeps reading the PRE-RENAME bytes (the stdlib opens files with
  `FILE_SHARE_DELETE`, so Windows allows the rename while the handle is
  open), and finds different bytes behind its offsets only once it next
  seeks — exactly the same `ArchiveChanged` recovery as before.
  `HexonArchiveIndex::read_member` refuses with `ArchiveChanged` when the
  file's size/mtime moved; `load_layer_blocking` then re-indexes ONCE, swaps
  the index, drops unpinned (stale) cache entries, and retries. Pinned bytes
  of live chunks stay as rendered until they leave the ring. `tileset_meta`
  of the old `Arc` is not refreshed — the next assignment picks up the
  registry's fresh source.
- **Install is also serialized and durably synced (W2b F3, 2026-10-14).**
  `HexonStore::write_lock` (a per-store `Mutex<()>`) is held from the
  publisher-continuity check through both renames and the registry-entry
  update, so two installs of the SAME `hexon_id` racing on different threads
  (local file, P2P assemble, registry fetch) are fully serialized — the
  second always sees the first's completed result at its continuity check,
  never a stale "nothing installed yet" read interleaved with the first's
  write (mirrors the registry's own `publish_lock`, `fe-hexon-registry/AGENTS.md`
  "Publish validation" step 3). The same lock guards every other
  `registry.json` read-modify-write (`remove_tileset`, `set_seeding`,
  `update_last_served`), so an unserialized install can never write back a
  stale registry snapshot (losing a toggle) (W2b review; test
  `installs_racing_seeding_toggles_lose_no_registry_update`). `write_synced_and_rename` also
  `File::create`s + `write_all`s + `sync_all`s the temp file BEFORE each
  rename (the old `std::fs::write` had no fsync), and `install_tileset`
  fsyncs the store directory itself once after both renames (POSIX-only; a
  no-op on Windows, where NTFS's metadata journal already covers it). Test:
  `concurrent_installs_of_one_id_by_two_publishers_leave_exactly_one_winner`
  (`tests/hexon_integrity_flow.rs`) — the fsyncs themselves get no dedicated
  test (their success has no externally observable effect short of an actual
  power-loss event).
- **`save_registry` is synced too (W3b fixup, 2026-10-14).** It used to be
  the one `registry.json` write that skipped `write_synced_and_rename` (a
  bare `std::fs::write` + `std::fs::rename`, no `sync_all`), even though
  every caller already took `write_lock` around it. It now goes through the
  same helper every other write in this file uses, plus the same
  `fsync_dir()` the hexon/meta renames get — so a crash right after any
  `registry.json` rename (install, remove, set_seeding, update_last_served)
  can no longer leave `load_registry` reading an empty/corrupt file and
  silently reporting `RegistryData::default()` (every installed tileset
  "vanishes" from the list, though the `.hexon` files themselves survive).
  No dedicated test for the fsyncs, same reasoning as the hexon/meta fsyncs
  above. Its temp file is unique per call (`temp_path("registry", "json")`,
  W3b review), not a fixed `registry.json.tmp`: `write_lock` serializes
  writers in ONE process only, and the GUI and the headless relay both
  default to the same store directory, so a fixed name let one process
  truncate the temp the other was renaming into place. (Cross-process
  read-modify-write is still unserialized — two processes toggling at once
  can lose one update; no cross-process lock exists today.) Test:
  `a_stale_registry_temp_file_never_blocks_a_save`.
- **The lock is the reason remove/seeding moved off the Bevy main thread
  too (W3b fixup, AC-30b, 2026-10-14).** This lock is also why
  `HexonOp::Remove`/`SetSeeding` can no longer run inline in
  `fractalengine::terrain_bridge::drain_hexon_ops` (the previous paragraph's
  "Hexon Manager's remove/seeding toggles run on the main thread" is now
  stale) — a worker install can hold `write_lock` across a write + `sync_all`
  of up to 1 GiB (`fe-format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES`), and a
  main-thread caller blocking on the SAME `std::sync::Mutex` would freeze
  frames for that long. Both ops now run on ONE serial worker thread
  (`HexonMutationWorker`, FIFO, so clicks apply in order) and report back
  over `HexonAssembleResultSender`, like `Install`/`AssembleDownload` (see
  `fractalengine/src/AGENTS.md` §tileset-p2p). `update_last_served` has no production caller today, but
  would need the same treatment if one is ever wired up.
- **`load_tileset` refuses unsafe ids** (`is_safe_hexon_id`) before the path
  join: the id arrives from a (replicated) petal terrain config.
- **`CompositeTileSource::resident_tile(coord)`** resolves every layer with
  zero I/O or returns `None` ("a read is needed"): `Online`/`Hybrid` with a
  disk cache configured always need the loader for hexon misses (only a read
  can say whether the cache holds the tile); `Offline` never reads the disk
  cache (NFR-4, preserved in `TileLoader` too).

## §tile-residency — async loads, pending ≠ failed, exit-ring eviction (S5)

`TileResidency` (render feature, `tiles/residency.rs`) realises D-74's "async
dispatch" decision: the main thread never reads a tile.

- `ensure_resident(coord, composite)`: a ready (loaded, unspawned) tile →
  `Ready`; a load in flight → `Pending`; `composite.resident_tile` resolves it
  without I/O → `Ready`/`Absent`; otherwise dispatch
  `composite.loader().load_blocking(coord)` on `IoTaskPool` (≤
  `MAX_TILE_LOADS_IN_FLIGHT` = 16; beyond that the coord just stays pending
  and is retried next frame) → `Pending`. A finished load whose layers are
  all absent is `Absent` — the only path into `FailedTiles` besides an
  undecodable tile.
- `poll()` (first thing in `fetch_and_spawn_terrain_chunks`) harvests
  finished tasks with `bevy::tasks::futures::check_ready` and drops ready
  tiles nobody spawned within `READY_TTL_FRAMES` (camera moved on), so a long
  flight cannot accumulate `Arc`s.
- `reset(revision, grounding)` (from `apply_terrain_assignments`) cancels
  in-flight tasks (dropping a `Task` cancels it) and clears held tiles; a
  revision mismatch seen by the fetch system resets defensively.
- **Exit-ring byte eviction.** `fetch_and_spawn_terrain_chunks` pins a
  spawned chunk's hexon-origin layers in the source that supplied them
  (`pin_tile`, insert-if-evicted). `update_terrain_lod` despawns chunks
  beyond `DESPAWN_HYSTERESIS` × the spawn radius of EVERY streaming focus (or
  at a stale zoom for their nearest focus; §terrain_plugin "Multi-focus
  streaming") and
  calls `composite.release_tile(coord)`: the last pin drops the bytes
  immediately — they leave RAM with the chunk, not at some later LRU
  pressure point. Re-entry reloads them (identical bytes, identical heights).
  A reassignment releases the old composite's pins wholesale; only the one
  active composite ever pins.
- Counters for the S8 readout:
  - `CompositeTileSource::resident_bytes()` (distinct hexon sources);
  - `streamed_resident_bundles()` / `streamed_resident_bytes()` (S8; built on
    `StreamedTileSource::{resident_bundle_count, resident_bytes}`);
  - `HexonTileSource::{resident_bytes, pinned_len}`;
  - `TileResidency::{in_flight_len, ready_len}`.

  The host mirrors the first two groups into fe-runtime `ResidencyStats`
  (fractalengine `src/AGENTS.md` §residency-stats).
- Agent-vision settle: `CaptureFocusResidency.pending` counts a capture
  focus's outstanding ring tiles (pending ≠ failed applies there too); see
  §terrain_plugin "Multi-focus streaming".
- Cost: a chunk now appears a frame or more after it enters the ring (Risk 4,
  accepted; in-app check is USER_GATED). D-38 snap-to-terrain reads
  `TerrainHeightField`, which only covers resident chunks — unchanged by S5.

## §streamed-tilesets — not-installed tilesets over P2P (p2p_asset_streaming S7)

**Why.** Before S7 a `tileset_hexon_uri` with no local `.hexon` file simply
failed to load (`petal_binding` warns and skips it) — a petal referencing a
peer's tileset rendered nothing, forever, even after that peer became
reachable. S7 adds a THIRD tile source, `tiles/streamed.rs::StreamedTileSource`,
served bundle-by-bundle as they arrive over P2P, without ever installing
anything to the local hexon store.

- **One lookup, one new source kind.** `TilesetBundleIndex::bundle_for_tile`
  (fe-format, S1 critic C4) is the ONLY tile→bundle lookup; `StreamedTileSource`
  calls it, never reimplements it. A bundle, once fetched, is a fully-decoded
  `HexonTileSource::from_streamed_bundle` (`Backing::Memory` — no further I/O
  to serve it; S8b: decoded under `STREAMED_BUNDLE_DECODE_BUDGET`, 64 MiB, not
  the 1 GiB install budget, since several bundles stay resident at once and
  their bytes come from peers), so `CompositeTileSource` only needs a THIRD slot in its lookup chain:
  hexon sources → streamed sources → disk cache → online. Checked in EVERY
  mode including Offline (the bytes are hexon-origin, preserving NFR-4) —
  only the resident-and-decoded check runs; nothing here ever reads a disk
  or dispatches a network call synchronously.
- **Index binding (S8b).** `StreamedTileSource::from_meta_json(id, json)`
  refuses an index whose `tileset_id` is not `id`, so a peer answering a meta
  request for X with Y's index never becomes X's source.
- **Two states, one gate.** A `tileset_hexon_uri` that fails to load locally
  is `composite.mark_streamed_pending(uri)`'d by `petal_binding` instead of
  just warning. `CompositeTileSource::streamed_pending(coord)` is true when
  EITHER an uri is still unresolved (no index yet — conservatively, only for
  a coordinate no OTHER source already covers) OR a resolved
  `StreamedTileSource` covers `coord` but its bundle hasn't arrived.
  `fetch_and_spawn_terrain_chunks`'s ring loop checks this BEFORE
  `TileResidency::ensure_resident` and, if true, records the bundle demand
  (`record_bundle_demand`) and `continue`s WITHOUT touching
  `residency`/`FailedTiles` at all. This is deliberate: the async
  `TileResidency` IoTaskPool dispatch (§tile-residency) is a one-shot,
  non-blocking function call snapshotted at dispatch time — it can never
  "wait" for a bundle that arrives later, so routing a streamed-pending
  coordinate through it would classify it `Absent` once and poison it into
  `FailedTiles` for the rest of the revision, permanently, even after the
  bundle streams in. The early `continue` re-evaluates `streamed_pending`
  fresh every frame instead, so a coordinate recovers the instant its bundle
  lands.
- **The tile pointer-set.** `tiles::TileBundleDemand` (render-gated, `tiles/streamed.rs`)
  is the tile half of the D-74 pointer-set (fe-ui's S8 `ResidencyLedger` is
  the node/stamp half): `clear()`ed then re-demanded every frame by
  `fetch_and_spawn_terrain_chunks`, so it always reflects exactly what the
  ring wants NOW, never stale demand from a camera position the ring has
  since left — bounded over-fetch at a ring edge (D-73 evidence §3: at most
  one bundle). `fractalengine::terrain_bridge::request_demanded_bundles`
  reads it and sends `SyncCommand::RequestChunk` for each not-yet-requested
  `(tileset_id, bundle_seq)`.
- **Exit-ring eviction is per-BUNDLE, not per-tile.** Unlike an installed
  `HexonTileSource`'s per-`(layer, coord)` `ByteLru`, a streamed bundle
  covers many tiles, so `StreamedTileSource` ref-counts PINS per bundle
  sequence: `CompositeTileSource::pin_tile`/`release_tile` call the
  unconditional `pin_streamed`/`release_streamed` (a no-op for a non-streamed
  coordinate) alongside their existing hexon-origin handling, and the bundle
  drops from memory the instant its last pin releases. The verified bundle
  FILE stays on disk in the download dir as a cache (fe-sync `AGENTS.md`
  §tileset-transfer) — a re-entry re-demands the bundle but
  `fe-sync::TilesetTransfer::fetch_chunk` serves it from that verified file
  rather than re-fetching over the network (not re-verified here, since S2/S3
  already blake3-checked it before `ChunkReceived` ever fires). A bundle that
  arrives after the ring moved on is never pinned, so `release` never drops
  it: `StreamedTileSource::evict_idle` (via
  `CompositeTileSource::evict_idle_streamed_bundles`, called each frame by
  the binary with a 10 s grace) drops resident bundles with no pin that are
  older than the grace (final review 2026-10-14; test
  `a_bundle_no_chunk_ever_pinned_is_evicted_once_idle`).
- **The binary owns network state fe-terrain can't.** `petal_binding` only
  marks a uri pending and renders nothing for it; it has no provider
  addresses, no peer knowledge, and (by design, root `AGENTS.md` §petal-map)
  no fe-sync dependency. `fractalengine::terrain_bridge::StreamedTilesetState`
  (advertised / meta-requested / meta-failed / chunk-requested sets) and
  three systems — `drive_streamed_tilesets`, `drain_streamed_bundle_results`,
  `request_demanded_bundles` — are the bridge: see `fractalengine/src/AGENTS.md`
  §tileset-p2p for the full flow. A `RequestTilesetMeta`/`RequestChunk` is
  sent with an EMPTY `peer_id`/`provider`, letting the sync thread's own
  `TilesetProviderMap` (populated transparently from real inbound ads)
  resolve a dialable address — the binary only needs to know a tileset_id is
  advertised SOMEWHERE, never by whom.
- **Known, accepted limits (not fixed this slice, no ask to go further):**
  the off-thread `TileLoader`/`fetch_tile` (fetch feature) do not know about
  streamed sources — unreachable in practice, since `streamed_pending` never
  lets a streamed-covered coordinate dispatch through that path; the
  grounding center-tile read in `fetch_and_spawn_terrain_chunks` calls
  `ensure_resident` directly, bypassing the ring loop's `streamed_pending`
  gate (grounding is expected to resolve from an installed hexon at the
  origin, not a streamed one); `source_metas`/`reconcile_metric_frame`/
  `select_source_lods` (FR-4) stay hexon-only — streamed sources don't
  contribute scale reconciliation yet; a uri nobody ever advertises stays
  `pending_streamed_uris` forever for that assignment (observably identical
  to today's "no source covers it" — the region just never renders, silently,
  instead of once marking `FailedTiles`).

**Sealed streamed tilesets (PAS-P5 S5; pin chain added W2b F1, 2026-10-14).**
`StreamedTileSource::from_meta_json(id, json, pin)` accepts either child-0
shape. A `HeaderDoc` (`from_header_doc(id, header, pin)`) is verified through
`fe_hexon::partial::verify_fetched_header` — the SAME header check every
other network path (install-flow, every P2P transport) uses, no third copy —
under `RequireSealed { pin }` and bound to the requested id. A container then
streams by its signed `chunks/index.json`. A monolithic archive gets a
synthesized index: bundle 0 = the whole archive, with one full-range entry
per zoom, since `covers` already bounds the extent. `pin` is the caller's
local pin chain (installed publisher, then TOFU `KnownTilesetPublishers`) —
once a pin exists for a tileset_id, a header signed by a DIFFERENT publisher
is refused, and so is a legacy UNSEALED plain-index reply (it can no longer
silently downgrade a known-sealed publisher). The caller
(`fractalengine::terrain_bridge::drive_streamed_tilesets`) resolves the pin
and runs this whole check on a WORKER THREAD, never the Bevy main thread
(S4 invariant) — see `fractalengine/src/AGENTS.md` §tileset-p2p for the
verdict-channel shape. Every source answers `bundle_check(seq) ->
BundleCheck` (`Digest` = the signed digest, or a legacy index's own digest;
`WholeArchive` = signed manifest + publisher). The host decode thread runs
that check on the bytes before decoding them, so a bundle that no index
names, or that fails its seal, never becomes terrain.

## §chunked-containers — sealed chunk containers, atomic install, reassembly (p2p_asset_streaming S6)

**What a container is.** Produced by `fe_format::HexonArchive::export_chunked`
+ `fe_format::seal`: ONE `.hexon` whose `chunks/index.json` is a signed
`TilesetBundleIndex` and whose body members are `chunks/{seq}.hexon` — each a
self-contained (unsealed, in this slice) tileset archive of its own. The
OUTER manifest's signature is the only one that matters; chunks are not
individually sealed (D-76: "a hash committed under one signed manifest is
equivalent trust"), so there is exactly one signature to check, not N.

- **`HexonStore::install_tileset` accepts both layouts.** It always calls
  `HexonArchive::import` (unchanged — `VerifyIfSealed`, full integrity pass),
  then records `InstalledTileset::verified = matches!(data.integrity,
  IntegrityStatus::Sealed{..})`. A monolithic archive's `tileset_meta.
  tile_count` is whatever its own JSON declares; a container's comes from
  `chunks/index.json`'s `tileset_meta` (the CALLER — `builder.rs::
  package_tiles_chunked` — is responsible for setting it to the AGGREGATE
  across every chunk, not one chunk's own count).
- **`HexonTileSource::open` branches on `index.layout()`.** `Layout::
  Monolithic` keeps the existing `IndexedArchive` (membership scan of the
  outer archive's own member names, unchanged — now built via
  `IndexedArchive::from_index` over an already-open `HexonArchiveIndex`
  rather than re-opening the path). `Layout::Container{..}` takes a NEW
  `Backing::Container(ContainerArchive{index, chunk_index})`: `chunk_index`
  is the parsed, signed `chunks/index.json`, read once at `open` (header
  phase — no chunk body is ever touched here).
- **Lazy PER CHUNK, not per tile.** A tile resolves to its covering chunk via
  `TilesetBundleIndex::bundle_for_tile` (the one tile→chunk lookup, critic
  C4 — same call `StreamedTileSource` makes, §streamed-tilesets); the chunk
  is then imported ON DEMAND via `HexonArchiveIndex::chunk(seq)` (verified
  against the signed hash when the outer container is sealed) under
  `STREAMED_BUNDLE_DECODE_BUDGET`, and the ONE requested `(layer, key)` tile
  is extracted and handed to the source's existing `ByteLru` cache. The
  decoded chunk itself is discarded immediately after — there is no second,
  unbounded "resident chunks" cache alongside the tile `ByteLru`, so
  touching every chunk of a huge container still can never hold more than
  the cache's own budget resident at once (see "residency budget" below). A
  repeated request for a tile from the same chunk simply re-decodes that
  chunk if its tile already evicted from the `ByteLru` — CPU for safety, the
  same trade `StreamedTileSource` makes implicitly by discarding a bundle the
  instant its last pin releases. `TileLayer::BakedSplats` reads go through
  `crate::splat::format::read_baked_splats_from_archive` over the SAME
  already-fetched chunk bytes instead of `HexonArchive::import` (baked
  splats are not an `HexonArchiveData` field) — `package_tiles_chunked`
  bakes them per chunk, so this is a real served layer, not a FR-5
  live-synthesis-only fallback.
- **Residency budget (critic AC-21b).** `TILE_BYTE_BUDGET` (256 MiB,
  `tiles/residency.rs`) stays enforced PER `HexonTileSource`, not globally —
  making it truly global needs a cross-source LRU (a redesign out of this
  slice's scope; `ByteLru` has no handle to a "the other sources in this
  process" view). Instead `CompositeTileSource::MAX_HEXON_SOURCES` (4) caps
  how many `HexonTileSource`s one composite holds — a petal's
  `tileset_hexon_uris` realistically cascades a regional tileset over at
  most a couple of broader fallback bases, so the cap's 4 × 256 MiB = 1 GiB
  worst case is the same order of magnitude a single tileset's existing
  budget already accepted. `add_hexon_source` past the cap is refused
  (`warn!`), never silently dropped from an in-progress lookup chain.
  **Deferred, not dropped:** a genuinely global byte ceiling across every
  resident source — revisit if a real workload needs more than 4 cascaded
  sources per petal.
- **Nested budget (S8, R8).** A container chunk's baked-splat section is
  read with `read_baked_splats_from_archive_with_budget(bytes,
  STREAMED_BUNDLE_DECODE_BUDGET)` — the same budget its tile import uses —
  and a refusal is an ERROR, not a silent miss. Before S8 the splat read ran
  under the 1 GiB whole-archive budget on every tile miss, so a chunk of
  many sub-cap splat entries could inflate ~1 GiB per lookup. Test:
  `container_splat_reads_are_bounded_by_the_streamed_bundle_budget`.
- **Publisher continuity (S8).** `install_tileset` refuses
  (`PUBLISHER_CONTINUITY_ERROR`) to replace an installed SEALED tileset with
  another publisher's archive or with an unsealed one; the same publisher
  can still update it, and a legacy unsealed or broken install pins nothing.
  `HexonStore::installed_publisher(id)` (one header-phase open) is the pin;
  `install_tileset` is the one choke point every path (P2P, registry,
  local) goes through, so no transport-level pin mistake can overwrite a
  sealed install. The host's confirm gate also reads it, so a fresh process
  with an empty in-memory pin store still refuses an impostor before any
  chunk (fractalengine `src/AGENTS.md` §tileset-p2p). Test:
  `a_sealed_install_is_never_replaced_by_another_publisher_or_an_unsealed_archive`.
- **Atomic install** (critic AC-21b/c). See §lazy-tiles "Reinstall is
  atomic" above — `install_tileset` now writes a same-directory temp file
  then renames over `{id}.hexon`/`{id}.meta.json`, for BOTH layouts.
- **Reassembly (`tiles/distribution.rs::assemble_sealed_container`).**
  Unlike `assemble_chunks` (which flattens tiles into a fresh, UNSIGNED
  archive — the path `bundle_tileset`/`assemble_bundles` still use for the
  legacy, deterministic-bundling P2P download, §bundles — the §3.5
  reconciliation rule: "only legacy unsealed tilesets keep P3P4's
  deterministic bundling"), `assemble_sealed_container` takes an already-
  VERIFIED `fe_format::HeaderDoc` (what PAS-P5 S4/S5/S7a's `FetchStrategy`
  will hand it, once those slices exist) plus the per-chunk bytes a caller
  already checked against the header's signed `chunks/index.json`, and
  writes those EXACT bytes back into a new zip — `manifest.json` = the raw
  (already-signed) manifest text, each `HEADER_MEMBER_PATHS` member = its raw
  text, `chunks/{seq}.hexon` = the verified chunk bytes, nothing
  re-serialized. Because nothing is re-derived, the publisher's signature
  still covers exactly these bytes, so the reassembled file still verifies
  `Sealed` and can be re-seeded with the ORIGINAL seal intact (the test
  `assembled_p2p_download_installs_as_a_reseedable_sealed_container`,
  `tests/hexon_integrity_flow.rs`, proves the full round trip locally, since
  S4/S5/S7a don't exist yet to drive it from the network). `install_tileset`'s
  own `HexonArchive::import` (always `VerifyIfSealed`) is still the
  backstop: a caller that got pre-verification wrong, or any corruption
  introduced here, is refused before anything reaches disk — reassembly
  itself does not re-check digests.
- **`builder.rs::package_tiles_chunked`** (replaces the old `package_chunked`
  chunk-list output) is the one producer: pure over already-downloaded
  tiles, splits via the shared `bundle::split_tiles_into_bundles`, bakes
  splats per chunk (chunks are NOT individually sealed), then
  `export_chunked` + `seal` LAST, after every chunk's splat append — the
  §integrity "seal is always the last transform" rule applies to the OUTER
  container. `package_chunked` (async) is now a thin download-then-package
  wrapper, returning the ONE sealed container instead of the old
  `Vec<Vec<u8>>` of separately-signed-per-chunk-manifest archives (D-76
  retires that per-chunk scheme).
- **`TilesetInfo.verified`** (`registry.rs`) surfaces `InstalledTileset.
  verified` through `TilesetRegistry::list_tilesets`, so a future Hexon
  Manager UI can show an "unverified" badge without reaching into the store.

## Selected report coverage and freshness

The selected-volume responder retains only the latest request for the active,
enabled petal. It streams a replacement result under that correlation when the
assignment revision or resident `TerrainHeightField` changes. Unbinding and
switching petals discard the retained request. An unchanged frame does no
integration or report allocation. Host assignment producers run before
`TerrainSystemSet::ApplyAssignments`; this prevents a DB-confirmed selection
from being answered against the previous proposal configuration.

`SelectionReport` counts the exact selected integration cell centers and finite
base/proposed pairs using the integrator's `for_each_cell` lattice. Counts
describe selected-lattice coverage, not tile download progress or the sampling
support outside the selection used by smoothing. Complete-coverage integration
is unchanged. The renderer seam exposes both counts; consumers must withhold
numeric reports when coverage is incomplete, including empty lattices.

## Report item metadata

The terrain-to-renderer volume seam carries each item's actual operation and material instead of dropping them when the record is a baked region. These labels do not change the composite integrator or coverage/finite refusal.

## Changed terrain space and placed objects

`volume_report::changed_space_overlaps` shares the composite surface sampler with true cut/fill. For each existing
integration cell it clips object XZ bounds against the cell/selection bounds and intersects the object's Y interval
with the interval between pristine and combined proposed height. It therefore rejects objects above/below the
changed volume and finds extents crossing the footprint when their origins are outside it. Numbers use the same
world scale and disclose sampling tolerance. Flat rectangular box fixtures have analytical overlap4 cubic meters;
curved/relief/irregular boundaries remain sampled estimates. Missing samples and invalid bounds remain unresolved.
The retained report recomputes on scene-snapshot changes as well as terrain/height changes. Optional scene absence
never invents a successful empty object-intersection report.

## Changed terrain space and placed objects

`volume_report::changed_space_overlaps` shares the composite surface sampler with true cut/fill. Each integration
cell clips object XZ bounds against its cell/selection bounds and intersects object Y with the interval between
pristine and combined proposed height. Above/below objects are rejected; crossing extents are found even with an
outside origin. Volumes use the same world scale and disclosed sampling tolerance. Flat rectangular box fixtures
have analytical overlap4 cubic meters; relief/irregular boundaries remain sampled estimates. Missing samples and
invalid bounds remain unresolved. Scene changes invalidate retained reports alongside terrain/height changes.
Optional scene absence never invents a successful empty object analysis.

## Rendered track geometry source

RenderedTrackGeometry is produced by the same build that creates a track Mesh3d.
It retains the exact typed input points and style, filtered curve positions,
centroid, and the once-prepared closed polygon/triangulation used by that mesh.
Fields are read-only outside the renderer. The actual build binds its mesh asset
ID; a merely prepared snapshot has no mesh binding and cannot be tagged as a
verified rendered source. The host consumes this snapshot rather than rerunning
preparation against a possibly newer live TrackRouteMap or TrackStyleMap.

The geometry is in raw Petal-local meters. Neither terrain sampling nor world
scale participates in the track ribbon/zone build; the stored projected points
already carry their coordinates. A future geometry-dependent scale, projection,
terrain or algorithm mode must extend this source contract before checked tools
can accept it. Color/visibility remain separate render/picking availability gates.

## Complete earthwork measurements

The common cut_fill_volume lattice now returns expected_cells and finite
sampled_cells with its original planning-grade cut/fill formula. Its complete
predicate requires nonempty full coverage, valid real-unit scale and finite
accumulators. Missing/partial/nonfinite coverage remains unavailable; a fully
sampled zero is valid. Selection reports reuse these same counts rather than
walking a second readiness lattice. No alternate volume formula is introduced.

The bake producer retains assignment revision and pristine height generation.
Height insertion/removal/clear triggers remeasurement even if no mesh vertex
changes or no chunk mesh exists. Chunk bake markers also carry that generation.
Reports retain the exact raw DB assignment fingerprint, original parsed assignment
fingerprint, operational reconciled config fingerprint and serialized region
fingerprint. Invalid original scale, missing source identity, duplicate region
IDs, more than256 regions or region IDs above512 bytes cannot produce a usable
background persistence target. Source encodings are identities, not permissions.

The existing region integration and volume-request journeys extend to absent,
partial, nonfinite, ready and true-zero coverage, no-mesh remeasurement and foreign
Petal revisions. These assertions are source-authored until the parent sweep.

## §ruler-pointer-exclusion

QA69 (QA-0024): RulerHudInput::paint reports the complete painted footprint to
fe-renderer's ScenePointerChrome::exclude_completed_hud, so the HUD's pixels
never admit a scene press (Brush stroke, armed shape, camera). Pointer-only; see
fe-renderer/src/scene-pointer-design.md §passive-hud.
