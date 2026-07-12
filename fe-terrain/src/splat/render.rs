//! Splat rendering + view-mode plugin (render feature); see `src/splat/AGENTS.md`.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::config::{ElevationSourceKind, TerrainConfig};
use crate::layers::LayerStack;
use crate::lod_ring::{covering_tiles, splat_desired_zoom};
use crate::petal_binding::{ActivePetalTerrain, ActiveTileSource};
use crate::projection::Projection;
use crate::scale::{scale_local, scaled_tile_size, world_to_real_height};
use crate::splat::synth::{synthesize_splats, SplatBuffer, TileSatellite};
use crate::splat::view_mode::TerrainViewMode;
use crate::terrain_plugin::{tile_world_size_m, LayerEntity, TerrainChunk};
use crate::tiles::{
    decode_png_pixels, CompositeTileSource, ElevationDecoder, TerrainRgbDecoder, TerrariumDecoder,
    TileCoord,
};

/// Max splat sub-tile meshes baked per frame so a full ring / zoom-in fills without a hitch.
const MAX_SPLAT_SPAWNS_PER_FRAME: usize = 16;
/// Edge length (texels) of the shared soft-disc alpha texture.
const DISC_TEX_SIZE: u32 = 64;
/// Flat fallback grid resolution when a tile has satellite but no elevation.
const FLAT_GRID: usize = 64;

/// The terrain splat rendering + view-mode plugin; see `src/splat/AGENTS.md`.
pub struct SplatPlugin;

impl Plugin for SplatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainViewMode>()
            .init_resource::<SplatConfig>()
            .add_message::<TerrainViewModeMsg>()
            .add_systems(Startup, init_splat_assets)
            .add_systems(
                Update,
                (
                    apply_view_mode_msgs,
                    reconcile_splat_chunks,
                    apply_view_mode_visibility,
                )
                    .chain(),
            );
    }
}

/// Message carrying a freshly parsed view mode (bridged from the petal terrain JSON).
#[derive(Message, Debug, Clone)]
pub struct TerrainViewModeMsg(pub TerrainViewMode);

/// Shared, procedurally-built splat material + soft-disc texture (one draw material for all tiles).
#[derive(Resource, Clone)]
pub struct SplatAssets {
    pub material: Handle<StandardMaterial>,
    pub disc: Handle<Image>,
}

/// Tunables for splat synthesis + hybrid switching.
#[derive(Resource, Clone)]
pub struct SplatConfig {
    /// Decimation stride passed to synthesis (`1` = per-texel, `2` = the perf-target default).
    pub stride: usize,
    /// Real-meter camera distance below which Hybrid keeps mesh; scaled by `world_scale`.
    pub hybrid_mesh_distance_m: f64,
}

impl Default for SplatConfig {
    fn default() -> Self {
        Self {
            stride: 2,
            hybrid_mesh_distance_m: 3000.0,
        }
    }
}

/// Marks a baked splat chunk entity; shadows a [`TerrainChunk`] by tile coords.
/// Owns its higher-zoom splat sub-tiles as child mesh entities (FR-2), so one mesh
/// chunk can map to many splat sub-meshes rather than 1:1.
#[derive(Component)]
pub struct SplatChunk {
    /// Tile coordinates (zoom, x, y) of the shadowed mesh chunk.
    pub coords: (u8, u32, u32),
    /// Splat-specific zoom the child sub-tiles were baked at (FR-1); may exceed `coords.0`.
    pub splat_zoom: u8,
}

/// Build the shared soft-disc texture + splat material once at startup.
fn init_splat_assets(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let disc = images.add(make_soft_disc_image(DISC_TEX_SIZE));
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(disc.clone()),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        // Draped quads may face away from the camera; keep both sides so no disc vanishes.
        cull_mode: None,
        ..default()
    });
    commands.insert_resource(SplatAssets { material, disc });
}

/// Apply the last queued view-mode message to the resource (only when it changes).
fn apply_view_mode_msgs(
    mut reader: MessageReader<TerrainViewModeMsg>,
    mut mode: ResMut<TerrainViewMode>,
) {
    if let Some(TerrainViewModeMsg(m)) = reader.read().last() {
        if *mode != *m {
            *mode = *m;
        }
    }
}

/// Spawn/despawn splat chunks to shadow the [`TerrainChunk`] set (budgeted per frame).
#[allow(clippy::too_many_arguments)]
fn reconcile_splat_chunks(
    active: Res<ActivePetalTerrain>,
    tile_source: Res<ActiveTileSource>,
    view_mode: Res<TerrainViewMode>,
    splat_cfg: Res<SplatConfig>,
    assets: Option<Res<SplatAssets>>,
    camera: Query<&GlobalTransform, With<Camera>>,
    terrain_chunks: Query<(&TerrainChunk, &Transform), Without<SplatChunk>>,
    splat_chunks: Query<(Entity, &SplatChunk)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    // Splats off (mesh-only mode, or no enabled terrain) → drop any that exist.
    let want_splats = active.config.as_ref().is_some_and(|c| c.enabled)
        && !matches!(*view_mode, TerrainViewMode::Mesh);
    if !want_splats {
        for (e, _) in splat_chunks.iter() {
            commands.entity(e).despawn();
        }
        return;
    }
    let (Some(config), Some(assets)) = (active.config.as_ref(), assets) else {
        return;
    };
    let Some(composite) = tile_source.composite.as_ref() else {
        return;
    };
    let Some(projection) = tile_source.projection.as_ref() else {
        return;
    };

    let scale = config.effective_world_scale();
    let cam_real_height = camera
        .single()
        .ok()
        .map(|t| world_to_real_height(t.translation().y as f64, scale) as f32)
        .unwrap_or(f32::MAX);

    let terrain_set: HashSet<(u8, u32, u32)> =
        terrain_chunks.iter().map(|(c, _)| c.tile_coords).collect();

    // Per mesh chunk, the splat zoom the camera currently wants (FR-1/FR-5 clamped).
    let wanted_zoom = |mesh_zoom: u8| {
        splat_desired_zoom(cam_real_height, mesh_zoom, config.min_zoom, config.max_zoom)
    };

    // SPLAT_LOD_DEBUG: throttled (~1/s) trace of camera height vs. the zoom the
    // splat pipeline picks, so zooming in is visible in the log. STRIP AFTER QA.
    splat_lod_debug(cam_real_height, config.max_zoom, &terrain_set, &wanted_zoom);

    // Despawn splats whose mesh chunk is gone, or whose baked splat zoom is now stale (FR-4).
    let mut existing: HashSet<(u8, u32, u32)> = HashSet::new();
    for (e, s) in splat_chunks.iter() {
        if !terrain_set.contains(&s.coords) || s.splat_zoom != wanted_zoom(s.coords.0) {
            commands.entity(e).despawn();
        } else {
            existing.insert(s.coords);
        }
    }

    // Shadow-spawn splats for mesh chunks lacking a current-zoom one; each chunk owns a Vec
    // of higher-zoom sub-tile child meshes (FR-2/FR-3) anchored within the parent transform.
    let mut spawned = 0usize;
    for (chunk, transform) in terrain_chunks.iter() {
        if spawned >= MAX_SPLAT_SPAWNS_PER_FRAME {
            break;
        }
        if existing.contains(&chunk.tile_coords) {
            continue;
        }
        let target_zoom = wanted_zoom(chunk.tile_coords.0);
        let parent = TileCoord::new(chunk.tile_coords.1, chunk.tile_coords.2, chunk.tile_coords.0);

        // Build every covering sub-tile mesh first; only commit the chunk if at least one
        // baked (hole-free per chunk — a stale chunk already despawned, so a bare frame is
        // acceptable, but an empty chunk entity is not).
        let mut children: Vec<(Handle<Mesh>, Vec3)> = Vec::new();
        for (sz, sx, sy) in covering_tiles(parent, target_zoom) {
            let sub = TileCoord::new(sx, sy, sz);
            let Some(mesh) =
                build_tile_splat_mesh((sz, sx, sy), target_zoom, config, composite, splat_cfg.stride)
            else {
                continue;
            };
            let Some(offset) = sub_tile_local_offset(sub, transform.translation, projection, scale)
            else {
                continue;
            };
            children.push((meshes.add(mesh), offset));
        }
        if children.is_empty() {
            continue;
        }
        spawned += children.len();

        let parent_entity = commands
            .spawn((
                *transform,
                Visibility::default(),
                SplatChunk {
                    coords: chunk.tile_coords,
                    splat_zoom: target_zoom,
                },
            ))
            .id();
        for (mesh, offset) in children {
            commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(assets.material.clone()),
                Transform::from_translation(offset),
                ChildOf(parent_entity),
            ));
        }
    }
}

/// SPLAT_LOD_DEBUG: throttled (~1/s) diagnostic of camera height vs. chosen splat
/// zoom. Temporary — remove after the close-range densification is verified.
fn splat_lod_debug(
    cam_real_height: f32,
    max_zoom: u8,
    terrain_set: &HashSet<(u8, u32, u32)>,
    wanted_zoom: &impl Fn(u8) -> u8,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static LAST_MS: AtomicU64 = AtomicU64::new(0);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last) < 1000 {
        return;
    }
    LAST_MS.store(now_ms, Ordering::Relaxed);
    let mesh_zoom = terrain_set.iter().map(|c| c.0).min();
    let splat_zoom = mesh_zoom.map(wanted_zoom);
    tracing::info!(
        target: "SPLAT_LOD_DEBUG",
        cam_real_height_m = cam_real_height,
        mesh_zoom = ?mesh_zoom,
        splat_zoom = ?splat_zoom,
        max_zoom,
        "splat lod: camera height -> chosen splat zoom (coverage-fill closes residual holes)"
    );
}

/// Local offset of a sub-tile's SW corner within its parent chunk's transform frame.
/// `parent_world` is the parent chunk's world anchor; sub-tile meshes are baked in tile-local
/// space, so this offset re-anchors each sub-tile to its true corner.
fn sub_tile_local_offset(
    sub: TileCoord,
    parent_world: Vec3,
    projection: &Projection,
    scale: f64,
) -> Option<Vec3> {
    // South-west corner lat/lon of the sub-tile (NW of the tile one row south).
    let (_nw_lat, nw_lon) = sub.to_lat_lon();
    let (s_lat, _) = TileCoord::new(sub.x, sub.y.saturating_add(1), sub.zoom).to_lat_lon();
    let anchor = projection.wgs84_to_local(s_lat, nw_lon, 0.0).ok()?;
    let a = scale_local(anchor, scale);
    Some(Vec3::new(
        a[0] as f32 - parent_world.x,
        a[1] as f32 - parent_world.y,
        a[2] as f32 - parent_world.z,
    ))
}

/// Drive mesh + splat entity [`Visibility`] from the view mode; Hybrid switches by distance.
///
/// Mesh chunks compose mode with their layer's visibility (so layer toggling still
/// works); splat chunks are governed by mode alone.
#[allow(clippy::type_complexity)]
fn apply_view_mode_visibility(
    view_mode: Res<TerrainViewMode>,
    active: Res<ActivePetalTerrain>,
    splat_cfg: Res<SplatConfig>,
    layer_stack: Res<LayerStack>,
    camera: Query<&GlobalTransform, With<Camera>>,
    mut mesh_chunks: Query<
        (&GlobalTransform, Option<&LayerEntity>, &mut Visibility),
        (With<TerrainChunk>, Without<SplatChunk>),
    >,
    mut splat_chunks: Query<(&GlobalTransform, &mut Visibility), (With<SplatChunk>, Without<TerrainChunk>)>,
) {
    let scale = active
        .config
        .as_ref()
        .filter(|c| c.enabled)
        .map(|c| c.effective_world_scale())
        .unwrap_or(1.0);
    let threshold = (splat_cfg.hybrid_mesh_distance_m * scale) as f32;
    let cam_pos = camera.single().ok().map(|t| t.translation());

    let layer_visible = |le: Option<&LayerEntity>| -> bool {
        le.map(|l| {
            layer_stack
                .get_layer(l.layer_id)
                .map(|m| m.visible)
                .unwrap_or(true)
        })
        .unwrap_or(true)
    };

    for (xf, layer, vis) in mesh_chunks.iter_mut() {
        let mode_wants_mesh = match *view_mode {
            TerrainViewMode::Mesh => true,
            TerrainViewMode::Splats => false,
            TerrainViewMode::Hybrid => cam_pos
                .map(|c| c.distance(xf.translation()) <= threshold)
                .unwrap_or(true),
        };
        set_vis(vis, mode_wants_mesh && layer_visible(layer));
    }

    for (xf, vis) in splat_chunks.iter_mut() {
        let show = match *view_mode {
            TerrainViewMode::Mesh => false,
            TerrainViewMode::Splats => true,
            TerrainViewMode::Hybrid => cam_pos
                .map(|c| c.distance(xf.translation()) > threshold)
                .unwrap_or(false),
        };
        set_vis(vis, show);
    }
}

/// Set visibility only when it differs; reads via `Deref` so unchanged frames
/// never trip change detection (the `DerefMut` write happens only on a real flip).
fn set_vis(mut vis: Mut<Visibility>, show: bool) {
    let want = if show {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if *vis != want {
        *vis = want;
    }
}

/// Re-fetch a tile's data and bake all its splats into one mesh; `None` when unusable.
fn build_tile_splat_mesh(
    coords: (u8, u32, u32),
    _requested_zoom: u8,
    config: &TerrainConfig,
    composite: &CompositeTileSource,
    stride: usize,
) -> Option<Mesh> {
    let scale = config.effective_world_scale();
    let coord = TileCoord::new(coords.1, coords.2, coords.0);
    let (nw_lat, _nw_lon) = coord.to_lat_lon();
    let (s_lat, _) = TileCoord::new(coord.x, coord.y.saturating_add(1), coord.zoom).to_lat_lon();
    let tile_size = scaled_tile_size(tile_world_size_m((nw_lat + s_lat) / 2.0, coord.zoom), scale);

    // Decode elevation (real meters, north-up) if present.
    let elevation = composite
        .get_tile_sync(coord)
        .and_then(|png| decode_png_pixels(&png).ok())
        .and_then(|(px, w, h)| {
            if w > 1 && h > 1 {
                let dec = match config.elevation_source {
                    ElevationSourceKind::TerrainRgb => TerrainRgbDecoder.decode(&px, w, h),
                    ElevationSourceKind::Terrarium => TerrariumDecoder.decode(&px, w, h),
                    ElevationSourceKind::None => vec![0.0; (w * h) as usize],
                };
                Some((dec, w as usize, h as usize))
            } else {
                None
            }
        });

    // Satellite RGBA (north-up; synth samples by decoded row directly, no flip).
    let satellite = composite
        .get_satellite_tile_sync(coord)
        .and_then(|bytes| image::load_from_memory(&bytes).ok())
        .map(|img| img.to_rgba8());

    // No elevation but satellite present → flat colored splat sheet.
    let (elevations, w, h) = match elevation {
        Some(e) => e,
        None if satellite.is_some() => (vec![0.0f32; FLAT_GRID * FLAT_GRID], FLAT_GRID, FLAT_GRID),
        None => return None,
    };

    let sat_ref = satellite.as_ref().map(|s| TileSatellite {
        rgba: s.as_raw(),
        width: s.width() as usize,
        height: s.height() as usize,
    });

    let buffer = synthesize_splats(&elevations, w, h, sat_ref, tile_size, scale as f32, stride);
    if buffer.is_empty() {
        return None;
    }
    // Coverage-driven hole fill: close any visible background gaps between splats
    // (jitter + irregular spacing leave them at every zoom), sizing each new splat
    // to its neighbor gap and shrinking as the field densifies. Runs whenever holes
    // exist, independent of the zoom ceiling. The requested zoom still selects the
    // real sub-tile resolution at the call site (higher zoom = real higher-res
    // data), complementary to filling residual holes at whatever res was baked.
    let buffer =
        crate::splat::interpolate::augment_splat_buffer_coverage(&buffer).unwrap_or(buffer);
    Some(bake_splat_mesh(&buffer))
}

/// Bake a [`SplatBuffer`] into a single triangle-list mesh: one normal-oriented,
/// vertex-colored quad per splat, UV-mapped to the shared soft-disc texture.
pub fn bake_splat_mesh(buf: &SplatBuffer) -> Mesh {
    let n = buf.len();
    let mut positions = Vec::with_capacity(n * 4);
    let mut normals = Vec::with_capacity(n * 4);
    let mut colors = Vec::with_capacity(n * 4);
    let mut uvs = Vec::with_capacity(n * 4);
    let mut indices = Vec::with_capacity(n * 6);

    for i in 0..n {
        let p = Vec3::from(buf.positions[i]);
        let nrm = Vec3::from(buf.normals[i]).normalize_or(Vec3::Y);
        let [major, minor] = buf.scales[i];

        // Major axis = steepest-descent (downhill) direction in the splat plane,
        // derived from the normal alone. Flat splats have no downhill → any in-plane axis.
        let down = Vec3::NEG_Y;
        let mut t = down - down.dot(nrm) * nrm;
        if t.length_squared() < 1e-8 {
            t = nrm.cross(Vec3::X);
            if t.length_squared() < 1e-8 {
                t = nrm.cross(Vec3::Z);
            }
        }
        let t = t.normalize_or(Vec3::X);
        let b = nrm.cross(t).normalize_or(Vec3::Z);
        let tm = t * major;
        let bm = b * minor;

        let base = positions.len() as u32;
        let corners = [
            (p - tm - bm, [0.0f32, 0.0]),
            (p + tm - bm, [1.0, 0.0]),
            (p + tm + bm, [1.0, 1.0]),
            (p - tm + bm, [0.0, 1.0]),
        ];
        for (corner, uv) in corners {
            positions.push([corner.x, corner.y, corner.z]);
            normals.push([nrm.x, nrm.y, nrm.z]);
            colors.push(buf.colors[i]);
            uvs.push(uv);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Build a radial soft-disc alpha texture: white RGB, alpha falling smoothly to 0 at the edge.
pub fn make_soft_disc_image(size: u32) -> Image {
    let mut data = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let r_max = c.max(1.0);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let r = (dx * dx + dy * dy).sqrt() / r_max; // 0 center .. ~1 edge
            let a = (1.0 - r).clamp(0.0, 1.0);
            let a = a * a; // quadratic falloff → soft edge
            let idx = ((y * size + x) * 4) as usize;
            data[idx] = 255;
            data[idx + 1] = 255;
            data[idx + 2] = 255;
            data[idx + 3] = (a * 255.0) as u8;
        }
    }
    Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_emits_quad_per_splat() {
        let buf = SplatBuffer {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            colors: vec![[1.0, 1.0, 1.0, 1.0]; 2],
            scales: vec![[1.0, 1.0], [2.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        };
        let mesh = bake_splat_mesh(&buf);
        assert_eq!(mesh.count_vertices(), 8); // 4 verts * 2 splats
        assert_eq!(mesh.indices().map(|i| i.len()).unwrap_or(0), 12); // 6 * 2
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_some());
    }

    #[test]
    fn empty_buffer_bakes_empty_mesh() {
        let mesh = bake_splat_mesh(&SplatBuffer::default());
        assert_eq!(mesh.count_vertices(), 0);
    }

    #[test]
    fn soft_disc_center_more_opaque_than_edge() {
        let img = make_soft_disc_image(16);
        let data = img.data.as_ref().expect("disc has cpu data");
        let size = 16usize;
        let center = ((size / 2) * size + size / 2) * 4;
        let corner = 0usize;
        assert!(data[center + 3] > data[corner + 3], "center alpha must exceed corner");
        assert_eq!(data.len(), size * size * 4);
    }
}
