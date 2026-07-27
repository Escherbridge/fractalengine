//! Terrain rendering plugin (render feature); see `src/AGENTS.md` §terrain_plugin.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::Indices;
use bevy::prelude::Projection as CameraProjection;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat};

use crate::config::{ElevationSourceKind, TerrainConfig};
use crate::iot::{TrackRouteMap, TrackStyleMap};
use crate::layers::{LayerId, LayerStack, LayerType};
use crate::lod_ring::{
    max_ring_radius_for_budget, ring_offsets, ring_radius_tiles, spawn_despawn_radii,
    upsample_factor_for_height, view_radius_world, wrong_zoom_replacement_present,
    DESPAWN_HYSTERESIS, MAX_UPSAMPLE, VIEW_RADIUS_FACTOR, ZOOM_BASE_HEIGHT_M,
};
use crate::mesh::interp::upsample_bilinear;
use crate::mesh::terrain::terrain_mesh;
use crate::mesh::track::{track_centroid, track_mesh, ColorMode as TrackColorMode};
use crate::petal_binding::{
    apply_terrain_assignments, ActivePetalTerrain, ActiveTileSource, TerrainAssignmentMsg,
};
use crate::projection::Projection;
use crate::scale::{scale_elevations, scale_local, scaled_tile_size, world_to_real_height};
use crate::tiles::{
    decode_png_pixels, CompositeTileSource, ElevationDecoder, TerrainRgbDecoder, TerrariumDecoder,
    TileCoord,
};
use fe_renderer::terrain_height::{HeightTile, TerrainHeightField};

/// Max chunks spawned per frame so a large adaptive ring fills without hitching.
const MAX_SPAWNS_PER_FRAME: usize = 16;

/// Hard cap on entities spawned per GeoJSON overlay file (polygons + polylines
/// + markers) — the only otherwise-unbounded spawner in this crate.
const MAX_GEOJSON_FEATURES: usize = 50_000;

/// Max decoded elevation-tile edge (px); oversized tiles are rejected so one
/// hostile/degenerate PNG can't build a multi-GB mesh. See AGENTS.md §tile-budget.
const MAX_TILE_DIMENSION: u32 = 1024;

/// Per-tile vertex budget after close-range upsampling (~34 MB of attributes).
const MAX_TILE_VERTICES: usize = 1_100_000;

/// Per-axis sample cap for the camera-clamp height field (~16 KB/tile);
/// see fe-renderer `src/AGENTS.md` §terrain-height.
const MAX_HEIGHT_GRID_AXIS: usize = 65;

/// Interim cap on proposal ghost meshes per petal (NFR-2). The true residency
/// budget (`spawn_allowance`/`mesh_instance_watchdog`) lives in the fractalengine
/// binary; see `render_terrain_proposals` for the TODO seam.
const MAX_PROPOSAL_OVERLAYS: usize = 4_096;

/// Take up to `requested` features from an overlay's shared budget.
fn take_feature_budget(remaining: &mut usize, requested: usize) -> usize {
    let granted = requested.min(*remaining);
    *remaining -= granted;
    granted
}

/// Largest upsample factor `1..=requested` keeping the densified grid
/// `((w-1)·U+1)·((h-1)·U+1)` within `max_vertices`. `w,h ≤ MAX_TILE_DIMENSION`
/// always fit at `U = 1`.
fn clamp_upsample_for_grid(w: u32, h: u32, requested: u32, max_vertices: usize) -> u32 {
    let mut u = requested.max(1);
    while u > 1 {
        let gw = (w.saturating_sub(1) as usize) * u as usize + 1;
        let gh = (h.saturating_sub(1) as usize) * u as usize + 1;
        if gw.saturating_mul(gh) <= max_vertices {
            break;
        }
        u -= 1;
    }
    u
}

/// Bilinear-resample a row-major grid down to ≤ `max_axis` per side (corner-preserving);
/// feeds the camera-clamp height field without duplicating full mesh grids in RAM.
fn resample_height_grid(
    grid: &[f32],
    w: usize,
    h: usize,
    max_axis: usize,
) -> (Vec<f32>, usize, usize) {
    if w < 2 || h < 2 || grid.len() != w * h {
        return (grid.to_vec(), w, h);
    }
    if w <= max_axis && h <= max_axis {
        return (grid.to_vec(), w, h);
    }
    let nw = w.min(max_axis).max(2);
    let nh = h.min(max_axis).max(2);
    let mut out = Vec::with_capacity(nw * nh);
    for r in 0..nh {
        let v = r as f32 / (nh - 1) as f32 * (h - 1) as f32;
        let r0 = (v.floor() as usize).min(h - 2);
        let fv = v - r0 as f32;
        for c in 0..nw {
            let u = c as f32 / (nw - 1) as f32 * (w - 1) as f32;
            let c0 = (u.floor() as usize).min(w - 2);
            let fu = u - c0 as f32;
            let g00 = grid[r0 * w + c0];
            let g10 = grid[r0 * w + c0 + 1];
            let g01 = grid[(r0 + 1) * w + c0];
            let g11 = grid[(r0 + 1) * w + c0 + 1];
            out.push(
                g00 * (1.0 - fu) * (1.0 - fv)
                    + g10 * fu * (1.0 - fv)
                    + g01 * (1.0 - fu) * fv
                    + g11 * fu * fv,
            );
        }
    }
    (out, nw, nh)
}

/// Links an entity to a layer in the [`LayerStack`] for visibility synchronization.
#[derive(Component)]
pub struct LayerEntity {
    pub layer_id: LayerId,
}

/// Marker component for spawned terrain chunk entities.
#[derive(Component)]
pub struct TerrainChunk {
    /// Tile coordinates (zoom, x, y).
    pub tile_coords: (u8, u32, u32),
    /// LOD level for this chunk.
    pub lod: u8,
}

/// Marker component for waypoint marker entities (pickable by selection raycast).
#[derive(Component)]
pub struct WaypointMarker {
    pub node_id: String,
    pub symbol: String,
    pub label: String,
}

/// Marker component for GPX track line entities.
#[derive(Component, Clone)]
pub struct GpxTrackLine {
    pub track_node_id: String,
}

/// Marker component for GeoJSON overlay source entities.
#[derive(Component, Clone)]
pub struct GeoJsonOverlay {
    pub source_path: String,
}

/// Marks a GeoJSON source entity as processed (and its spawned children).
#[derive(Component)]
pub struct GeoJsonProcessed;

/// Marker for a spawned terrain-proposal ghost mesh (FR-5); carries the proposal
/// id so a future selection/report path can resolve back to the record.
#[derive(Component, Clone)]
pub struct ProposalOverlayEntity {
    pub proposal_id: String,
}

/// Tracks the assignment revision the proposal ghosts were last built for, so
/// they rebuild only when the petal's proposals actually change.
#[derive(Resource, Default)]
pub struct ProposalRenderState {
    last_revision: Option<u64>,
}

/// Chunk marker: earthwork regions were last baked into this chunk's mesh at
/// `revision` (T3; see `src/AGENTS.md` §sculpt (Earthwork bake)).
#[derive(Component)]
pub struct EarthworkBaked {
    pub revision: u64,
}

/// Pristine (pre-bake) local vertex Y snapshot for a chunk mesh, taken before
/// its first earthwork bake so every re-bake starts from the untouched base
/// (Q-2 revert; see `src/AGENTS.md` §sculpt (Earthwork bake)).
#[derive(Component)]
pub struct PristineChunkHeights(pub Vec<f32>);

/// Revision bookkeeping for the earthwork bake + one-shot missing-field warn.
#[derive(Resource, Default)]
pub struct EarthworkBakeState {
    last_revision: Option<u64>,
    warned_revision: Option<u64>,
}

/// Configuration for terrain LOD thresholds.
#[derive(Resource, Clone)]
pub struct TerrainLodConfig {
    /// Distance at which to switch to higher LOD tiles.
    pub lod_distances: Vec<f32>,
    /// Maximum number of chunks to keep active at once.
    pub max_chunks: usize,
}

impl Default for TerrainLodConfig {
    fn default() -> Self {
        Self {
            lod_distances: vec![50.0, 200.0, 500.0, 1000.0],
            max_chunks: 256,
        }
    }
}

/// Configuration for waypoint marker appearance.
#[derive(Resource, Clone)]
pub struct WaypointMarkerConfig {
    /// Marker mesh size in world units.
    pub marker_size: f32,
    /// Default marker color RGBA.
    pub marker_color: [f32; 4],
}

impl Default for WaypointMarkerConfig {
    fn default() -> Self {
        Self {
            marker_size: 0.5,
            marker_color: [1.0, 0.8, 0.0, 1.0],
        }
    }
}

/// Tiles that failed to load under the current assignment revision (anti retry-storm).
#[derive(Resource, Default)]
pub struct FailedTiles {
    revision: u64,
    tiles: HashSet<(u8, u32, u32)>,
}

/// The main terrain rendering plugin; see `src/AGENTS.md` §terrain_plugin.
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainLodConfig>()
            .init_resource::<WaypointMarkerConfig>()
            .init_resource::<TerrainHeightField>()
            .init_resource::<TrackRouteMap>()
            .init_resource::<TrackStyleMap>()
            .init_resource::<ActivePetalTerrain>()
            .init_resource::<ActiveTileSource>()
            .init_resource::<FailedTiles>()
            .init_resource::<ProposalRenderState>()
            .init_resource::<EarthworkBakeState>()
            .insert_resource(LayerStack::new())
            .add_message::<TerrainAssignmentMsg>()
            // Idempotent: fe-ui's plugin also registers this shared-seam message.
            .add_message::<fe_renderer::terrain_overlay::EarthworkVolumeReport>()
            .add_systems(
                Update,
                (
                    apply_terrain_assignments,
                    update_terrain_lod,
                    fetch_and_spawn_terrain_chunks,
                    render_gpx_tracks,
                    render_waypoint_markers,
                    render_geojson_overlays,
                    render_terrain_proposals,
                    bake_earthwork_regions,
                    sync_layer_visibility,
                )
                    .chain(),
            )
            .add_systems(Update, crate::iot::animation::advance_track_animations)
            .add_plugins(crate::ruler_plugin::RulerPlugin);
    }
}

/// Map camera height to a tile zoom level (higher camera → lower zoom), clamped.
pub(crate) fn desired_zoom(cam_height_m: f32, min_zoom: u8, max_zoom: u8) -> u8 {
    let (min_zoom, max_zoom) = if min_zoom <= max_zoom {
        (min_zoom, max_zoom)
    } else {
        (max_zoom, min_zoom)
    };
    let h = f64::from(cam_height_m.max(1.0));
    // One zoom step out per doubling of height above the base (see ZOOM_BASE_HEIGHT_M).
    let steps = (h / ZOOM_BASE_HEIGHT_M).max(1.0).log2().floor() as i64;
    let z = max_zoom as i64 - steps.max(0);
    z.clamp(min_zoom as i64, max_zoom as i64) as u8
}

/// Perspective far plane in world units, if the camera has a perspective projection.
fn perspective_far_world(proj: &CameraProjection) -> Option<f64> {
    match proj {
        CameraProjection::Perspective(p) => Some(p.far as f64),
        _ => None,
    }
}

/// East-west width of a slippy tile in meters at the given latitude and zoom.
pub(crate) fn tile_world_size_m(lat: f64, zoom: u8) -> f64 {
    40_075_016.686 * lat.to_radians().cos() / 2f64.powi(zoom as i32)
}

/// Despawn chunks beyond the hysteresis view radius, or at a stale zoom once the
/// desired-zoom replacement fully covers them (hole-free); see AGENTS.md §terrain_plugin.
fn update_terrain_lod(
    camera_query: Query<(&GlobalTransform, &CameraProjection), With<Camera>>,
    chunk_query: Query<(Entity, &TerrainChunk, &GlobalTransform)>,
    lod_config: Res<TerrainLodConfig>,
    active: Res<ActivePetalTerrain>,
    mut commands: Commands,
    mut height_field: Option<ResMut<TerrainHeightField>>,
) {
    let Ok((cam_transform, cam_proj)) = camera_query.single() else {
        return;
    };
    let cam_pos = cam_transform.translation();

    // Scale-aware: reason about zoom in real meters and despawn distances in
    // world units (chunk transforms are scaled); see AGENTS.md §scale.
    let scale = active
        .config
        .as_ref()
        .filter(|c| c.enabled)
        .map(|c| c.effective_world_scale())
        .unwrap_or(1.0);
    let desired = active.config.as_ref().filter(|c| c.enabled).map(|c| {
        desired_zoom(
            world_to_real_height(cam_pos.y as f64, scale) as f32,
            c.min_zoom,
            c.max_zoom,
        )
    });

    // Despawn radius follows the visible frustum with hysteresis so tiles never
    // flicker or leave holes while still in view; floored to ~2 tiles + the LOD
    // base so big low-zoom tiles don't churn.
    let far_world = perspective_far_world(cam_proj)
        .unwrap_or_else(|| (cam_pos.y as f64).abs() * VIEW_RADIUS_FACTOR);
    let view_r = view_radius_world(cam_pos.y as f64, far_world, VIEW_RADIUS_FACTOR);
    let base_max = lod_config.lod_distances.last().copied().unwrap_or(1000.0) as f64 * 2.0;
    let tile_world_equator = match desired {
        Some(z) => scaled_tile_size(tile_world_size_m(0.0, z), scale),
        None => scaled_tile_size(tile_world_size_m(0.0, 0), scale),
    };
    let (_spawn_r, despawn_r) =
        spawn_despawn_radii(view_r.max(base_max), tile_world_equator, DESPAWN_HYSTERESIS);

    let existing: HashSet<(u8, u32, u32)> =
        chunk_query.iter().map(|(_, c, _)| c.tile_coords).collect();

    for (entity, chunk, chunk_transform) in chunk_query.iter() {
        let too_far = (cam_pos.distance(chunk_transform.translation()) as f64) > despawn_r;
        let coord = TileCoord::new(
            chunk.tile_coords.1,
            chunk.tile_coords.2,
            chunk.tile_coords.0,
        );
        let stale_zoom =
            desired.is_some_and(|z| wrong_zoom_replacement_present(coord, z, &existing));
        if too_far || stale_zoom {
            commands.entity(entity).despawn();
            if let Some(field) = height_field.as_mut() {
                field.remove(&chunk.tile_coords);
            }
        }
    }
}

/// Offline-first chunk spawning: an adaptive tile ring (frustum-sized, nearest-first)
/// around the camera at the desired zoom; see AGENTS.md §terrain_plugin.
#[allow(clippy::too_many_arguments)]
fn fetch_and_spawn_terrain_chunks(
    camera_query: Query<(&GlobalTransform, &CameraProjection), With<Camera>>,
    chunk_query: Query<&TerrainChunk>,
    lod_config: Res<TerrainLodConfig>,
    active: Res<ActivePetalTerrain>,
    tile_source: Res<ActiveTileSource>,
    layer_stack: Res<LayerStack>,
    mut failed: ResMut<FailedTiles>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut height_field: Option<ResMut<TerrainHeightField>>,
) {
    let Some(config) = active.config.as_ref().filter(|c| c.enabled) else {
        return;
    };
    let (Some(composite), Some(projection)) = (
        tile_source.composite.as_ref(),
        tile_source.projection.as_ref(),
    ) else {
        return;
    };
    let Ok((cam_transform, cam_proj)) = camera_query.single() else {
        return;
    };

    if failed.revision != active.revision {
        failed.tiles.clear();
        failed.revision = active.revision;
    }

    let mut active_count = chunk_query.iter().count();
    if active_count >= lod_config.max_chunks {
        return;
    }

    // Invert world scale so the camera is reasoned about in real meters (zoom
    // selection + projection expect real-world coordinates); see AGENTS.md §scale.
    let scale = config.effective_world_scale();
    let cam_pos = cam_transform.translation();
    let (lat, lon, _) = projection.local_to_wgs84(
        cam_pos.x as f64 / scale,
        cam_pos.y as f64 / scale,
        cam_pos.z as f64 / scale,
    );
    let lat = lat.clamp(-85.0, 85.0);
    let lon = lon.clamp(-180.0, 180.0);
    let cam_real_height = world_to_real_height(cam_pos.y as f64, scale) as f32;
    let zoom = desired_zoom(cam_real_height, config.min_zoom, config.max_zoom);

    // Adaptive ring: cover the visible frustum (camera height × factor, capped by
    // the scale-aware far plane) instead of a fixed 3×3, bounded by the chunk budget.
    let far_world = perspective_far_world(cam_proj)
        .unwrap_or_else(|| (cam_pos.y as f64).abs() * VIEW_RADIUS_FACTOR);
    let view_r = view_radius_world(cam_pos.y as f64, far_world, VIEW_RADIUS_FACTOR);
    let scaled_tile = scaled_tile_size(tile_world_size_m(lat, zoom), scale);
    let max_radius = max_ring_radius_for_budget(lod_config.max_chunks);
    let radius = ring_radius_tiles(view_r, scaled_tile, max_radius);

    // Close-range upsampling: once the camera descends below the served max zoom's
    // natural height, densify decoded elevations so geometry stays smooth up close.
    let upsample = if zoom >= config.max_zoom {
        upsample_factor_for_height(cam_real_height as f64, ZOOM_BASE_HEIGHT_M, MAX_UPSAMPLE)
    } else {
        1
    };

    let existing: HashSet<(u8, u32, u32)> = chunk_query.iter().map(|c| c.tile_coords).collect();
    let center = TileCoord::from_lat_lon(lat, lon, zoom);
    let tiles_per_axis = TileCoord::tiles_at_zoom(zoom) as i64;

    let mut spawned_this_frame = 0usize;
    // Nearest-first so the camera's immediate surroundings fill before the edges.
    for (dx, dy) in ring_offsets(radius) {
        if active_count >= lod_config.max_chunks || spawned_this_frame >= MAX_SPAWNS_PER_FRAME {
            return;
        }
        let x = center.x as i64 + dx;
        let y = center.y as i64 + dy;
        if x < 0 || y < 0 || x >= tiles_per_axis || y >= tiles_per_axis {
            continue;
        }
        let coord = TileCoord::new(x as u32, y as u32, zoom);
        let key = (zoom, coord.x, coord.y);
        if existing.contains(&key) || failed.tiles.contains(&key) {
            continue;
        }

        if spawn_chunk(
            coord,
            config,
            composite,
            projection,
            &layer_stack,
            upsample,
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            height_field.as_deref_mut(),
        ) {
            active_count += 1;
            spawned_this_frame += 1;
        } else {
            failed.tiles.insert(key);
        }
    }
}

/// Build + spawn one terrain chunk entity; returns false when no data was usable.
///
/// `upsample` (≥1) bilinearly densifies decoded elevations for close-range detail.
#[allow(clippy::too_many_arguments)]
fn spawn_chunk(
    coord: TileCoord,
    config: &TerrainConfig,
    composite: &CompositeTileSource,
    projection: &Projection,
    layer_stack: &LayerStack,
    upsample: u32,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    height_field: Option<&mut TerrainHeightField>,
) -> bool {
    let elevation_png = composite.get_tile_sync(coord);
    let satellite_bytes = composite.get_satellite_tile_sync(coord);
    if elevation_png.is_none() && satellite_bytes.is_none() {
        tracing::warn!(tile = %coord.cache_key(), "no elevation or satellite tile available; skipping");
        return false;
    }

    // Tile geometry: SW-corner anchor, mesh +x = east, +z = north (rows flipped).
    // World scale shrinks/grows the whole tile (size, heights, anchor) so several
    // scales of space are viewable; see `src/AGENTS.md` §scale.
    let scale = config.effective_world_scale();
    let (nw_lat, nw_lon) = coord.to_lat_lon();
    let (s_lat, _) = TileCoord::new(coord.x, coord.y.saturating_add(1), coord.zoom).to_lat_lon();
    let tile_size = scaled_tile_size(tile_world_size_m((nw_lat + s_lat) / 2.0, coord.zoom), scale);

    let elevation_data = elevation_png.and_then(|png| match decode_png_pixels(&png) {
        Ok((_, w, h)) if w > MAX_TILE_DIMENSION || h > MAX_TILE_DIMENSION => {
            // Oversized tile: one such PNG upsampled could allocate multi-GB
            // meshes and OOM the host — reject it (§tile-budget).
            tracing::warn!(
                tile = %coord.cache_key(),
                width = w,
                height = h,
                "elevation tile exceeds {}px cap; ignoring",
                MAX_TILE_DIMENSION
            );
            None
        }
        Ok((pixels, w, h)) if w > 1 && h > 1 => {
            let decoded = match config.elevation_source {
                ElevationSourceKind::TerrainRgb => TerrainRgbDecoder.decode(&pixels, w, h),
                ElevationSourceKind::Terrarium => TerrariumDecoder.decode(&pixels, w, h),
                ElevationSourceKind::None => vec![0.0; (w * h) as usize],
            };
            // Clamp the densification so the vertex count stays budgeted even
            // for large (e.g. 512px @2x) tiles (§tile-budget).
            let upsample = clamp_upsample_for_grid(w, h, upsample, MAX_TILE_VERTICES);
            // Densify before scaling so interpolation stays in real meters and the
            // world-Y invariant `(ele - origin_ele) * scale` is preserved.
            let (grid, gw, gh) = upsample_bilinear(&decoded, w as usize, h as usize, upsample);
            // Scale heights to world units (origin_ele subtraction lands in the anchor).
            let scaled = scale_elevations(&grid, scale as f32);
            let flipped = flip_rows(&scaled, gw, gh);
            let mesh = terrain_mesh(&flipped, gw as u32, gh as u32, tile_size);
            // Downsampled copy feeds the camera-clamp height field (§terrain-height).
            let heights = resample_height_grid(&flipped, gw, gh, MAX_HEIGHT_GRID_AXIS);
            Some((mesh, heights))
        }
        Ok(_) => {
            tracing::warn!(tile = %coord.cache_key(), "elevation tile smaller than 2x2; ignoring");
            None
        }
        Err(err) => {
            tracing::warn!(tile = %coord.cache_key(), error = %err, "failed to decode elevation tile");
            None
        }
    });

    let (mesh, heights) = match elevation_data {
        Some(t) => t,
        None if satellite_bytes.is_some() => {
            // Flat 16x16 grid fallback so the satellite texture still renders.
            let mesh = terrain_mesh(&vec![0.0f32; 16 * 16], 16, 16, tile_size);
            (mesh, (vec![0.0f32; 4], 2, 2))
        }
        None => return false,
    };

    let has_satellite_texture;
    let material = match satellite_bytes.as_deref().and_then(decode_satellite_image) {
        Some(image) => {
            has_satellite_texture = true;
            let handle = images.add(image);
            materials.add(StandardMaterial {
                base_color_texture: Some(handle),
                ..default()
            })
        }
        None => {
            has_satellite_texture = false;
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.35, 0.4, 0.3),
                ..default()
            })
        }
    };

    // Anchor at SW corner with ele=0 so mesh y (scaled meters) lands at (ele - origin_ele) * scale.
    let anchor = match projection.wgs84_to_local(s_lat, nw_lon, 0.0) {
        Ok(a) => scale_local(a, scale),
        Err(err) => {
            tracing::warn!(tile = %coord.cache_key(), error = %err, "tile corner outside projection bounds");
            return false;
        }
    };

    let layer_id = if has_satellite_texture {
        find_layer(layer_stack, |t| matches!(t, LayerType::Satellite))
            .or_else(|| find_layer(layer_stack, |t| matches!(t, LayerType::Terrain)))
    } else {
        find_layer(layer_stack, |t| matches!(t, LayerType::Terrain))
    };

    let mut entity = commands.spawn((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        Transform::from_xyz(anchor[0] as f32, anchor[1] as f32, anchor[2] as f32),
        TerrainChunk {
            tile_coords: (coord.zoom, coord.x, coord.y),
            lod: coord.zoom,
        },
    ));
    if let Some(layer_id) = layer_id {
        entity.insert(LayerEntity { layer_id });
    }
    if let Some(field) = height_field {
        let (grid, hw, hh) = heights;
        field.insert(
            (coord.zoom, coord.x, coord.y),
            HeightTile {
                anchor: Vec3::new(anchor[0] as f32, anchor[1] as f32, anchor[2] as f32),
                tile_size: tile_size as f32,
                grid,
                width: hw as u32,
                height: hh as u32,
            },
        );
    }
    true
}

/// First layer in the stack matching the predicate.
fn find_layer(stack: &LayerStack, pred: impl Fn(&LayerType) -> bool) -> Option<LayerId> {
    stack.iter().find(|l| pred(&l.layer_type)).map(|l| l.id)
}

/// Reverse elevation rows so data row 0 (north) maps to the mesh's far (+z) edge.
fn flip_rows(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(v.len());
    for row in (0..h).rev() {
        out.extend_from_slice(&v[row * w..(row + 1) * w]);
    }
    out
}

/// Decode PNG/JPG satellite bytes into a Bevy texture (v-flipped to match mesh rows).
fn decode_satellite_image(bytes: &[u8]) -> Option<Image> {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.flipv().to_rgba8();
            let (w, h) = rgba.dimensions();
            let mut image = Image::new(
                Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba.into_raw(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            // Clamp-to-edge + linear so tile borders don't wrap-bleed into dark seams; see `src/AGENTS.md` §terrain_plugin.
            image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::ClampToEdge,
                address_mode_v: ImageAddressMode::ClampToEdge,
                address_mode_w: ImageAddressMode::ClampToEdge,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                ..default()
            });
            Some(image)
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to decode satellite tile image");
            None
        }
    }
}

/// Render GPX track overlays as width-aware ribbon meshes honoring each track's
/// per-track [`TrackStyle`] (color, width, visibility); skips non-finite points.
///
/// One-shot build for entities `Without<Mesh3d>` — a live restyle re-enters this
/// path via the gpx bridge despawning+respawning the `GpxTrackLine` (the same
/// `force_line_redraw` discipline point-edits use). Color, width AND visibility
/// are all applied here at build time: there is no separate visibility fast
/// path — a `gis.track.visible` toggle persists like any other style prop and so
/// takes the same `SetNodeProperty` → bridge despawn/respawn → rebuild round-trip
/// as a color/width change. `Visibility::Hidden`/`Inherited` is just the value
/// this build writes based on the current style. See `src/AGENTS.md`
/// §track-styling.
fn render_gpx_tracks(
    track_query: Query<(Entity, &GpxTrackLine), Without<Mesh3d>>,
    route_map: Res<TrackRouteMap>,
    style_map: Res<TrackStyleMap>,
    layer_stack: Res<LayerStack>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, track) in track_query.iter() {
        let Some(route) = route_map.routes.get(&track.track_node_id) else {
            continue;
        };

        // pen_curve_tool_20260722 (Phase 2): flatten per-anchor bezier handles into
        // the dense ribbon polyline (all-corner tracks pass through unchanged).
        let positions: Vec<[f32; 3]> = crate::mesh::curve::flatten_route(
            &route.points,
            crate::mesh::curve::SAMPLES_PER_SEGMENT,
        )
        .into_iter()
        .filter(|v| v[0].is_finite() && v[1].is_finite() && v[2].is_finite())
        .collect();

        if positions.len() < 2 {
            continue;
        }

        // FR-3/FR-4: default style reproduces the historic cyan look for tracks
        // with no persisted `gis.track.*` props.
        let style = style_map.style_for(&track.track_node_id);

        // FR-3 thickness: the ribbon builder extrudes a width-aware triangle
        // strip and bakes the solid color into vertex colors — so the material
        // is WHITE + unlit (vertex colors tint it) rather than a base_color.
        let color = Color::srgba_u8(
            style.color_u8()[0],
            style.color_u8()[1],
            style.color_u8()[2],
            style.color_u8()[3],
        );
        // path_interaction_20260716 (FR-4): centroid-anchor the ribbon so the
        // gimbal has a real transform to grab. Build the mesh with positions
        // relative to the centroid and spawn the entity `Transform` at the
        // centroid — net world position unchanged, but Move/Rotate/Scale now
        // pivot about the path's center. The gpx bridge tags a `TrackPickShape`
        // carrying the SAME centroid so the whole-path bake pivots identically.
        let centroid = track_centroid(&positions);
        let rel: Vec<[f32; 3]> = positions
            .iter()
            .map(|p| [p[0] - centroid[0], p[1] - centroid[1], p[2] - centroid[2]])
            .collect();
        let ribbon = track_mesh(&rel, style.width.max(0.01), TrackColorMode::Solid(color));

        let handle = meshes.add(ribbon);
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            ..default()
        });

        // try_insert throughout: the bridge may despawn a line the same frame
        // (petal switch / style re-render) before these commands flush.
        let mut e = commands.entity(entity);
        e.try_insert((
            Mesh3d(handle),
            MeshMaterial3d(material),
            Transform::from_translation(Vec3::new(centroid[0], centroid[1], centroid[2])),
        ));
        // FR-3 visibility: apply the current visible flag at build time. A later
        // toggle isn't a cheap in-place `Visibility` flip — it persists the
        // `gis.track.visible` prop, which despawns+respawns the `GpxTrackLine`
        // via the bridge and re-runs this build, same as a color/width change.
        e.try_insert(if style.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });

        let layer_id = find_layer(
            &layer_stack,
            |t| matches!(t, LayerType::GpxTrack { node_id, .. } if node_id == &track.track_node_id),
        );
        if let Some(layer_id) = layer_id {
            e.try_insert(LayerEntity { layer_id });
        }
    }
}

/// Render waypoint markers as small pickable spheres.
fn render_waypoint_markers(
    waypoint_query: Query<(Entity, &WaypointMarker), Without<Mesh3d>>,
    marker_config: Res<WaypointMarkerConfig>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if waypoint_query.is_empty() {
        return;
    }

    let sphere = meshes.add(Sphere::new(marker_config.marker_size));
    let color = Color::srgba(
        marker_config.marker_color[0],
        marker_config.marker_color[1],
        marker_config.marker_color[2],
        marker_config.marker_color[3],
    );
    let material = materials.add(StandardMaterial::from(color));

    for (entity, _marker) in waypoint_query.iter() {
        commands.entity(entity).try_insert((
            Mesh3d(sphere.clone()),
            MeshMaterial3d(material.clone()),
            Pickable::default(),
        ));
    }
}

/// Render GeoJSON overlays once per source entity; marks sources processed.
fn render_geojson_overlays(
    overlay_query: Query<(Entity, &GeoJsonOverlay), Without<GeoJsonProcessed>>,
    layer_stack: Res<LayerStack>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, overlay) in overlay_query.iter() {
        // Mark processed up-front (also on failure) so a bad file never retry-spams.
        commands.entity(entity).try_insert(GeoJsonProcessed);

        let json_str = match std::fs::read_to_string(&overlay.source_path) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(path = %overlay.source_path, error = %err, "failed to read GeoJSON overlay");
                continue;
            }
        };

        // Simple identity projection — in production, use the petal's projection.
        let result = match crate::layers::parse_geojson(&json_str, |lon, lat| {
            (lon as f32, lat as f32)
        }) {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(path = %overlay.source_path, error = %err, "failed to parse GeoJSON overlay");
                continue;
            }
        };

        let layer_id = find_layer(
            &layer_stack,
            |t| matches!(t, LayerType::GeoJsonOverlay { source_path } if source_path == &overlay.source_path),
        );

        // One shared budget across the three feature kinds so a huge overlay
        // saturates instead of spawning millions of entities.
        let total = result.polygon_meshes.len()
            + result.polyline_meshes.len()
            + result.marker_positions.len();
        let mut remaining = MAX_GEOJSON_FEATURES;
        if total > MAX_GEOJSON_FEATURES {
            tracing::warn!(
                path = %overlay.source_path,
                features = total,
                cap = MAX_GEOJSON_FEATURES,
                "GeoJSON overlay saturated feature cap; truncating"
            );
        }

        let polygons = take_feature_budget(&mut remaining, result.polygon_meshes.len());
        for polygon in result.polygon_meshes.iter().take(polygons) {
            let mut mesh = Mesh::new(
                bevy::render::render_resource::PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, polygon.positions.clone());

            let mat = materials.add(StandardMaterial {
                base_color: Color::srgba(
                    polygon.fill_color[0],
                    polygon.fill_color[1],
                    polygon.fill_color[2],
                    polygon.fill_color[3],
                ),
                ..default()
            });

            let mut child = commands.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                GeoJsonProcessed,
            ));
            if let Some(layer_id) = layer_id {
                child.insert(LayerEntity { layer_id });
            }
        }

        let polylines = take_feature_budget(&mut remaining, result.polyline_meshes.len());
        for line in result.polyline_meshes.iter().take(polylines) {
            let mut mesh = Mesh::new(
                bevy::render::render_resource::PrimitiveTopology::LineStrip,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, line.positions.clone());

            let mat = materials.add(StandardMaterial {
                base_color: Color::srgba(
                    line.stroke_color[0],
                    line.stroke_color[1],
                    line.stroke_color[2],
                    line.stroke_color[3],
                ),
                ..default()
            });

            let mut child = commands.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                GeoJsonProcessed,
            ));
            if let Some(layer_id) = layer_id {
                child.insert(LayerEntity { layer_id });
            }
        }

        let markers = take_feature_budget(&mut remaining, result.marker_positions.len());
        // One sphere mesh shared by every marker in this overlay (no per-marker asset).
        let shared_marker_mesh = meshes.add(Sphere::new(0.3));
        for marker in result.marker_positions.iter().take(markers) {
            let marker_mesh = shared_marker_mesh.clone();
            let marker_mat = materials.add(StandardMaterial::from(Color::srgba(
                marker.color[0],
                marker.color[1],
                marker.color[2],
                marker.color[3],
            )));

            let mut child = commands.spawn((
                Mesh3d(marker_mesh),
                MeshMaterial3d(marker_mat),
                Transform::from_xyz(marker.position[0], marker.position[1], marker.position[2]),
                Pickable::default(),
                GeoJsonProcessed,
            ));
            if let Some(layer_id) = layer_id {
                child.insert(LayerEntity { layer_id });
            }
        }
    }
}

/// Render non-destructive terrain proposals as ghosted/tinted meshes over the
/// READ-ONLY [`TerrainHeightField`] (FR-5, NFR-1). Rebuilds only when the petal's
/// assignment revision changes (proposal edits round-trip through
/// `apply_terrain_assignments`). Tagged to the `ProposalOverlay` layer for
/// visibility; capped by `MAX_PROPOSAL_OVERLAYS` (NFR-2 interim). See
/// `src/AGENTS.md` §terrain-proposals.
#[allow(clippy::too_many_arguments)]
fn render_terrain_proposals(
    active: Res<ActivePetalTerrain>,
    layer_stack: Res<LayerStack>,
    height_field: Option<Res<TerrainHeightField>>,
    existing: Query<Entity, With<ProposalOverlayEntity>>,
    mut state: ResMut<ProposalRenderState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Revision-gated: skip unless the active assignment changed since last build.
    if state.last_revision == Some(active.revision) {
        return;
    }
    state.last_revision = Some(active.revision);

    // Sole despawner of proposal ghosts: clear the previous petal's set.
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }

    let Some(config) = active.config.as_ref().filter(|c| c.enabled) else {
        return;
    };
    if config.proposals.is_empty() {
        return;
    }

    let layer_id = find_layer(&layer_stack, |t| matches!(t, LayerType::ProposalOverlay));

    // READ-ONLY base sampler over the true heightfield — NEVER mutated (NFR-1).
    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));

    let mut remaining = MAX_PROPOSAL_OVERLAYS;
    for proposal in &config.proposals {
        // T3: earthwork regions bake into the chunk meshes (`bake_earthwork_regions`)
        // — never double-visualize records that pass the complete region predicate.
        if proposal.is_earthwork_region() {
            continue;
        }
        if remaining == 0 {
            tracing::warn!(
                cap = MAX_PROPOSAL_OVERLAYS,
                "proposal overlays saturated cap; truncating"
            );
            break;
        }
        let Some((positions, indices)) =
            crate::terrain_proposal::proposal_overlay_mesh_data(&base, proposal)
        else {
            continue;
        };
        remaining -= 1;

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_indices(Indices::U32(indices));

        let tint = fe_renderer::terrain_overlay::op_tint(proposal.op_snake());
        let material = materials.add(fe_renderer::terrain_overlay::ghost_material(tint));

        let mut entity = commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            fe_renderer::terrain_overlay::ProposalGhost,
            ProposalOverlayEntity {
                proposal_id: proposal.id.clone(),
            },
        ));
        if let Some(layer_id) = layer_id {
            entity.insert(LayerEntity { layer_id });
        }
    }
    // TODO(ultrapilot): route these spawns through the residency mesh budget
    // (`spawn_allowance` / `mesh_instance_watchdog`, NFR-2) — those live in the
    // fractalengine binary and are unreachable from fe-terrain. MAX_PROPOSAL_OVERLAYS
    // is the interim per-petal cap until that bridge exists.
    // TODO(ultrapilot): revision-gate means a relative-op ghost built before the
    // height field has coverage stays at its `target`/0.0 fallback; rebuild (or
    // re-ground) when terrain chunks arrive for the footprint. (Earthwork regions
    // do NOT share this gap: `bake_earthwork_regions` re-bakes late-arriving
    // chunks via the per-chunk `EarthworkBaked` marker.)
}

// ---------------------------------------------------------------------------
// T3 earthwork bake (sculpt_earthwork_regions_20260725 integration). Regions
// (proposal records carrying `material`) bake as VERTEX DELTAS into resident
// chunk meshes; the shared `TerrainHeightField` stays PRE-BAKE so it remains
// the pristine READ-ONLY base for deltas, volumes, and the Q-2 revert. See
// `src/AGENTS.md` §sculpt (Earthwork bake).
// ---------------------------------------------------------------------------

/// Volume-lattice resolution: each region's grid step = longest bound extent
/// divided by this (≈16k samples/region; O(step) tolerance, Q-4 planning-grade).
const EARTHWORK_LATTICE: f32 = 128.0;

/// One region pre-resolved for the per-vertex bake loop.
struct PreparedRegion {
    footprint: crate::sculpt::Footprint,
    bounds: Option<(f32, f32, f32, f32)>,
    op: crate::sculpt::SculptOp,
    target: f32,
    delta: f32,
    mean: Option<f32>,
    strength: f32,
    grid_step: f32,
}

/// Valid earthwork regions in the config's proposal block — the bake predicate
/// filter, parsed through the designated `sculpt::parse_regions`.
fn config_earthwork_regions(config: &TerrainConfig) -> Vec<crate::sculpt::EarthworkRegion> {
    let raw: Vec<serde_json::Value> = config
        .proposals
        .iter()
        .filter(|p| p.is_earthwork_region())
        .filter_map(|p| serde_json::to_value(p).ok())
        .collect();
    crate::sculpt::parse_regions(&serde_json::Value::Array(raw))
}

/// Grid step for a region's volume/mean lattice from its bounds (see
/// [`EARTHWORK_LATTICE`]); floored so a degenerate footprint can't stall.
fn region_grid_step(bounds: Option<(f32, f32, f32, f32)>) -> f32 {
    let Some((min_x, min_z, max_x, max_z)) = bounds else {
        return 1.0;
    };
    ((max_x - min_x).max(max_z - min_z) / EARTHWORK_LATTICE).max(1e-4)
}

/// Resolve footprint/params/mean once per region for the bake loop.
fn prepare_regions(
    regions: &[crate::sculpt::EarthworkRegion],
    base: &impl Fn(f32, f32) -> Option<f32>,
) -> Vec<PreparedRegion> {
    regions
        .iter()
        .map(|r| {
            let footprint = r.footprint_shape();
            let bounds = footprint.bounds();
            let grid_step = region_grid_step(bounds);
            let mean = if r.op == crate::sculpt::SculptOp::Smooth {
                crate::sculpt::region_mean_height(base, &footprint, grid_step)
            } else {
                None
            };
            PreparedRegion {
                footprint,
                bounds,
                op: r.op,
                target: r.target_height.unwrap_or(0.0),
                delta: if r.op == crate::sculpt::SculptOp::Smooth {
                    0.0
                } else {
                    r.delta.unwrap_or(0.0)
                },
                mean,
                strength: r.effective_strength(),
                grid_step,
            }
        })
        .collect()
}

/// XZ AABB overlap (touching counts).
fn bounds_intersect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// World-XZ AABB of a chunk's local vertex positions offset by its translation.
fn positions_world_xz_bounds(
    positions: &[[f32; 3]],
    translation: Vec3,
) -> Option<(f32, f32, f32, f32)> {
    let mut it = positions.iter();
    let first = it.next()?;
    let (mut min_x, mut min_z, mut max_x, mut max_z) = (first[0], first[2], first[0], first[2]);
    for p in it {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_z = min_z.min(p[2]);
        max_z = max_z.max(p[2]);
    }
    Some((
        min_x + translation.x,
        min_z + translation.z,
        max_x + translation.x,
        max_z + translation.z,
    ))
}

/// Net surface delta at `(x, z)` from applying every containing region in
/// record order over `base_y`.
fn earthwork_surface_delta(prepared: &[PreparedRegion], base_y: f32, x: f32, z: f32) -> f32 {
    let mut h = base_y;
    for r in prepared {
        if r.footprint.contains(x, z) {
            if let Some(p) =
                crate::sculpt::proposed_height(r.op, Some(h), r.target, r.delta, r.mean, r.strength)
            {
                h = p;
            }
        }
    }
    h - base_y
}

/// Smooth vertex normals for `positions` under `indices` (the `terrain_mesh`
/// accumulation, re-run after a bake so lighting follows the new relief).
fn recompute_smooth_normals(positions: &[[f32; 3]], indices: Option<&Indices>) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];
    let Some(indices) = indices else {
        return normals;
    };
    let idx: Vec<usize> = indices.iter().collect();
    for tri in idx.chunks_exact(3) {
        let [i0, i1, i2] = [tri[0], tri[1], tri[2]];
        if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
            continue;
        }
        let v0 = Vec3::from(positions[i0]);
        let v1 = Vec3::from(positions[i1]);
        let v2 = Vec3::from(positions[i2]);
        let face = (v1 - v0).cross(v2 - v0);
        for &i in &[i0, i1, i2] {
            normals[i][0] += face.x;
            normals[i][1] += face.y;
            normals[i][2] += face.z;
        }
    }
    for n in &mut normals {
        let v = Vec3::from(*n).normalize_or(Vec3::Y);
        *n = [v.x, v.y, v.z];
    }
    normals
}

/// Bake earthwork regions into resident chunk meshes and publish per-region
/// cut/fill volumes. Per-chunk `EarthworkBaked` markers gate work to (a) an
/// assignment-revision change (mirrors `render_terrain_proposals`) and (b)
/// late-arriving chunks; every bake restores the chunk's `PristineChunkHeights`
/// snapshot first, so edits/deletes revert cleanly (Q-2). The height field is
/// never written — it IS the pristine base. See `src/AGENTS.md` §sculpt
/// (Earthwork bake).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn bake_earthwork_regions(
    active: Res<ActivePetalTerrain>,
    height_field: Option<Res<TerrainHeightField>>,
    mut state: ResMut<EarthworkBakeState>,
    chunks: Query<
        (
            Entity,
            &Transform,
            &Mesh3d,
            Option<&EarthworkBaked>,
            Option<&PristineChunkHeights>,
        ),
        With<TerrainChunk>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    mut reports: MessageWriter<fe_renderer::terrain_overlay::EarthworkVolumeReport>,
) {
    let revision = active.revision;
    let revision_changed = state.last_revision != Some(revision);
    state.last_revision = Some(revision);

    let config = active.config.as_ref().filter(|c| c.enabled);
    let regions = config.map(config_earthwork_regions).unwrap_or_default();

    // READ-ONLY pristine base sampler (the field stays pre-bake — NFR-1).
    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));
    let prepared = prepare_regions(&regions, &base);

    if !regions.is_empty() && height_field.is_none() && state.warned_revision != Some(revision) {
        state.warned_revision = Some(revision);
        tracing::warn!(
            regions = regions.len(),
            "earthwork bake: no TerrainHeightField — regions persist but cannot bake (N-8)"
        );
    }

    let mut baked_any = false;
    for (entity, tx, mesh3d, baked, pristine) in chunks.iter() {
        if baked.is_some_and(|b| b.revision == revision) {
            continue; // up to date for this revision
        }
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            continue; // asset not ready — retry next frame (no marker)
        };
        let Some(raw) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
        else {
            commands.entity(entity).insert(EarthworkBaked { revision });
            continue;
        };
        // Working copy restored to pristine Y (the snapshot is the pre-bake truth).
        let mut positions: Vec<[f32; 3]> = raw.to_vec();
        if let Some(p) = pristine {
            if p.0.len() == positions.len() {
                for (v, y) in positions.iter_mut().zip(&p.0) {
                    v[1] = *y;
                }
            }
        }
        let chunk_bounds = positions_world_xz_bounds(&positions, tx.translation);
        let touches = chunk_bounds.is_some_and(|cb| {
            prepared
                .iter()
                .any(|r| r.bounds.is_some_and(|rb| bounds_intersect(cb, rb)))
        });

        if touches {
            let pristine_y: Vec<f32> = positions.iter().map(|v| v[1]).collect();
            let mut changed = false;
            for v in positions.iter_mut() {
                let wx = tx.translation.x + v[0];
                let wz = tx.translation.z + v[2];
                let Some(b) = base(wx, wz) else {
                    continue; // no pristine cover here — leave the vertex alone
                };
                let dh = earthwork_surface_delta(&prepared, b, wx, wz);
                if dh != 0.0 {
                    v[1] += dh;
                    changed = true;
                }
            }
            if changed || pristine.is_some() {
                if pristine.is_none() {
                    commands
                        .entity(entity)
                        .insert(PristineChunkHeights(pristine_y));
                }
                if let Some(mesh) = meshes.get_mut(&mesh3d.0) {
                    let normals = recompute_smooth_normals(&positions, mesh.indices());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
                    baked_any = true;
                }
            }
        } else if pristine.is_some() {
            // Regions edited/deleted away from this chunk — restore pristine relief.
            if let Some(mesh) = meshes.get_mut(&mesh3d.0) {
                let normals = recompute_smooth_normals(&positions, mesh.indices());
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
                baked_any = true;
            }
        }
        commands.entity(entity).insert(EarthworkBaked { revision });
    }

    // Publish real-unit volumes once per revision (and again as late chunks
    // extend field coverage — fe-ui's changed-value gate absorbs the re-fires).
    if (revision_changed || baked_any) && !regions.is_empty() {
        let petal_id = active.petal_id.clone().unwrap_or_default();
        let world_scale = config.map(|c| c.effective_world_scale()).unwrap_or(1.0);
        for (region, prep) in regions.iter().zip(&prepared) {
            let cf = region.cut_fill(&base, prep.grid_step, world_scale);
            reports.write(fe_renderer::terrain_overlay::EarthworkVolumeReport {
                petal_id: petal_id.clone(),
                region_id: region.id.clone(),
                cut_m3: cf.cut_m3,
                fill_m3: cf.fill_m3,
            });
        }
    }
}

/// Synchronize [`LayerStack`] visibility/opacity to layer-bound entities (on change only).
fn sync_layer_visibility(
    layer_stack: Res<LayerStack>,
    mut query: Query<(
        &LayerEntity,
        &mut Visibility,
        Option<&MeshMaterial3d<StandardMaterial>>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !layer_stack.is_changed() {
        return;
    }

    for (layer_entity, mut visibility, material_handle) in query.iter_mut() {
        let Some(layer) = layer_stack.get_layer(layer_entity.layer_id) else {
            continue;
        };

        *visibility = if layer.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        if let Some(mat_handle) = material_handle {
            if let Some(mat) = materials.get_mut(&mat_handle.0) {
                let c = mat.base_color.to_linear();
                mat.base_color = Color::linear_rgba(c.red, c.green, c.blue, layer.opacity);
                if layer.opacity < 1.0 {
                    mat.alpha_mode = AlphaMode::Blend;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- T3 earthwork bake helpers ---

    #[test]
    fn bounds_intersect_overlap_touch_and_miss() {
        let a = (0.0, 0.0, 10.0, 10.0);
        assert!(bounds_intersect(a, (5.0, 5.0, 15.0, 15.0)));
        assert!(
            bounds_intersect(a, (10.0, 0.0, 20.0, 10.0)),
            "touching counts"
        );
        assert!(!bounds_intersect(a, (10.1, 0.0, 20.0, 10.0)));
        assert!(!bounds_intersect(a, (0.0, -20.0, 10.0, -10.1)));
    }

    #[test]
    fn region_grid_step_scales_with_extent_and_floors() {
        // 128-unit extent → step 1; tiny/degenerate extents floor, never stall.
        assert!((region_grid_step(Some((0.0, 0.0, 128.0, 64.0))) - 1.0).abs() < 1e-6);
        assert!(region_grid_step(Some((0.0, 0.0, 0.0, 0.0))) >= 1e-4);
        assert_eq!(region_grid_step(None), 1.0);
    }

    #[test]
    fn positions_world_xz_bounds_offsets_by_translation() {
        let pos = [[0.0, 5.0, 0.0], [10.0, 7.0, 20.0], [-2.0, 1.0, 3.0]];
        let b = positions_world_xz_bounds(&pos, Vec3::new(100.0, 0.0, -50.0)).unwrap();
        assert_eq!(b, (98.0, -50.0, 110.0, -30.0));
        assert!(positions_world_xz_bounds(&[], Vec3::ZERO).is_none());
    }

    #[test]
    fn earthwork_surface_delta_applies_regions_in_order() {
        use crate::sculpt::{EarthworkRegion, SculptOp};
        let base = |_x: f32, _z: f32| Some(1.0);
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let regions = vec![
            EarthworkRegion {
                id: "r1".into(),
                op: SculptOp::Raise,
                footprint: square.clone(),
                material: "earth".into(),
                target_height: None,
                delta: Some(2.0),
            },
            EarthworkRegion {
                id: "r2".into(),
                op: SculptOp::Level,
                footprint: square,
                material: "earth".into(),
                target_height: Some(0.5),
                delta: None,
            },
        ];
        let prepared = prepare_regions(&regions, &base);
        // Inside: raise 1→3, then level to 0.5 → net delta -0.5 off base 1.0.
        assert!((earthwork_surface_delta(&prepared, 1.0, 5.0, 5.0) + 0.5).abs() < 1e-5);
        // Outside every footprint: no delta.
        assert_eq!(earthwork_surface_delta(&prepared, 1.0, 50.0, 50.0), 0.0);
    }

    #[test]
    fn config_earthwork_regions_filters_on_material_predicate() {
        use crate::terrain_proposal::{TerrainOp, TerrainProposal};
        let mut cfg = TerrainConfig::default();
        cfg.proposals.push(TerrainProposal {
            id: "p1".into(),
            op: TerrainOp::Raise,
            footprint: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            target_height: None,
            delta: Some(1.0),
            material: None, // plain proposal — ghosts, never bakes
        });
        cfg.proposals.push(TerrainProposal {
            id: "r1".into(),
            op: TerrainOp::Level,
            footprint: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]],
            target_height: Some(3.0),
            delta: None,
            material: Some("gravel".into()), // earthwork region — bakes, never ghosts
        });
        let regions = config_earthwork_regions(&cfg);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].id, "r1");
        assert_eq!(regions[0].material, "gravel");
        assert_eq!(regions[0].op, crate::sculpt::SculptOp::Level);
    }

    #[test]
    fn recompute_smooth_normals_flat_grid_points_up() {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        let indices = Indices::U32(vec![0, 2, 1, 1, 2, 3]);
        let normals = recompute_smooth_normals(&positions, Some(&indices));
        for n in &normals {
            assert!(
                (n[1] - 1.0).abs() < 1e-5,
                "flat grid normal must be +Y: {n:?}"
            );
        }
        // No indices → safe default normals, same length.
        assert_eq!(recompute_smooth_normals(&positions, None).len(), 4);
    }

    #[test]
    fn resample_height_grid_passthrough_when_small() {
        let grid = vec![1.0, 2.0, 3.0, 4.0];
        let (out, w, h) = resample_height_grid(&grid, 2, 2, 65);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, grid);
    }

    #[test]
    fn resample_height_grid_preserves_corners() {
        // 5x5 ramp grid downsampled to 3x3 keeps exact corner values.
        let mut grid = Vec::new();
        for r in 0..5 {
            for c in 0..5 {
                grid.push((r * 10 + c) as f32);
            }
        }
        let (out, w, h) = resample_height_grid(&grid, 5, 5, 3);
        assert_eq!((w, h), (3, 3));
        assert!((out[0] - grid[0]).abs() < 1e-4);
        assert!((out[2] - grid[4]).abs() < 1e-4);
        assert!((out[6] - grid[20]).abs() < 1e-4);
        assert!((out[8] - grid[24]).abs() < 1e-4);
        // Center of a bilinear ramp interpolates exactly.
        assert!((out[4] - grid[12]).abs() < 1e-4);
    }

    #[test]
    fn resample_height_grid_guards_degenerate_input() {
        let grid = vec![1.0, 2.0, 3.0];
        let (out, w, h) = resample_height_grid(&grid, 3, 1, 2);
        assert_eq!((w, h), (3, 1));
        assert_eq!(out, grid);
    }

    #[test]
    fn feature_budget_grants_up_to_remaining_and_saturates() {
        let mut remaining = 10;
        assert_eq!(take_feature_budget(&mut remaining, 4), 4);
        assert_eq!(take_feature_budget(&mut remaining, 100), 6);
        assert_eq!(remaining, 0);
        assert_eq!(take_feature_budget(&mut remaining, 5), 0);
    }

    #[test]
    fn feature_budget_zero_request_is_noop() {
        let mut remaining = MAX_GEOJSON_FEATURES;
        assert_eq!(take_feature_budget(&mut remaining, 0), 0);
        assert_eq!(remaining, MAX_GEOJSON_FEATURES);
    }

    #[test]
    fn clamp_upsample_keeps_standard_tile_at_full_factor() {
        // 256px tile at U=4 → 1021² = 1,042,441 ≤ budget: unchanged.
        assert_eq!(clamp_upsample_for_grid(256, 256, 4, MAX_TILE_VERTICES), 4);
    }

    #[test]
    fn clamp_upsample_reduces_large_tile_factor() {
        // 512px tile: U=4 → 2045² ≈ 4.18M > budget; U=2 → 1023² ≈ 1.05M fits.
        assert_eq!(clamp_upsample_for_grid(512, 512, 4, MAX_TILE_VERTICES), 2);
        // 1024px tile: only U=1 (1024² ≈ 1.05M) fits.
        assert_eq!(clamp_upsample_for_grid(1024, 1024, 4, MAX_TILE_VERTICES), 1);
    }

    #[test]
    fn clamp_upsample_degenerate_inputs_never_zero() {
        assert_eq!(clamp_upsample_for_grid(2, 2, 0, MAX_TILE_VERTICES), 1);
        assert_eq!(clamp_upsample_for_grid(1, 1, 4, MAX_TILE_VERTICES), 4);
        // Even an absurd budget floors at U=1 rather than 0.
        assert_eq!(clamp_upsample_for_grid(4096, 4096, 4, 16), 1);
    }

    #[test]
    fn tile_dimension_cap_always_fits_vertex_budget_at_unit_upsample() {
        // The dimension guard + U=1 floor keep max-size tiles within budget.
        let max = MAX_TILE_DIMENSION as usize;
        assert!(max * max <= MAX_TILE_VERTICES);
    }

    #[test]
    fn desired_zoom_monotonic_in_height() {
        let mut prev = desired_zoom(1.0, 0, 20);
        for h in [10.0, 100.0, 1_000.0, 10_000.0, 100_000.0, 1_000_000.0] {
            let z = desired_zoom(h, 0, 20);
            assert!(z <= prev, "zoom must not increase with height");
            prev = z;
        }
    }

    #[test]
    fn desired_zoom_clamped_to_range() {
        assert_eq!(desired_zoom(0.0, 10, 15), 15);
        assert_eq!(desired_zoom(1.0, 10, 15), 15);
        assert_eq!(desired_zoom(1e9, 10, 15), 10);
        // Swapped bounds normalize.
        assert_eq!(desired_zoom(1e9, 15, 10), 10);
    }

    #[test]
    fn tile_world_size_equator_zoom0() {
        let s = tile_world_size_m(0.0, 0);
        assert!((s - 40_075_016.686).abs() < 1.0);
    }

    #[test]
    fn tile_world_size_monotonic_in_zoom() {
        let mut prev = tile_world_size_m(45.0, 0);
        for z in 1..=18u8 {
            let v = tile_world_size_m(45.0, z);
            assert!(v < prev, "tile size must shrink as zoom grows");
            prev = v;
        }
    }

    #[test]
    fn flip_rows_reverses_row_order() {
        let v = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2 cols x 3 rows
        assert_eq!(flip_rows(&v, 2, 3), vec![5.0, 6.0, 3.0, 4.0, 1.0, 2.0]);
    }

    #[test]
    fn scaled_mesh_size_shrinks_tile_with_scale() {
        let real = tile_world_size_m(45.0, 8);
        assert_eq!(scaled_tile_size(real, 1.0), real);
        assert!((scaled_tile_size(real, 0.001) - real * 0.001).abs() < 1e-6);
    }

    #[test]
    fn inverse_camera_mapping_recovers_real_height() {
        // A camera at world-Y 500 under 0.001 scale sits at 500 km real → low zoom.
        let scale = 0.001;
        let world_y = 500.0_f64;
        let real = world_to_real_height(world_y, scale);
        assert!((real - 500_000.0).abs() < 1e-3);
        let z_scaled = desired_zoom(real as f32, 8, 15);
        let z_unscaled = desired_zoom(world_y as f32, 8, 15);
        assert!(
            z_scaled < z_unscaled,
            "inverting scale must pick a lower zoom"
        );
    }
}
