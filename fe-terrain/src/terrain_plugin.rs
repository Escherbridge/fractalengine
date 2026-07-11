//! Terrain rendering plugin (render feature); see `src/AGENTS.md` §terrain_plugin.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::prelude::Projection as CameraProjection;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::config::{ElevationSourceKind, TerrainConfig};
use crate::iot::TrackRouteMap;
use crate::layers::{LayerId, LayerStack, LayerType};
use crate::lod_ring::{
    max_ring_radius_for_budget, ring_offsets, ring_radius_tiles, spawn_despawn_radii,
    upsample_factor_for_height, view_radius_world, wrong_zoom_replacement_present,
    DESPAWN_HYSTERESIS, MAX_UPSAMPLE, VIEW_RADIUS_FACTOR, ZOOM_BASE_HEIGHT_M,
};
use crate::mesh::interp::upsample_bilinear;
use crate::mesh::terrain::terrain_mesh;
use crate::petal_binding::{
    apply_terrain_assignments, ActivePetalTerrain, ActiveTileSource, TerrainAssignmentMsg,
};
use crate::projection::Projection;
use crate::scale::{scale_elevations, scale_local, scaled_tile_size, world_to_real_height};
use crate::tiles::{
    decode_png_pixels, CompositeTileSource, ElevationDecoder, TerrainRgbDecoder, TerrariumDecoder,
    TileCoord,
};

/// Max chunks spawned per frame so a large adaptive ring fills without hitching.
const MAX_SPAWNS_PER_FRAME: usize = 16;

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
            .init_resource::<TrackRouteMap>()
            .init_resource::<ActivePetalTerrain>()
            .init_resource::<ActiveTileSource>()
            .init_resource::<FailedTiles>()
            .insert_resource(LayerStack::new())
            .add_message::<TerrainAssignmentMsg>()
            .add_systems(
                Update,
                (
                    apply_terrain_assignments,
                    update_terrain_lod,
                    fetch_and_spawn_terrain_chunks,
                    render_gpx_tracks,
                    render_waypoint_markers,
                    render_geojson_overlays,
                    sync_layer_visibility,
                )
                    .chain(),
            )
            .add_systems(Update, crate::iot::animation::advance_track_animations);
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
        let coord = TileCoord::new(chunk.tile_coords.1, chunk.tile_coords.2, chunk.tile_coords.0);
        let stale_zoom = desired
            .is_some_and(|z| wrong_zoom_replacement_present(coord, z, &existing));
        if too_far || stale_zoom {
            commands.entity(entity).despawn();
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

    let elevation_mesh = elevation_png.and_then(|png| match decode_png_pixels(&png) {
        Ok((pixels, w, h)) if w > 1 && h > 1 => {
            let decoded = match config.elevation_source {
                ElevationSourceKind::TerrainRgb => TerrainRgbDecoder.decode(&pixels, w, h),
                ElevationSourceKind::Terrarium => TerrariumDecoder.decode(&pixels, w, h),
                ElevationSourceKind::None => vec![0.0; (w * h) as usize],
            };
            // Densify before scaling so interpolation stays in real meters and the
            // world-Y invariant `(ele - origin_ele) * scale` is preserved.
            let (grid, gw, gh) = upsample_bilinear(&decoded, w as usize, h as usize, upsample);
            // Scale heights to world units (origin_ele subtraction lands in the anchor).
            let scaled = scale_elevations(&grid, scale as f32);
            let flipped = flip_rows(&scaled, gw, gh);
            Some(terrain_mesh(&flipped, gw as u32, gh as u32, tile_size))
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

    let mesh = match elevation_mesh {
        Some(m) => m,
        None if satellite_bytes.is_some() => {
            // Flat 16x16 grid fallback so the satellite texture still renders.
            terrain_mesh(&vec![0.0f32; 16 * 16], 16, 16, tile_size)
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
            Some(Image::new(
                Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba.into_raw(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            ))
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to decode satellite tile image");
            None
        }
    }
}

/// Render GPX track overlays as line meshes; skips non-finite points.
fn render_gpx_tracks(
    track_query: Query<(Entity, &GpxTrackLine), Without<Mesh3d>>,
    route_map: Res<TrackRouteMap>,
    layer_stack: Res<LayerStack>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, track) in track_query.iter() {
        let Some(route) = route_map.routes.get(&track.track_node_id) else {
            continue;
        };

        let positions: Vec<Vec3> = route
            .points
            .iter()
            .map(|p| {
                Vec3::new(
                    p.position[0] as f32,
                    p.position[1] as f32,
                    p.position[2] as f32,
                )
            })
            .filter(|v| v.is_finite())
            .collect();

        if positions.len() < 2 {
            continue;
        }

        let mut line_mesh = Mesh::new(
            bevy::render::render_resource::PrimitiveTopology::LineStrip,
            RenderAssetUsages::default(),
        );
        line_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);

        let handle = meshes.add(line_mesh);
        let material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.0, 0.8, 1.0),
            ..default()
        });

        let mut e = commands.entity(entity);
        e.insert((Mesh3d(handle), MeshMaterial3d(material)));

        let layer_id = find_layer(&layer_stack, |t| {
            matches!(t, LayerType::GpxTrack { node_id, .. } if node_id == &track.track_node_id)
        });
        if let Some(layer_id) = layer_id {
            e.insert(LayerEntity { layer_id });
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
        commands.entity(entity).insert((
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
        commands.entity(entity).insert(GeoJsonProcessed);

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

        let layer_id = find_layer(&layer_stack, |t| {
            matches!(t, LayerType::GeoJsonOverlay { source_path } if source_path == &overlay.source_path)
        });

        for polygon in &result.polygon_meshes {
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

            let mut child = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat), GeoJsonProcessed));
            if let Some(layer_id) = layer_id {
                child.insert(LayerEntity { layer_id });
            }
        }

        for line in &result.polyline_meshes {
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

            let mut child = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat), GeoJsonProcessed));
            if let Some(layer_id) = layer_id {
                child.insert(LayerEntity { layer_id });
            }
        }

        for marker in &result.marker_positions {
            let marker_mesh = meshes.add(Sphere::new(0.3));
            let marker_mat = materials.add(StandardMaterial::from(Color::srgba(
                marker.color[0],
                marker.color[1],
                marker.color[2],
                marker.color[3],
            )));

            let mut child = commands.spawn((
                Mesh3d(marker_mesh),
                MeshMaterial3d(marker_mat),
                Transform::from_xyz(
                    marker.position[0],
                    marker.position[1],
                    marker.position[2],
                ),
                Pickable::default(),
                GeoJsonProcessed,
            ));
            if let Some(layer_id) = layer_id {
                child.insert(LayerEntity { layer_id });
            }
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
        assert!(z_scaled < z_unscaled, "inverting scale must pick a lower zoom");
    }
}
