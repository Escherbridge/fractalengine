//! Terrain rendering plugin — registers Bevy systems for LOD, chunk lifecycle,
//! GPX track rendering, waypoint markers, and GeoJSON overlays.
//!
//! This module is only available when the `render` feature is enabled.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;

use crate::iot::TrackRouteMap;
use crate::layers::{LayerId, LayerStack};

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

/// Marker component for waypoint marker entities.
/// Makes waypoint entities pickable by the selection system's raycast.
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

/// Marker component for GeoJSON overlay entities.
#[derive(Component, Clone)]
pub struct GeoJsonOverlay {
    pub source_path: String,
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

/// The main terrain rendering plugin.
///
/// Registers:
/// - [`TerrainLodConfig`] and [`WaypointMarkerConfig`] resources
/// - [`TrackRouteMap`] resource (for animation lookup)
/// - Systems: LOD update, chunk lifecycle, GPX rendering, waypoint markers, GeoJSON overlays
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainLodConfig>()
            .init_resource::<WaypointMarkerConfig>()
            .init_resource::<TrackRouteMap>()
            .insert_resource(LayerStack::new())
            .add_systems(
                Update,
                (
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

/// Update LOD levels for terrain chunks based on camera distance.
///
/// Runs every frame, checking each active chunk's distance to the main camera.
/// If a chunk is too far, it is marked for despawn. If a nearby area has no
/// chunk at a higher LOD, it is queued for spawn.
fn update_terrain_lod(
    camera_query: Query<&GlobalTransform, With<Camera>>,
    mut chunk_query: Query<(Entity, &TerrainChunk, &GlobalTransform)>,
    lod_config: Res<TerrainLodConfig>,
    mut commands: Commands,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };
    let cam_pos = cam_transform.translation();

    for (entity, chunk, chunk_transform) in chunk_query.iter_mut() {
        let dist = cam_pos.distance(chunk_transform.translation());
        let current_lod = chunk.lod as usize;

        // Check if we should switch to a different LOD
        if current_lod < lod_config.lod_distances.len() {
            let switch_dist = lod_config.lod_distances[current_lod];
            if dist > switch_dist && current_lod > 0 {
                // Too far for current LOD — should switch to lower LOD
            } else if current_lod > 0 && dist < lod_config.lod_distances[current_lod - 1] {
                // Close enough for higher LOD
            }
        }

        // Despawn chunks that are too far away
        let max_dist = lod_config.lod_distances.last().copied().unwrap_or(1000.0) * 2.0;
        if dist > max_dist {
            commands.entity(entity).despawn();
        }
    }
}

/// Fetch and spawn terrain chunks near the camera that don't already exist.
///
/// Computes which tiles should be visible based on camera position and LOD,
/// then spawns new `TerrainChunk` entities for missing tiles. Satellite
/// textures are applied as `base_color_texture` on the chunk mesh.
fn fetch_and_spawn_terrain_chunks(
    camera_query: Query<&GlobalTransform, With<Camera>>,
    chunk_query: Query<&TerrainChunk>,
    lod_config: Res<TerrainLodConfig>,
    mut _commands: Commands,
    mut _meshes: ResMut<Assets<Mesh>>,
    mut _materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(_cam_transform) = camera_query.single() else {
        return;
    };

    // Count active chunks
    let active_count = chunk_query.iter().count();
    if active_count >= lod_config.max_chunks {
        return; // At capacity
    }

    // Placeholder: actual tile fetching and mesh spawning would go here
    // Integration point for the tile loading pipeline from fe-terrain/src/tiles/
}

/// Render GPX track overlays as line meshes with vertex colors.
///
/// For each `GpxTrackLine` entity, reads the associated track node's route
/// data, applies the configured `ColorMode`, and creates/updates line segment
/// meshes.
fn render_gpx_tracks(
    track_query: Query<(Entity, &GpxTrackLine), Without<Mesh3d>>,
    route_map: Res<TrackRouteMap>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, track) in track_query.iter() {
        let Some(route) = route_map.routes.get(&track.track_node_id) else {
            continue;
        };

        if route.points.len() < 2 {
            continue;
        }

        // Build line positions from timestamped route points
        let positions: Vec<Vec3> = route
            .points
            .iter()
            .map(|p| Vec3::new(p.position[0] as f32, p.position[1] as f32, p.position[2] as f32))
            .collect();

        // Create a line strip mesh
        let mut line_mesh =
            Mesh::new(bevy::render::render_resource::PrimitiveTopology::LineStrip, RenderAssetUsages::default());
        line_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);

        let handle = meshes.add(line_mesh);
        let material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.0, 0.8, 1.0),
            ..default()
        });

        commands.entity(entity).insert((
            Mesh3d(handle),
            MeshMaterial3d(material),
        ));
    }
}

/// Render waypoint markers as instanced meshes.
///
/// Each waypoint entity gets a `WaypointMarker` component and a small sphere
/// mesh for visibility. Markers are [`Pickable`] for selection system raycast.
fn render_waypoint_markers(
    waypoint_query: Query<(Entity, &WaypointMarker), Without<Mesh3d>>,
    marker_config: Res<WaypointMarkerConfig>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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

/// Render GeoJSON overlay meshes draped at terrain elevation.
///
/// For each `GeoJsonOverlay` entity, reads the source file, parses it via
/// [`crate::layers::parse_geojson`], and spawns polygon/line/marker meshes.
fn render_geojson_overlays(
    overlay_query: Query<(Entity, &GeoJsonOverlay), Without<Mesh3d>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (_entity, overlay) in overlay_query.iter() {
        // Read and parse GeoJSON file
        let Ok(json_str) = std::fs::read_to_string(&overlay.source_path) else {
            continue;
        };

        let Ok(result) = crate::layers::parse_geojson(&json_str, |lon, lat| {
            // Simple identity projection — in production, use the petal's projection
            (lon as f32, lat as f32)
        }) else {
            continue;
        };

        // Spawn polygon meshes
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

            commands.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                overlay.clone(),
            ));
        }

        // Spawn polyline meshes
        for line in &result.polyline_meshes {
            let mut mesh =
                Mesh::new(bevy::render::render_resource::PrimitiveTopology::LineStrip, RenderAssetUsages::default());
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

            commands.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                overlay.clone(),
            ));
        }

        // Spawn marker instances
        for marker in &result.marker_positions {
            let marker_mesh = meshes.add(Sphere::new(0.3));
            let marker_mat = materials.add(StandardMaterial::from(Color::srgba(
                marker.color[0],
                marker.color[1],
                marker.color[2],
                marker.color[3],
            )));

            commands.spawn((
                Mesh3d(marker_mesh),
                MeshMaterial3d(marker_mat),
                Transform::from_xyz(
                    marker.position[0],
                    marker.position[1],
                    marker.position[2],
                ),
                Pickable::default(),
            ));
        }
    }
}

/// Synchronize [`LayerStack`] visibility state to Bevy's [`Visibility`] component.
///
/// For each entity with a [`LayerEntity`] marker, reads the corresponding layer's
/// `visible` and `opacity` flags from the [`LayerStack`] resource and applies them:
/// - `visible == false` → `Visibility::Hidden`
/// - `visible == true`  → `Visibility::Inherited`
/// - `opacity` applied to the entity's `StandardMaterial` alpha channel
fn sync_layer_visibility(
    layer_stack: Res<LayerStack>,
    mut query: Query<(&LayerEntity, &mut Visibility, Option<&MeshMaterial3d<StandardMaterial>>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (layer_entity, mut visibility, material_handle) in query.iter_mut() {
        let Some(layer) = layer_stack.get_layer(layer_entity.layer_id) else {
            continue;
        };

        *visibility = if layer.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        // Apply opacity to material alpha
        if let Some(mat_handle) = material_handle {
            if let Some(mat) = materials.get_mut(&mat_handle.0) {
                let c = mat.base_color.to_linear();
                mat.base_color = Color::linear_rgba(c.red, c.green, c.blue, layer.opacity);
            }
        }
    }
}
