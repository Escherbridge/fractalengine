//! Terrain rendering plugin (render feature); see `src/AGENTS.md` §terrain_plugin.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::Indices;
use bevy::prelude::Projection as CameraProjection;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat};

use crate::config::{ElevationSourceKind, TerrainConfig};
use crate::delta_viz::DeltaLattice;
use crate::iot::{TrackRouteMap, TrackStyleMap};
use crate::layers::{LayerId, LayerStack, LayerType};
use crate::lod_ring::{
    DESPAWN_HYSTERESIS, FocusReach, MAX_STREAMING_FOCI, MAX_UPSAMPLE, VIEW_RADIUS_FACTOR,
    ZOOM_BASE_HEIGHT_M, capture_chunk_allowance, chunk_retained_by_any_focus,
    max_ring_radius_for_budget, nearest_focus_index, ring_offsets, ring_radius_tiles,
    spawn_despawn_radii, stale_for_nearest_focus, upsample_factor_for_height, view_radius_world,
};
use crate::mesh::interp::upsample_bilinear;
use crate::mesh::terrain::terrain_mesh;
use crate::mesh::track::{
    ColorMode as TrackColorMode, PreparedTrackZone, prepare_track_zone, track_centroid, track_mesh,
    track_zone_mesh_prepared,
};
use crate::petal_binding::{
    ActivePetalTerrain, ActiveTileSource, TerrainAssignmentMsg, apply_terrain_assignments,
    mean_tile_elevation,
};
use crate::projection::Projection;
use crate::scale::{scale_elevations, scale_local, scaled_tile_size, world_to_real_height};
use crate::sculpt::{EARTHWORK_LATTICE, lattice_grid_step, region_grid_step};
use crate::tiles::{
    ElevationDecoder, LoadedTile, Residency, TerrainRgbDecoder, TerrariumDecoder, TileBundleDemand,
    TileCoord, TileResidency, decode_png_pixels,
};
use fe_renderer::camera::{CaptureCamera, StreamingFocus};
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

/// Hard per-petal cap on proposal ghost meshes (NFR-2); the live allowance
/// arrives via [`ProposalOverlayBudget`]. See `src/AGENTS.md` §terrain-proposals.
pub const MAX_PROPOSAL_OVERLAYS: usize = 4_096;

/// Per-petal cap on TOTAL cut/fill heatmap drape vertices (NFR-2): committed
/// proposal drapes + baked-region drapes share it via [`HeatmapVertexUsage`].
/// A drape that would cross it is skipped with a warning (the ghost stays).
pub const MAX_HEATMAP_VERTICES_PER_PETAL: usize = 1_048_576;

/// Live-preview debounce (OD-3, FR-3): the preview upgrades to its full
/// lattice once the armed tool's draft has been unchanged for this long.
pub const PREVIEW_DEBOUNCE_SECS: f64 = 0.1;

/// Coarse lattice resolution (OD-3, NFR-3) sampled WHILE the preview draft is
/// still changing; [`EARTHWORK_LATTICE`] is the settled "full" resolution —
/// committed proposals, baked regions and the preview all share that policy.
const PREVIEW_LATTICE_COARSE: f32 = 64.0;

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

/// A GPX ribbon its own style hides (`gis.track.visible = false`): a layer
/// toggle must never un-hide it (`sync_layer_visibility`).
#[derive(Component)]
pub struct StyleHidden;

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

/// Exact petal-local source and prepared geometry used to build one track mesh.
#[derive(Component, Debug, Clone)]
pub struct RenderedTrackGeometry {
    source_points: Vec<crate::iot::animation::TimestampedRoutePoint>,
    style: crate::iot::TrackStyle,
    positions: Vec<[f32; 3]>,
    zone: Option<PreparedTrackZone>,
    centroid: [f32; 3],
    mesh_id: Option<bevy::asset::AssetId<Mesh>>,
}

impl RenderedTrackGeometry {
    pub fn prepare(
        points: &[crate::iot::animation::TimestampedRoutePoint],
        style: crate::iot::TrackStyle,
    ) -> Option<Self> {
        let positions: Vec<_> =
            crate::mesh::curve::flatten_route(points, crate::mesh::curve::SAMPLES_PER_SEGMENT)
                .into_iter()
                .filter(|point| point.iter().all(|value| value.is_finite()))
                .collect();
        if positions.len() < 2 {
            return None;
        }
        Some(Self {
            centroid: track_centroid(&positions),
            zone: style.closed.then(|| prepare_track_zone(&positions)),
            positions,
            source_points: points.to_vec(),
            style,
            mesh_id: None,
        })
    }

    pub fn source_points(&self) -> &[crate::iot::animation::TimestampedRoutePoint] {
        &self.source_points
    }
    pub fn style(&self) -> crate::iot::TrackStyle {
        self.style
    }
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }
    pub fn zone(&self) -> Option<&PreparedTrackZone> {
        self.zone.as_ref()
    }
    pub fn centroid(&self) -> [f32; 3] {
        self.centroid
    }
    pub fn matches_mesh(&self, mesh: &Mesh3d) -> bool {
        self.mesh_id == Some(mesh.0.id())
    }
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

/// Bookkeeping for the last proposal-ghost build: the revision it served, which
/// proposals it built, how many footprint vertices were grounded, and whether
/// the budget truncated it. See `src/AGENTS.md` §terrain-proposals.
#[derive(Resource, Default)]
pub struct ProposalRenderState {
    last_revision: Option<u64>,
    /// Indices into `config.proposals` of the ghosts currently resident.
    built: Vec<usize>,
    /// Some built ghost used the no-base fallback height (awaits terrain).
    ungrounded: bool,
    /// Grounded footprint vertices across `built` at build time.
    grounded_vertices: usize,
    /// The build stopped early at the overlay allowance.
    truncated: bool,
}

/// Mesh-residency allowance for NEW proposal ghosts (NFR-2): the headroom under
/// the app's mesh-instance ceiling, written by the binary's budget bridge.
/// Defaults to [`MAX_PROPOSAL_OVERLAYS`] when no bridge runs.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProposalOverlayBudget {
    pub allowance: usize,
}

impl Default for ProposalOverlayBudget {
    fn default() -> Self {
        Self {
            allowance: MAX_PROPOSAL_OVERLAYS,
        }
    }
}

/// Heatmap drape sub-toggle (OD-8/OD-10); lives in the fe-renderer seam so fe-ui can bind it.
pub use fe_renderer::terrain_overlay::HeatmapOverlayToggle;

/// Marker for a committed proposal's FR-2 heatmap drape; own marker/owner
/// (delta-viz tripwire) — `render_terrain_proposals` despawns it alongside
/// the matching ghost, never mixed into `apply_terrain_assignments`'s query.
#[derive(Component, Clone)]
pub struct ProposalHeatmapEntity {
    pub proposal_id: String,
}

/// Marker for the FR-4 baked-region heatmap drape. Sole owner/despawner:
/// [`render_baked_region_heatmaps`].
#[derive(Component, Clone)]
pub struct BakedRegionHeatmapEntity {
    pub region_id: String,
}

/// Rebuild bookkeeping for [`render_baked_region_heatmaps`] (mirrors
/// [`ProposalRenderState`]'s revision / re-ground / budget-growth gates).
#[derive(Resource, Default)]
pub struct BakedHeatmapRenderState {
    last_revision: Option<u64>,
    /// Drapes currently resident (counts against [`ProposalOverlayBudget`]).
    built: usize,
    /// Region footprint vertices with a pristine base sample at build time.
    grounded_vertices: usize,
    /// Some region footprint lacked base cover (awaits terrain chunks).
    ungrounded: bool,
    /// The build stopped early at the overlay allowance or vertex cap.
    truncated: bool,
}

/// Resident heatmap drape vertices per system, so committed-proposal and
/// baked-region drapes together stay under [`MAX_HEATMAP_VERTICES_PER_PETAL`].
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct HeatmapVertexUsage {
    pub committed: usize,
    pub baked: usize,
}

/// Marker for a live FR-3 preview entity (ghost or heatmap drape). Sole
/// owner/despawner: [`render_proposal_preview`] — never mixed into
/// `render_terrain_proposals`'s or `apply_terrain_assignments`'s despawn
/// queries (delta-viz tripwire).
#[derive(Component)]
pub struct ProposalPreviewEntity;

/// Chunk marker: earthwork regions were last baked into this chunk's mesh at
/// `revision` (T3; see `src/AGENTS.md` §sculpt (Earthwork bake)).
#[derive(Component)]
pub struct EarthworkBaked {
    pub revision: u64,
    pub height_generation: Option<u64>,
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
    last_height_generation: Option<u64>,
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
///
/// Only "absent from every covering index" or "undecodable" lands here; a tile
/// whose load is still in flight is pending, never failed (§tile-residency).
#[derive(Resource, Default)]
pub struct FailedTiles {
    revision: u64,
    tiles: HashSet<(u8, u32, u32)>,
}

impl FailedTiles {
    /// Whether `coord` is recorded as failed for the current revision.
    pub fn contains(&self, coord: TileCoord) -> bool {
        self.tiles.contains(&(coord.zoom, coord.x, coord.y))
    }

    /// Number of failed tiles recorded for the current revision.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// True when no tile has failed under the current revision.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// Ring residency of the active capture camera, published every frame by
/// [`fetch_and_spawn_terrain_chunks`] for the agent-vision settle; see
/// `src/AGENTS.md` §terrain_plugin "Multi-focus streaming".
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFocusResidency {
    /// The capture camera this count describes (`None`: no capture focus this frame).
    pub capture_focus: Option<Entity>,
    /// Its ring tiles still outstanding: loading, awaiting a streamed bundle,
    /// awaiting origin grounding, or deferred by the chunk/spawn budget.
    pub pending: usize,
}

/// Streaming-focus cameras (plus plain cameras, for the lone-camera fallback),
/// read by the LOD and fetch passes.
type StreamingFocusQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static CameraProjection,
        Has<CaptureCamera>,
        Has<StreamingFocus>,
    ),
    Or<(With<StreamingFocus>, With<Camera>)>,
>;

/// One streaming focus sampled for this frame.
#[derive(Clone, Copy)]
struct FocusSample {
    entity: Entity,
    position: Vec3,
    far_world: Option<f64>,
    capture: bool,
}

/// At most [`MAX_STREAMING_FOCI`] foci in a fixed array (no per-frame
/// allocation): slot 0 = the capture focus, slot 1 = the main focus.
struct StreamingFoci {
    slots: [Option<FocusSample>; MAX_STREAMING_FOCI],
}

impl StreamingFoci {
    /// First capture focus and first non-capture focus; extra foci are ignored.
    /// With no `StreamingFocus` anywhere, a lone camera is the main focus (a
    /// host without fe-renderer's orbit camera keeps its pre-V4 streaming).
    fn collect(query: &StreamingFocusQuery) -> Self {
        let mut slots = [None; MAX_STREAMING_FOCI];
        let mut lone_camera = None;
        let mut plain_cameras = 0usize;
        for (entity, transform, projection, capture, focus) in query.iter() {
            let sample = FocusSample {
                entity,
                position: transform.translation(),
                far_world: perspective_far_world(projection),
                capture,
            };
            if !focus {
                plain_cameras += 1;
                lone_camera = Some(FocusSample {
                    capture: false,
                    ..sample
                });
                continue;
            }
            let slot = &mut slots[usize::from(!capture)];
            if slot.is_none() {
                *slot = Some(sample);
            }
        }
        if slots.iter().all(Option::is_none) && plain_cameras == 1 {
            slots[1] = lone_camera;
        }
        Self { slots }
    }

    /// Foci in slot order: the capture focus first while one exists (the
    /// order `reaches` indexes, so a capture's reach is index 0).
    fn iter(&self) -> impl Iterator<Item = &FocusSample> {
        self.slots.iter().flatten()
    }

    /// Foci in fetch order: the user's view first, so a capture never takes
    /// the per-frame spawn cap from it.
    fn fetch_order(&self) -> impl Iterator<Item = &FocusSample> {
        self.slots.iter().rev().flatten()
    }

    fn capture(&self) -> Option<&FocusSample> {
        self.slots[0].as_ref()
    }

    fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Per-focus LOD reach in streaming order, plus how many slots are filled.
    fn reaches(
        &self,
        lod_config: &TerrainLodConfig,
        config: Option<&TerrainConfig>,
        scale: f64,
    ) -> ([FocusReach; MAX_STREAMING_FOCI], usize) {
        let mut reaches = [FocusReach::default(); MAX_STREAMING_FOCI];
        let mut len = 0;
        for sample in self.iter() {
            reaches[len] = focus_reach(sample, lod_config, config, scale);
            len += 1;
        }
        (reaches, len)
    }
}

/// A focus's desired zoom and hysteresis despawn radius (world units).
fn focus_reach(
    sample: &FocusSample,
    lod_config: &TerrainLodConfig,
    config: Option<&TerrainConfig>,
    scale: f64,
) -> FocusReach {
    let cam_pos = sample.position;
    // Scale-aware: reason about zoom in real meters and despawn distances in
    // world units (chunk transforms are scaled); see AGENTS.md §scale.
    let desired = config.map(|c| {
        desired_zoom(
            world_to_real_height(cam_pos.y as f64, scale) as f32,
            c.min_zoom,
            c.max_zoom,
        )
    });
    // Despawn radius follows the visible frustum with hysteresis so tiles never
    // flicker or leave holes while still in view; floored to ~2 tiles + the LOD
    // base so big low-zoom tiles don't churn.
    let far_world = sample
        .far_world
        .unwrap_or_else(|| (cam_pos.y as f64).abs() * VIEW_RADIUS_FACTOR);
    let view_r = view_radius_world(cam_pos.y as f64, far_world, VIEW_RADIUS_FACTOR);
    let base_max = lod_config.lod_distances.last().copied().unwrap_or(1000.0) as f64 * 2.0;
    let tile_world_equator = scaled_tile_size(tile_world_size_m(0.0, desired.unwrap_or(0)), scale);
    let (_spawn_r, despawn_r) =
        spawn_despawn_radii(view_r.max(base_max), tile_world_equator, DESPAWN_HYSTERESIS);
    FocusReach {
        position: cam_pos.to_array(),
        despawn_radius: despawn_r,
        desired_zoom: desired,
    }
}

/// The main terrain rendering plugin; see `src/AGENTS.md` §terrain_plugin.
pub struct TerrainPlugin;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
/// Ordering boundary for host assignment producers.
pub enum TerrainSystemSet {
    ApplyAssignments,
}

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
            .init_resource::<TileResidency>()
            .init_resource::<CaptureFocusResidency>()
            // S7 (streamed tilesets): the tile half of the D-74 pointer-set —
            // see `fetch_and_spawn_terrain_chunks` and
            // `src/AGENTS.md` §streamed-tilesets.
            .init_resource::<TileBundleDemand>()
            .init_resource::<ProposalRenderState>()
            .init_resource::<ProposalOverlayBudget>()
            .init_resource::<HeatmapOverlayToggle>()
            .init_resource::<EarthworkBakeState>()
            .init_resource::<fe_renderer::terrain_overlay::EarthworkVolumeState>()
            .init_resource::<BakedHeatmapRenderState>()
            .init_resource::<HeatmapVertexUsage>()
            .insert_resource(LayerStack::new())
            .add_message::<TerrainAssignmentMsg>()
            // Idempotent: fe-ui's plugin also registers these shared-seam messages.
            .add_message::<fe_renderer::terrain_overlay::EarthworkVolumeReport>()
            .add_message::<fe_renderer::terrain_overlay::ProposalPreviewReport>()
            // P2 S5 volume-report request/reply seam (idempotent with fe-ui's registration).
            .add_message::<fe_renderer::terrain_overlay::VolumeReportRequest>()
            .add_message::<fe_renderer::terrain_overlay::VolumeReportResult>()
            .add_systems(
                Update,
                (
                    apply_terrain_assignments.in_set(TerrainSystemSet::ApplyAssignments),
                    update_terrain_lod,
                    fetch_and_spawn_terrain_chunks,
                    render_gpx_tracks,
                    render_waypoint_markers,
                    render_geojson_overlays,
                    render_terrain_proposals,
                    bake_earthwork_regions,
                    render_baked_region_heatmaps,
                    render_proposal_preview,
                    answer_volume_report_requests,
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

/// Despawn chunks beyond EVERY streaming focus's hysteresis radius, or at a
/// stale zoom for their nearest focus once its replacement fully covers them
/// (hole-free), releasing their tile bytes (exit-ring eviction); see AGENTS.md
/// §terrain_plugin "Multi-focus streaming", §tile-residency.
pub fn update_terrain_lod(
    foci_query: StreamingFocusQuery,
    chunk_query: Query<(Entity, &TerrainChunk, &GlobalTransform)>,
    lod_config: Res<TerrainLodConfig>,
    active: Res<ActivePetalTerrain>,
    tile_source: Res<ActiveTileSource>,
    mut commands: Commands,
    mut height_field: Option<ResMut<TerrainHeightField>>,
) {
    let foci = StreamingFoci::collect(&foci_query);
    if foci.is_empty() {
        return;
    }
    let config = active.config.as_ref().filter(|c| c.enabled);
    let scale = config.map(|c| c.effective_world_scale()).unwrap_or(1.0);
    let (reaches, focus_count) = foci.reaches(&lod_config, config, scale);
    let reaches = &reaches[..focus_count];

    let existing: HashSet<(u8, u32, u32)> =
        chunk_query.iter().map(|(_, c, _)| c.tile_coords).collect();

    for (entity, chunk, chunk_transform) in chunk_query.iter() {
        let chunk_pos = chunk_transform.translation().to_array();
        let coord = TileCoord::new(
            chunk.tile_coords.1,
            chunk.tile_coords.2,
            chunk.tile_coords.0,
        );
        let too_far = !chunk_retained_by_any_focus(chunk_pos, reaches);
        let stale_zoom = stale_for_nearest_focus(coord, chunk_pos, reaches, &existing);
        if too_far || stale_zoom {
            commands.entity(entity).despawn();
            if let Some(field) = height_field.as_mut() {
                field.remove(&chunk.tile_coords);
            }
            if let Some(composite) = tile_source.composite.as_ref() {
                composite.release_tile(coord);
            }
        }
    }
}

/// Offline-first chunk spawning: per streaming focus (capture focus first), an
/// adaptive tile ring (frustum-sized, nearest-first) at that focus's desired zoom,
/// fed by async tile residency (pending tiles are skipped, never failed); see
/// AGENTS.md §terrain_plugin ("Multi-focus streaming"), §tile-residency.
#[allow(clippy::too_many_arguments)]
pub fn fetch_and_spawn_terrain_chunks(
    foci_query: StreamingFocusQuery,
    chunk_query: Query<&TerrainChunk>,
    lod_config: Res<TerrainLodConfig>,
    mut active: ResMut<ActivePetalTerrain>,
    mut tile_source: ResMut<ActiveTileSource>,
    mut residency: ResMut<TileResidency>,
    mut bundle_demand: ResMut<TileBundleDemand>,
    layer_stack: Res<LayerStack>,
    mut failed: ResMut<FailedTiles>,
    mut capture_residency: Option<ResMut<CaptureFocusResidency>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut height_field: Option<ResMut<TerrainHeightField>>,
) {
    residency.poll();
    // S7: rebuilt fresh every frame from what the ring actually wants below
    // (`record_bundle_demand`) — never stale demand from a position the
    // camera has since left.
    bundle_demand.clear();
    let foci = StreamingFoci::collect(&foci_query);
    // No terrain to stream means nothing for a capture to wait on: 0 pending.
    let fresh_residency = CaptureFocusResidency {
        capture_focus: foci.capture().map(|focus| focus.entity),
        pending: 0,
    };
    if let Some(published) = capture_residency.as_mut() {
        if **published != fresh_residency {
            **published = fresh_residency;
        }
    }
    let Some(elevation_source) = active
        .config
        .as_ref()
        .filter(|c| c.enabled)
        .map(|c| c.elevation_source.clone())
    else {
        return;
    };
    if tile_source.composite.is_none() || tile_source.projection.is_none() {
        return;
    }
    if foci.is_empty() {
        return;
    }

    if failed.revision != active.revision {
        failed.tiles.clear();
        failed.revision = active.revision;
    }
    if residency.revision() != active.revision {
        // Assignment ran without a residency resource in reach: start clean, ungrounded.
        residency.reset(active.revision, None);
    }

    // Ground the projection origin from the center tile before any chunk spawns,
    // so every chunk anchors against the same origin_ele (§tile-residency).
    if let Some(center) = residency.grounding() {
        let Some(composite) = tile_source.composite.as_ref() else {
            return;
        };
        let elevation = match residency.ensure_resident(center, composite) {
            Residency::Pending => {
                if foci.capture().is_some() {
                    // The grounding tile gates every chunk, the capture's included.
                    if let Some(published) = capture_residency.as_mut() {
                        published.pending = 1;
                    }
                }
                return;
            }
            Residency::Absent => None,
            Residency::Ready(tile) => {
                let elevation = tile
                    .elevation
                    .as_ref()
                    .and_then(|hit| mean_tile_elevation(&hit.bytes, &elevation_source));
                residency.put_back(center, tile);
                elevation
            }
        };
        residency.finish_grounding();
        if let Some(ele) = elevation {
            tracing::info!(
                origin_ele = ele,
                "grounded terrain origin from center-tile elevation"
            );
            if let Some(config) = active.config.as_mut() {
                config.origin.origin_ele = ele;
            }
            if let Some(projection) = tile_source.projection.as_mut() {
                projection.origin_ele = ele;
            }
        }
    }

    let (Some(config), Some(composite), Some(projection)) = (
        active.config.as_ref(),
        tile_source.composite.as_ref(),
        tile_source.projection.as_ref(),
    ) else {
        return;
    };

    let active_count = chunk_query.iter().count();
    let capture_allowance = if foci.capture().is_some() {
        capture_chunk_allowance(lod_config.max_chunks)
    } else {
        0
    };
    // One focus at budget: nothing to do (the pre-multi-focus early-out).
    if capture_allowance == 0 && active_count >= lod_config.max_chunks {
        return;
    }

    // Invert world scale so each camera is reasoned about in real meters (zoom
    // selection + projection expect real-world coordinates); see AGENTS.md §scale.
    let scale = config.effective_world_scale();
    let (reaches, focus_count) = foci.reaches(&lod_config, Some(config), scale);
    let reaches = &reaches[..focus_count];
    let contested = focus_count > 1;
    let existing: HashSet<(u8, u32, u32)> = chunk_query.iter().map(|c| c.tile_coords).collect();
    // Separate budgets (§terrain_plugin "Budget"): a capture holds the chunks
    // nearest it, up to its allowance; the user's view keeps `max_chunks` of
    // the rest, so a capture can never pause the view's fill.
    let capture_held = if foci.capture().is_some() {
        chunk_query
            .iter()
            .filter(|chunk| {
                let (z, x, y) = chunk.tile_coords;
                chunk_anchor_world(TileCoord::new(x, y, z), projection, scale)
                    .is_ok_and(|anchor| nearest_focus_index(anchor.to_array(), reaches) == Some(0))
            })
            .count()
    } else {
        0
    };
    // Chunks held per focus kind: [user view, capture].
    let mut held = [active_count - capture_held, capture_held];
    let mut spawned_this_frame = 0usize;

    for focus in foci.fetch_order() {
        let slot = usize::from(focus.capture);
        let budget = if focus.capture {
            capture_allowance
        } else {
            lod_config.max_chunks
        };
        let cam_pos = focus.position;
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
        let far_world = focus
            .far_world
            .unwrap_or_else(|| (cam_pos.y as f64).abs() * VIEW_RADIUS_FACTOR);
        let view_r = view_radius_world(cam_pos.y as f64, far_world, VIEW_RADIUS_FACTOR);
        let scaled_tile = scaled_tile_size(tile_world_size_m(lat, zoom), scale);
        let max_radius = max_ring_radius_for_budget(budget);
        let radius = ring_radius_tiles(view_r, scaled_tile, max_radius);

        // Close-range upsampling: once the camera descends below the served max zoom's
        // natural height, densify decoded elevations so geometry stays smooth up close.
        let upsample = if zoom >= config.max_zoom {
            upsample_factor_for_height(cam_real_height as f64, ZOOM_BASE_HEIGHT_M, MAX_UPSAMPLE)
        } else {
            1
        };

        let center = TileCoord::from_lat_lon(lat, lon, zoom);
        let tiles_per_axis = TileCoord::tiles_at_zoom(zoom) as i64;
        let mut pending = 0usize;

        // Nearest-first so the camera's immediate surroundings fill before the edges.
        for (dx, dy) in ring_offsets(radius) {
            let budget_spent = held[slot] >= budget || spawned_this_frame >= MAX_SPAWNS_PER_FRAME;
            // The main focus just stops; a capture keeps walking to count what it still awaits.
            if budget_spent && !focus.capture {
                break;
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
            // Never spawn what the LOD pass would evict next frame: a tile nearer
            // another focus that already covers it at that focus's zoom (no churn).
            if contested
                && chunk_anchor_world(coord, projection, scale).is_ok_and(|anchor| {
                    stale_for_nearest_focus(coord, anchor.to_array(), reaches, &existing)
                })
            {
                continue;
            }
            if composite.streamed_pending(coord) {
                // S7 (streamed tilesets): an index resolution or a bundle fetch
                // is still in flight for whatever would cover this coordinate —
                // demand it, but skip WITHOUT touching `residency`/`failed`, so
                // it is retried fresh every frame and never recorded as a
                // failure (src/AGENTS.md §streamed-tilesets).
                composite.record_bundle_demand(coord, &mut bundle_demand);
                pending += 1;
                continue;
            }
            if budget_spent {
                pending += 1;
                continue;
            }

            let tile = match residency.ensure_resident(coord, composite) {
                Residency::Pending => {
                    pending += 1;
                    continue;
                }
                Residency::Absent => {
                    failed.tiles.insert(key);
                    continue;
                }
                Residency::Ready(tile) => tile,
            };
            if spawn_chunk(
                coord,
                &tile,
                config,
                projection,
                &layer_stack,
                upsample,
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut images,
                height_field.as_deref_mut(),
            ) {
                // Live chunk: its bytes stay resident until the exit ring releases them.
                composite.pin_tile(coord, &tile);
                held[slot] += 1;
                spawned_this_frame += 1;
            } else {
                failed.tiles.insert(key);
            }
        }
        if let Some(published) = capture_residency.as_mut().filter(|_| focus.capture) {
            published.pending = pending;
        }
    }
}

/// World-space SW-corner anchor of `coord`'s chunk — exactly the `Transform`
/// [`spawn_chunk`] gives it, so spawn- and LOD-side focus distances agree.
fn chunk_anchor_world(
    coord: TileCoord,
    projection: &Projection,
    scale: f64,
) -> Result<Vec3, crate::projection::ProjectionError> {
    let (_, nw_lon) = coord.to_lat_lon();
    let (s_lat, _) = TileCoord::new(coord.x, coord.y.saturating_add(1), coord.zoom).to_lat_lon();
    let anchor = scale_local(projection.wgs84_to_local(s_lat, nw_lon, 0.0)?, scale);
    Ok(Vec3::new(
        anchor[0] as f32,
        anchor[1] as f32,
        anchor[2] as f32,
    ))
}

/// Build + spawn one terrain chunk entity; returns false when no data was usable.
///
/// `upsample` (≥1) bilinearly densifies decoded elevations for close-range detail.
#[allow(clippy::too_many_arguments)]
fn spawn_chunk(
    coord: TileCoord,
    tile: &LoadedTile,
    config: &TerrainConfig,
    projection: &Projection,
    layer_stack: &LayerStack,
    upsample: u32,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    height_field: Option<&mut TerrainHeightField>,
) -> bool {
    let elevation_png = tile.elevation.as_ref().map(|hit| &hit.bytes);
    let satellite_bytes = tile.satellite.as_ref().map(|hit| &hit.bytes);
    if elevation_png.is_none() && satellite_bytes.is_none() {
        tracing::warn!(tile = %coord.cache_key(), "no elevation or satellite tile available; skipping");
        return false;
    }

    // Tile geometry: SW-corner anchor, mesh +x = east, +z = north (rows flipped).
    // World scale shrinks/grows the whole tile (size, heights, anchor) so several
    // scales of space are viewable; see `src/AGENTS.md` §scale.
    let scale = config.effective_world_scale();
    let (nw_lat, _) = coord.to_lat_lon();
    let (s_lat, _) = TileCoord::new(coord.x, coord.y.saturating_add(1), coord.zoom).to_lat_lon();
    let tile_size = scaled_tile_size(tile_world_size_m((nw_lat + s_lat) / 2.0, coord.zoom), scale);

    let elevation_data = elevation_png.and_then(|png| match decode_png_pixels(png) {
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
    let material = match satellite_bytes.and_then(|bytes| decode_satellite_image(bytes)) {
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
    let anchor = match chunk_anchor_world(coord, projection, scale) {
        Ok(a) => a,
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
        Transform::from_translation(anchor),
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
                anchor,
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
        let style = style_map.style_for(&track.track_node_id);
        let Some(mut geometry) = RenderedTrackGeometry::prepare(&route.points, style) else {
            continue;
        };
        let positions = &geometry.positions;

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
        let centroid = geometry.centroid;
        let rel: Vec<[f32; 3]> = positions
            .iter()
            .map(|p| [p[0] - centroid[0], p[1] - centroid[1], p[2] - centroid[2]])
            .collect();
        let ribbon = if let Some(prepared) = &geometry.zone {
            let fill = style.fill_color_u8();
            let prepared_relative = PreparedTrackZone {
                points: prepared
                    .points
                    .iter()
                    .map(|point| {
                        [
                            point[0] - centroid[0],
                            point[1] - centroid[1],
                            point[2] - centroid[2],
                        ]
                    })
                    .collect(),
                triangle_indices: prepared.triangle_indices.clone(),
            };
            track_zone_mesh_prepared(
                &prepared_relative,
                style.width.max(0.01),
                color,
                Color::srgba_u8(fill[0], fill[1], fill[2], fill[3]),
            )
        } else {
            track_mesh(&rel, style.width.max(0.01), TrackColorMode::Solid(color))
        };

        let handle = meshes.add(ribbon);
        geometry.mesh_id = Some(handle.id());
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            alpha_mode: if style.closed {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            },
            ..default()
        });

        // try_insert throughout: the bridge may despawn a line the same frame
        // (petal switch / style re-render) before these commands flush.
        let mut e = commands.entity(entity);
        e.try_insert((
            Mesh3d(handle),
            MeshMaterial3d(material),
            Transform::from_translation(Vec3::new(centroid[0], centroid[1], centroid[2])),
            geometry,
        ));
        // FR-3 visibility: apply the current visible flag at build time. A later
        // toggle isn't a cheap in-place `Visibility` flip — it persists the
        // `gis.track.visible` prop, which despawns+respawns the `GpxTrackLine`
        // via the bridge and re-runs this build, same as a color/width change.
        // Per-source `gpx_track` layer, else the category layer (§petal_binding
        // "Category layers"): the ribbon shows only if BOTH its style and that
        // layer are visible. Applied here too because a re-assignment rebuilds
        // ribbons after `sync_layer_visibility`'s change-gated pass has run.
        let layer = crate::layers::gpx_layer_for(&layer_stack, &track.track_node_id);
        let layer_visible = layer.is_none_or(|l| l.visible);
        e.try_insert(if style.visible && layer_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if !style.visible {
            e.try_insert(StyleHidden);
        }
        if let Some(layer) = layer {
            e.try_insert(LayerEntity { layer_id: layer.id });
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

        // Per-source `geojson_overlay` layer, else the category layer.
        let layer = crate::layers::geojson_layer_for(&layer_stack, &overlay.source_path);
        let layer_id = layer.map(|l| l.id);
        let initial_visibility = if layer.is_none_or(|l| l.visible) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

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
                child.insert((LayerEntity { layer_id }, initial_visibility));
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
                child.insert((LayerEntity { layer_id }, initial_visibility));
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
                child.insert((LayerEntity { layer_id }, initial_visibility));
            }
        }
    }
}

/// Footprint vertices of `proposal` whose ghost height rests on a real base
/// sample (all of them for an absolute Flatten/Level with a target).
fn proposal_grounded_vertices(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposal: &crate::terrain_proposal::TerrainProposal,
) -> usize {
    use crate::terrain_proposal::TerrainOp;
    let absolute = matches!(proposal.op, TerrainOp::Flatten | TerrainOp::Level)
        && proposal.target_height.is_some();
    if absolute {
        return proposal.footprint.len();
    }
    proposal
        .footprint
        .iter()
        .filter(|&&[x, z]| base(x, z).is_some())
        .count()
}

/// Ghost-set rebuild gate: a new assignment revision, newly grounded footprint
/// vertices (chunks arrived under an ungrounded ghost), or more budget for a
/// truncated set.
fn proposals_need_rebuild(
    last_revision: Option<u64>,
    revision: u64,
    coverage_grew: bool,
    truncated: bool,
    budget_grew: bool,
) -> bool {
    last_revision != Some(revision) || coverage_grew || (truncated && budget_grew)
}

/// Ghosts a rebuild may hold: the new-spawn allowance plus the resident ghosts
/// the rebuild despawns first, hard-capped at [`MAX_PROPOSAL_OVERLAYS`].
fn proposal_overlay_capacity(allowance: usize, resident: usize) -> usize {
    allowance
        .saturating_add(resident)
        .min(MAX_PROPOSAL_OVERLAYS)
}

/// Render non-destructive terrain proposals as a relief-following LATTICE
/// ghost (FR-1, fixes R-1's flat-lid) plus an optional cut/fill heatmap drape
/// (FR-2) over the READ-ONLY [`TerrainHeightField`] (NFR-1). Rebuilds on an
/// assignment revision change, when terrain chunks ground a proposal that had
/// nothing to show yet, when the residency budget grows under a truncated
/// set, or when [`HeatmapOverlayToggle`] flips. Tagged to the `ProposalOverlay`
/// layer; bounded by [`ProposalOverlayBudget`] (NFR-2). See `src/AGENTS.md`
/// §terrain-proposals, §delta-viz.
#[allow(clippy::too_many_arguments)]
fn render_terrain_proposals(
    active: Res<ActivePetalTerrain>,
    layer_stack: Res<LayerStack>,
    heatmap_toggle: Res<HeatmapOverlayToggle>,
    height_field: Option<Res<TerrainHeightField>>,
    budget: Res<ProposalOverlayBudget>,
    existing: Query<Entity, With<ProposalOverlayEntity>>,
    existing_heatmaps: Query<Entity, With<ProposalHeatmapEntity>>,
    mut state: ResMut<ProposalRenderState>,
    mut heatmap_usage: ResMut<HeatmapVertexUsage>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let config = active.config.as_ref().filter(|c| c.enabled);
    // READ-ONLY base sampler over the true heightfield — NEVER mutated (NFR-1).
    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));

    let capacity = proposal_overlay_capacity(budget.allowance, state.built.len());
    // Re-ground check samples only while a ghost awaits terrain and chunks moved.
    let coverage_grew = state.ungrounded
        && height_field.as_ref().is_some_and(|f| f.is_changed())
        && config.is_some_and(|c| {
            let grounded: usize = state
                .built
                .iter()
                .filter_map(|&i| c.proposals.get(i))
                .map(|p| proposal_grounded_vertices(&base, p))
                .sum();
            grounded > state.grounded_vertices
        });
    let rebuild_needed = proposals_need_rebuild(
        state.last_revision,
        active.revision,
        coverage_grew,
        state.truncated,
        capacity > state.built.len(),
    ) || heatmap_toggle.is_changed();
    if !rebuild_needed {
        return;
    }
    let state = &mut *state;
    state.last_revision = Some(active.revision);
    state.built.clear();
    state.ungrounded = false;
    state.grounded_vertices = 0;
    state.truncated = false;

    // Sole despawner of proposal ghosts + their heatmap drapes: clear the previous build.
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    for entity in existing_heatmaps.iter() {
        commands.entity(entity).despawn();
    }
    heatmap_usage.committed = 0;

    let Some(config) = config else {
        return;
    };
    if config.proposals.is_empty() {
        return;
    }

    let layer_id = find_layer(&layer_stack, |t| matches!(t, LayerType::ProposalOverlay));
    let initial_visibility = layer_initial_visibility(&layer_stack, layer_id);
    let world_scale = config.effective_world_scale();
    let mut heatmap_cap_warned = false;

    for (index, proposal) in config.proposals.iter().enumerate() {
        // T3: earthwork regions bake into the chunk meshes (`bake_earthwork_regions`)
        // — never double-visualize records that pass the complete region predicate.
        if proposal.is_earthwork_region() {
            continue;
        }
        if state.built.len() >= capacity {
            state.truncated = true;
            tracing::warn!(
                cap = capacity,
                "proposal overlays reached the residency budget; truncating"
            );
            break;
        }

        // Grounding bookkeeping happens for every claimed slot, whether or not
        // the lattice below currently has anything to show (re-ground gate).
        let grounded = proposal_grounded_vertices(&base, proposal);
        state.grounded_vertices += grounded;
        state.ungrounded |= grounded < proposal.footprint.len();
        state.built.push(index);

        let footprint = crate::sculpt::Footprint::Polygon {
            verts: proposal.footprint.clone(),
        };
        let grid_step = region_grid_step(footprint.bounds());
        let proposed = |x: f32, z: f32| {
            crate::terrain_proposal::apply_proposal_over_base(&base, proposal, &[[x, z]])
                .into_iter()
                .next()
                .flatten()
        };
        let Some(lattice) =
            DeltaLattice::build(&base, &proposed, &footprint, grid_step, world_scale)
        else {
            continue;
        };
        if lattice.ghost_positions.is_empty() {
            // Nothing groundable yet — an honest hole, never a fallback decal (R-1).
            continue;
        }

        let mut ghost_mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        ghost_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, lattice.ghost_positions.clone());
        ghost_mesh.insert_indices(Indices::U32(lattice.indices.clone()));
        let tint = fe_renderer::terrain_overlay::op_tint(proposal.op_snake());
        let ghost_material_handle =
            materials.add(fe_renderer::terrain_overlay::ghost_material(tint));
        let mut ghost_entity = commands.spawn((
            Mesh3d(meshes.add(ghost_mesh)),
            MeshMaterial3d(ghost_material_handle),
            fe_renderer::terrain_overlay::ProposalGhost,
            ProposalOverlayEntity {
                proposal_id: proposal.id.clone(),
            },
        ));
        if let Some(layer_id) = layer_id {
            ghost_entity.insert((LayerEntity { layer_id }, initial_visibility));
        }

        if heatmap_toggle.enabled && !lattice.drape_positions.is_empty() {
            let vertices = lattice.drape_positions.len();
            // Never sets `state.truncated`: that flag re-arms a rebuild on any
            // budget headroom, which would loop every frame on a vertex cap.
            if !heatmap_vertices_fit(heatmap_usage.committed, heatmap_usage.baked, vertices) {
                if !heatmap_cap_warned {
                    heatmap_cap_warned = true;
                    tracing::warn!(
                        cap = MAX_HEATMAP_VERTICES_PER_PETAL,
                        "proposal heatmap drapes reached the per-petal vertex cap; skipping drapes"
                    );
                }
                continue;
            }
            heatmap_usage.committed += vertices;
            let colors = heatmap_vertex_colors(&lattice.dh, world_scale);
            let mut drape_mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            drape_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, lattice.drape_positions);
            drape_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            drape_mesh.insert_indices(Indices::U32(lattice.indices));
            let drape_material = materials.add(fe_renderer::terrain_overlay::heatmap_material());
            let mut drape_entity = commands.spawn((
                Mesh3d(meshes.add(drape_mesh)),
                MeshMaterial3d(drape_material),
                ProposalHeatmapEntity {
                    proposal_id: proposal.id.clone(),
                },
            ));
            if let Some(layer_id) = layer_id {
                drape_entity.insert((LayerEntity { layer_id }, initial_visibility));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// T3 earthwork bake (sculpt_earthwork_regions_20260725 integration). Regions
// (proposal records carrying `material`) bake as VERTEX DELTAS into resident
// chunk meshes; the shared `TerrainHeightField` stays PRE-BAKE so it remains
// the pristine READ-ONLY base for deltas, volumes, and the Q-2 revert. See
// `src/AGENTS.md` §sculpt (Earthwork bake).
// ---------------------------------------------------------------------------

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

/// Initial `Visibility` for an overlay spawned onto `layer_id`: hidden when
/// that layer is hidden. `sync_layer_visibility` only reacts to LayerStack
/// CHANGES, so a mesh spawned under an already-hidden layer would otherwise
/// show (NFR-6; same rule as the GeoJSON overlay spawner).
fn layer_initial_visibility(layer_stack: &LayerStack, layer_id: Option<LayerId>) -> Visibility {
    match layer_id.and_then(|id| layer_stack.get_layer(id)) {
        Some(layer) if !layer.visible => Visibility::Hidden,
        _ => Visibility::Inherited,
    }
}

/// Whether a drape of `mesh_vertices` fits under [`MAX_HEATMAP_VERTICES_PER_PETAL`]
/// given this system's resident drapes plus the other heatmap system's usage.
fn heatmap_vertices_fit(resident: usize, other_system: usize, mesh_vertices: usize) -> bool {
    resident
        .saturating_add(other_system)
        .saturating_add(mesh_vertices)
        <= MAX_HEATMAP_VERTICES_PER_PETAL
}

/// Auto-scaled heatmap ramp max (OD-2): the deepest `|dh|` in the shown set,
/// rounded UP to a nice meter value (1/2/5 × 10ⁿ, Civil 3D-style bands) so the
/// ramp always covers what it draws; floored so a near-zero edit still shows
/// a usable ramp.
fn auto_ramp_max_m(max_abs_dh_m: f32) -> f32 {
    const RAMP_FLOOR_M: f32 = 0.1;
    let v = (max_abs_dh_m as f64).max(0.0);
    if v <= 0.0 {
        return RAMP_FLOOR_M;
    }
    let exponent = v.log10().floor();
    let magnitude = 10f64.powf(exponent);
    let fraction = v / magnitude;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    ((nice * magnitude) as f32).max(RAMP_FLOOR_M)
}

/// Fraction of the auto ramp max left transparent around `dh=0` (OD-1's
/// "neutral band"); scales with the ramp so it reads sensibly at any depth.
const HEATMAP_NEUTRAL_FRACTION: f32 = 0.05;

/// Per-vertex cut/fill heatmap RGBA for a lattice's `dh` (scene units), using
/// [`auto_ramp_max_m`] over THIS mesh's own set — OD-2's "shown set" is scoped
/// per overlay here, not synchronized globally across every proposal/region
/// (a deliberate, documented scope cut; see `src/AGENTS.md` §delta-viz).
/// `world_scale` converts scene units to real meters for the ramp (NFR-4).
fn heatmap_vertex_colors(dh: &[f32], world_scale: f64) -> Vec<[f32; 4]> {
    let scale = crate::scale::sanitize_world_scale(world_scale) as f32;
    let max_abs_m = dh.iter().fold(0.0f32, |m, d| m.max((d / scale).abs()));
    let ramp_max = auto_ramp_max_m(max_abs_m);
    let neutral = ramp_max * HEATMAP_NEUTRAL_FRACTION;
    dh.iter()
        .map(|d| fe_renderer::terrain_overlay::cut_fill_rgba(d / scale, ramp_max, neutral))
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

/// Smooth vertex normals for `positions` under `indices` (the `terrain_mesh`
/// accumulation, re-run after a bake so lighting follows the new relief).
fn recompute_smooth_normals(positions: &[[f32; 3]], indices: Option<&Indices>) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];
    let Some(indices) = indices else {
        return normals;
    };
    let idx: Vec<usize> = indices.iter().collect();
    for &[i0, i1, i2] in idx.as_chunks::<3>().0 {
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
    mut volume_state: ResMut<fe_renderer::terrain_overlay::EarthworkVolumeState>,
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

    let height_generation = height_field.as_ref().map(|field| field.generation());
    let height_changed = state.last_height_generation != height_generation;
    state.last_height_generation = height_generation;
    let config = active.config.as_ref().filter(|c| c.enabled);
    let regions = config.map(config_earthwork_regions).unwrap_or_default();

    // READ-ONLY pristine base sampler (the field stays pre-bake — NFR-1).
    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));
    // FR-5 one truth: bake, FR-4 heatmap (`render_baked_region_heatmaps`) and
    // the volume report all resolve this SAME composite (`prepare_composite`:
    // each Smooth mean at its region's own lattice), so they agree bit-for-bit.
    let composite_regions = crate::sculpt::prepare_composite(&regions, &base);

    if !regions.is_empty() && height_field.is_none() && state.warned_revision != Some(revision) {
        state.warned_revision = Some(revision);
        tracing::warn!(
            regions = regions.len(),
            "earthwork bake: no TerrainHeightField — regions persist but cannot bake (N-8)"
        );
    }

    let mut baked_any = false;
    for (entity, tx, mesh3d, baked, pristine) in chunks.iter() {
        if baked.is_some_and(|b| b.revision == revision && b.height_generation == height_generation)
        {
            continue; // up to date for this revision
        }
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            continue; // asset not ready — retry next frame (no marker)
        };
        let Some(raw) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
        else {
            commands.entity(entity).insert(EarthworkBaked {
                revision,
                height_generation,
            });
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
            composite_regions.iter().any(|r| {
                r.footprint
                    .bounds()
                    .is_some_and(|rb| bounds_intersect(cb, rb))
            })
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
                let proposed =
                    crate::sculpt::composite_proposed_height(&composite_regions, b, wx, wz);
                let dh = proposed - b;
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
        commands.entity(entity).insert(EarthworkBaked {
            revision,
            height_generation,
        });
    }

    if revision_changed || height_changed || baked_any {
        use fe_renderer::terrain_overlay::{
            EarthworkVolumePair, EarthworkVolumeReport, EarthworkVolumeSource,
        };
        volume_state.reports.clear();
        volume_state.source = None;
        let Some(config) = config else {
            return;
        };
        let Some(petal_id) = active.petal_id.as_ref() else {
            return;
        };
        let Ok(encoded) = serde_json::to_vec(config) else {
            return;
        };
        let world_scale = config.effective_world_scale();
        let source = EarthworkVolumeSource {
            petal_id: petal_id.clone(),
            terrain_revision: revision,
            height_generation: height_generation.unwrap_or(0),
            config_fingerprint:
                fe_renderer::terrain_overlay::EarthworkTerrainBinding::fingerprint_config(&encoded),
            raw_source_fingerprint: active.raw_source_fingerprint,
            assignment_fingerprint: active.assignment_fingerprint,
            world_scale,
        };
        volume_state.source = Some(source.clone());
        let valid_scale = fe_renderer::terrain_overlay::EarthworkTerrainBinding::valid_scale(
            config.world_scale,
            config.scale_bounds,
        ) && active.input_scale_valid
            && active.raw_source_fingerprint.is_some()
            && active.assignment_fingerprint.is_some();
        if regions.len() > 256 {
            return;
        }
        for region in &regions {
            if region.id.len() > 512
                || regions.iter().filter(|other| other.id == region.id).count() != 1
            {
                continue;
            }
            let Ok(encoded) = serde_json::to_vec(region) else {
                continue;
            };
            let grid_step = region_grid_step(region.footprint_shape().bounds());
            let cf = region.cut_fill(&base, grid_step, world_scale);
            let report = EarthworkVolumeReport {
                source: source.clone(),
                region_id: region.id.clone(),
                region_fingerprint: *blake3::hash(&encoded).as_bytes(),
                expected_cells: cf.expected_cells,
                sampled_cells: cf.sampled_cells,
                pair: (valid_scale && cf.complete()).then_some(EarthworkVolumePair {
                    cut_m3: cf.cut_m3,
                    fill_m3: cf.fill_m3,
                }),
            };
            volume_state
                .reports
                .insert(region.id.clone(), report.clone());
            reports.write(report);
        }
    }
}

/// FR-4 cut/fill heatmap for every committed earthwork region against the
/// PRISTINE (pre-bake) [`TerrainHeightField`] — R-3's fix: baking erased the
/// before/after comparison; this standalone overlay restores it without
/// un-baking anything. Shares the ONE composite surface `bake_earthwork_regions`
/// bakes with (FR-5). Rebuilds on a revision change, a heatmap-toggle flip,
/// late-arriving covering chunks, or budget growth under truncation; bounded
/// by [`ProposalOverlayBudget`] + [`MAX_HEATMAP_VERTICES_PER_PETAL`] (NFR-2).
/// Sole owner/despawner of [`BakedRegionHeatmapEntity`]. See `src/AGENTS.md`
/// §sculpt, §delta-viz.
#[allow(clippy::too_many_arguments)]
fn render_baked_region_heatmaps(
    active: Res<ActivePetalTerrain>,
    layer_stack: Res<LayerStack>,
    heatmap_toggle: Res<HeatmapOverlayToggle>,
    height_field: Option<Res<TerrainHeightField>>,
    budget: Res<ProposalOverlayBudget>,
    existing: Query<Entity, With<BakedRegionHeatmapEntity>>,
    mut state: ResMut<BakedHeatmapRenderState>,
    mut heatmap_usage: ResMut<HeatmapVertexUsage>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let config = active.config.as_ref().filter(|c| c.enabled);
    // READ-ONLY pristine base sampler — never the baked chunk mesh (R-3/NFR-1).
    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));
    let grounded_vertices = |regions: &[crate::sculpt::EarthworkRegion]| -> usize {
        regions
            .iter()
            .flat_map(|r| r.footprint.iter())
            .filter(|&&[x, z]| base(x, z).is_some())
            .count()
    };

    let capacity = proposal_overlay_capacity(budget.allowance, state.built);
    // Re-ground check runs only while a drape awaits terrain and chunks moved.
    let coverage_grew = state.ungrounded
        && height_field.as_ref().is_some_and(|f| f.is_changed())
        && config.is_some_and(|c| {
            grounded_vertices(&config_earthwork_regions(c)) > state.grounded_vertices
        });
    let rebuild_needed = proposals_need_rebuild(
        state.last_revision,
        active.revision,
        coverage_grew,
        state.truncated,
        capacity > state.built,
    ) || heatmap_toggle.is_changed();
    if !rebuild_needed {
        return;
    }
    let state = &mut *state;
    state.last_revision = Some(active.revision);
    state.built = 0;
    state.grounded_vertices = 0;
    state.ungrounded = false;
    state.truncated = false;

    // Sole despawner of this marker: clear the previous build.
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    heatmap_usage.baked = 0;

    if !heatmap_toggle.enabled {
        return;
    }
    let Some(config) = config else {
        return;
    };
    let regions = config_earthwork_regions(config);
    if regions.is_empty() {
        return;
    }

    let composite_regions = crate::sculpt::prepare_composite(&regions, &base);
    let world_scale = config.effective_world_scale();
    let layer_id = find_layer(&layer_stack, |t| matches!(t, LayerType::ProposalOverlay));
    let initial_visibility = layer_initial_visibility(&layer_stack, layer_id);
    let mut heatmap_cap_warned = false;

    for region in &regions {
        if state.built >= capacity {
            state.truncated = true;
            tracing::warn!(
                cap = capacity,
                "baked-region heatmaps reached the residency budget; truncating"
            );
            break;
        }
        let grounded = grounded_vertices(std::slice::from_ref(region));
        state.grounded_vertices += grounded;
        state.ungrounded |= grounded < region.footprint.len();

        let footprint = region.footprint_shape();
        // The region's OWN lattice — the same step its persisted volume uses.
        let grid_step = region_grid_step(footprint.bounds());
        let proposed = |x: f32, z: f32| {
            base(x, z)
                .map(|b| crate::sculpt::composite_proposed_height(&composite_regions, b, x, z))
        };
        let Some(lattice) =
            DeltaLattice::build(&base, &proposed, &footprint, grid_step, world_scale)
        else {
            continue;
        };
        if lattice.drape_positions.is_empty() {
            continue;
        }
        let vertices = lattice.drape_positions.len();
        // Never sets `state.truncated` (see `render_terrain_proposals`).
        if !heatmap_vertices_fit(heatmap_usage.baked, heatmap_usage.committed, vertices) {
            if !heatmap_cap_warned {
                heatmap_cap_warned = true;
                tracing::warn!(
                    cap = MAX_HEATMAP_VERTICES_PER_PETAL,
                    "baked-region heatmap drapes reached the per-petal vertex cap; skipping"
                );
            }
            continue;
        }
        heatmap_usage.baked += vertices;
        state.built += 1;

        let colors = heatmap_vertex_colors(&lattice.dh, world_scale);
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, lattice.drape_positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(lattice.indices));
        let material = materials.add(fe_renderer::terrain_overlay::heatmap_material());
        let mut entity = commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            BakedRegionHeatmapEntity {
                region_id: region.id.clone(),
            },
        ));
        if let Some(layer_id) = layer_id {
            entity.insert((LayerEntity { layer_id }, initial_visibility));
        }
    }
}

// ---------------------------------------------------------------------------
// FR-3 live pre-commit preview (terrain_proposal_delta_viz_20261025 S2).
// Reads the fe-renderer-seam `ProposalPreview` Resource that the armed tool
// (fe-ui, S3) inserts/removes; renders FR-1 ghost + FR-2 heatmap debounced
// (OD-3) and writes a `ProposalPreviewReport`. NEVER writes the DB or the
// height field — see `src/AGENTS.md` §delta-viz.
// ---------------------------------------------------------------------------

/// Comparable snapshot of a [`ProposalPreview`](fe_renderer::terrain_overlay::ProposalPreview)
/// draft for the debounce's change detection (OD-3).
#[derive(Debug, Clone, PartialEq)]
struct PreviewSnapshot {
    petal_id: String,
    op: String,
    footprint: Vec<[f32; 2]>,
    target_m: f32,
    delta_m: f32,
    strength: f32,
    source: fe_renderer::terrain_overlay::PreviewSource,
}

impl From<&fe_renderer::terrain_overlay::ProposalPreview> for PreviewSnapshot {
    fn from(p: &fe_renderer::terrain_overlay::ProposalPreview) -> Self {
        Self {
            petal_id: p.petal_id.clone(),
            op: p.op.clone(),
            footprint: p.footprint.clone(),
            target_m: p.target_m,
            delta_m: p.delta_m,
            strength: p.strength,
            source: p.source,
        }
    }
}

/// What [`PreviewDebounce::poll`] decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewAction {
    /// Draft cleared (or petal-scope mismatch) — despawn, nothing to build.
    Clear,
    /// (Re)build now: coarse while still changing, full once settled (OD-3).
    Build { full: bool },
}

/// Trailing-edge debounce for the FR-3 live preview (OD-3): a NEW draft value
/// rebuilds immediately at the coarse lattice (so the user always sees
/// something every tick — NFR-3); once the draft has been stable for
/// [`PREVIEW_DEBOUNCE_SECS`], one more rebuild upgrades to the full lattice.
/// Mirrors `fe_ui::settings::SettingsSaveDebounce`'s trailing-edge shape.
#[derive(Debug, Default)]
struct PreviewDebounce {
    seen: Option<PreviewSnapshot>,
    last_change_secs: f64,
    settled: bool,
}

impl PreviewDebounce {
    /// Returns the action to take now, or `None` for "already built at the
    /// current fidelity — nothing changed".
    fn poll(&mut self, current: Option<&PreviewSnapshot>, now_secs: f64) -> Option<PreviewAction> {
        match current {
            None => self.seen.take().map(|_| PreviewAction::Clear),
            Some(snap) if self.seen.as_ref() != Some(snap) => {
                self.seen = Some(snap.clone());
                self.last_change_secs = now_secs;
                self.settled = false;
                Some(PreviewAction::Build { full: false })
            }
            Some(_) => self.poll_unchanged(now_secs),
        }
    }

    /// [`poll`](Self::poll) for a frame whose draft is known unchanged — no
    /// snapshot (and no allocation) needed: only the settle timer can fire.
    fn poll_unchanged(&mut self, now_secs: f64) -> Option<PreviewAction> {
        if self.seen.is_some()
            && !self.settled
            && now_secs - self.last_change_secs >= PREVIEW_DEBOUNCE_SECS
        {
            self.settled = true;
            Some(PreviewAction::Build { full: true })
        } else {
            None
        }
    }
}

/// Build the FR-3 live-preview lattice for the armed tool's draft at a chosen
/// `resolution` (coarse while changing, full once settled — OD-3). Palette-op
/// drafts reuse `apply_proposal_over_base` (the full `TerrainOp` vocabulary,
/// matching what the committed ghost will look like after commit); shape/brush
/// drafts go through `sculpt::proposed_height` so `Smooth` gets its real
/// region-mean relax instead of the proposal-seam's identity shortcut
/// (`terrain_proposal.rs`'s `Smooth` is identity there — delta-viz tripwire).
///
/// `ProposalPreview.footprint` is WORLD units; `target_m`/`delta_m` are REAL
/// METERS for every source (fe-ui's `publish_proposal_preview` converts at its
/// emit boundary), while `TerrainProposal`/`sculpt::proposed_height` take
/// WORLD/scene units (N-1). Converted back here via `* world_scale` (the
/// inverse of fe-ui's `world_to_meters`) — the one seam-unit crossing.
fn build_preview_lattice(
    preview: &fe_renderer::terrain_overlay::ProposalPreview,
    base: &impl Fn(f32, f32) -> Option<f32>,
    resolution: f32,
    world_scale: f64,
) -> Option<DeltaLattice> {
    use fe_renderer::terrain_overlay::PreviewSource;
    let footprint = crate::sculpt::Footprint::Polygon {
        verts: preview.footprint.clone(),
    };
    let grid_step = lattice_grid_step(footprint.bounds(), resolution);
    let scale = crate::scale::sanitize_world_scale(world_scale);
    let target_world = (preview.target_m as f64 * scale) as f32;
    let delta_world = (preview.delta_m as f64 * scale) as f32;
    match preview.source {
        PreviewSource::PaletteOp | PreviewSource::SelectedProposal => {
            let op = crate::terrain_proposal::TerrainOp::from_snake(&preview.op)?;
            let draft = crate::terrain_proposal::TerrainProposal {
                id: "__preview__".into(),
                op,
                footprint: preview.footprint.clone(),
                target_height: Some(target_world),
                delta: Some(delta_world),
                material: None,
            };
            let proposed = |x: f32, z: f32| {
                crate::terrain_proposal::apply_proposal_over_base(base, &draft, &[[x, z]])
                    .into_iter()
                    .next()
                    .flatten()
            };
            DeltaLattice::build(base, &proposed, &footprint, grid_step, world_scale)
        }
        PreviewSource::SculptShape | PreviewSource::BrushStroke => {
            let op = crate::sculpt::SculptOp::from_snake(&preview.op)?;
            let mean = (op == crate::sculpt::SculptOp::Smooth)
                .then(|| crate::sculpt::region_mean_height(base, &footprint, grid_step))
                .flatten();
            let proposed = |x: f32, z: f32| {
                crate::sculpt::proposed_height(
                    op,
                    base(x, z),
                    target_world,
                    delta_world,
                    mean,
                    preview.strength,
                )
            };
            DeltaLattice::build(base, &proposed, &footprint, grid_step, world_scale)
        }
    }
}

/// Live FR-3 preview: the armed tool's draft rendered as a ghost + heatmap
/// drape, debounced (OD-3). NEVER writes the DB (fe-terrain has no such
/// channel) or the height field — this system takes it as `Option<Res<_>>`,
/// so mutation is structurally impossible (NFR-1). Never bumps
/// `ActivePetalTerrain.revision`. A `SelectedProposal` draft is readout-only:
/// its committed ghost/drape already render, so no duplicate mesh spawns.
/// Sole owner/despawner of [`ProposalPreviewEntity`].
#[allow(clippy::too_many_arguments)]
fn render_proposal_preview(
    preview: Option<Res<fe_renderer::terrain_overlay::ProposalPreview>>,
    active: Res<ActivePetalTerrain>,
    layer_stack: Res<LayerStack>,
    heatmap_toggle: Res<HeatmapOverlayToggle>,
    height_field: Option<Res<TerrainHeightField>>,
    existing: Query<Entity, With<ProposalPreviewEntity>>,
    time: Res<Time>,
    mut debounce: Local<PreviewDebounce>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut reports: MessageWriter<fe_renderer::terrain_overlay::ProposalPreviewReport>,
) {
    // Petal-scope gate: a draft for a different/absent petal is treated as cleared.
    let scoped = preview
        .as_deref()
        .filter(|p| active.petal_id.as_deref() == Some(p.petal_id.as_str()));
    let now = time.elapsed_secs_f64();

    // NFR-3: snapshot (clones) only when the draft or its petal scope may have
    // moved; an idle frame just ticks the settle timer, allocation-free.
    let draft_moved = preview.as_ref().is_some_and(|p| p.is_changed())
        || active.is_changed()
        || scoped.is_some() != debounce.seen.is_some();
    let mut action = if draft_moved {
        let snapshot = scoped.map(PreviewSnapshot::from);
        debounce.poll(snapshot.as_ref(), now)
    } else {
        debounce.poll_unchanged(now)
    };
    if action.is_none() && heatmap_toggle.is_changed() && debounce.seen.is_some() {
        // The drape sub-toggle flipped under a live preview: rebuild as-is.
        action = Some(PreviewAction::Build {
            full: debounce.settled,
        });
    }
    let Some(action) = action else {
        return;
    };

    // Sole despawner of this marker: clear the previous build (and every Clear).
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }

    let PreviewAction::Build { full } = action else {
        return; // Clear: despawned above, nothing to (re)build.
    };
    let Some(preview) = scoped else {
        return;
    };

    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));
    let world_scale = active
        .config
        .as_ref()
        .map(|c| c.effective_world_scale())
        .unwrap_or(1.0);
    let resolution = if full {
        EARTHWORK_LATTICE
    } else {
        PREVIEW_LATTICE_COARSE
    };
    let Some(lattice) = build_preview_lattice(preview, &base, resolution, world_scale) else {
        return;
    };

    reports.write(fe_renderer::terrain_overlay::ProposalPreviewReport {
        petal_id: preview.petal_id.clone(),
        cut_m3: lattice.cut_fill.cut_m3,
        fill_m3: lattice.cut_fill.fill_m3,
        net_m3: lattice.cut_fill.net_m3(),
        scaled: lattice.cut_fill.scaled,
    });

    if preview.source == fe_renderer::terrain_overlay::PreviewSource::SelectedProposal {
        return; // readout-only: the committed overlay already shows this delta
    }
    if lattice.ghost_positions.is_empty() {
        return; // Nothing groundable yet — an honest hole, same as the committed ghost.
    }

    let layer_id = find_layer(&layer_stack, |t| matches!(t, LayerType::ProposalOverlay));
    let initial_visibility = layer_initial_visibility(&layer_stack, layer_id);

    let mut ghost_mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    ghost_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, lattice.ghost_positions);
    ghost_mesh.insert_indices(Indices::U32(lattice.indices.clone()));
    let tint = fe_renderer::terrain_overlay::op_tint(&preview.op);
    let ghost_material_handle = materials.add(fe_renderer::terrain_overlay::ghost_material(tint));
    let mut ghost_entity = commands.spawn((
        Mesh3d(meshes.add(ghost_mesh)),
        MeshMaterial3d(ghost_material_handle),
        fe_renderer::terrain_overlay::ProposalGhost,
        ProposalPreviewEntity,
    ));
    if let Some(layer_id) = layer_id {
        ghost_entity.insert((LayerEntity { layer_id }, initial_visibility));
    }

    if heatmap_toggle.enabled && !lattice.drape_positions.is_empty() {
        let colors = heatmap_vertex_colors(&lattice.dh, world_scale);
        let mut drape_mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        drape_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, lattice.drape_positions);
        drape_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        drape_mesh.insert_indices(Indices::U32(lattice.indices));
        let drape_material = materials.add(fe_renderer::terrain_overlay::heatmap_material());
        let mut drape_entity = commands.spawn((
            Mesh3d(meshes.add(drape_mesh)),
            MeshMaterial3d(drape_material),
            ProposalPreviewEntity,
        ));
        if let Some(layer_id) = layer_id {
            drape_entity.insert((LayerEntity { layer_id }, initial_visibility));
        }
    }
}

/// A layer-bound entity's visibility, optional material, and style-hidden flag.
type LayerBoundEntity = (
    &'static LayerEntity,
    &'static mut Visibility,
    Option<&'static MeshMaterial3d<StandardMaterial>>,
    Has<StyleHidden>,
);

/// Synchronize [`LayerStack`] visibility/opacity to layer-bound entities (on change only).
fn sync_layer_visibility(
    layer_stack: Res<LayerStack>,
    mut query: Query<LayerBoundEntity>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !layer_stack.is_changed() {
        return;
    }

    for (layer_entity, mut visibility, material_handle, style_hidden) in query.iter_mut() {
        let Some(layer) = layer_stack.get_layer(layer_entity.layer_id) else {
            continue;
        };

        *visibility = if layer.visible && !style_hidden {
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

// ---------------------------------------------------------------------------
// P2 selected-volume report (terrain_proposal_delta_viz_20261025 S5). Answers
// the fe-renderer-seam `VolumeReportRequest` with a true `VolumeReportResult`
// computed by `volume_report::selection_volume_report` — the SAME composite
// truth the bake/preview use (FR-5 extended to the report). See
// `src/AGENTS.md` §volume-report.
// ---------------------------------------------------------------------------

/// Latest scoped selection and the last served terrain revision.
#[derive(Default)]
struct RetainedVolumeRequest {
    request: Option<fe_renderer::terrain_overlay::VolumeReportRequest>,
    revision: Option<u64>,
}

/// Refresh the current report on request, assignment or resident-height changes.
fn answer_volume_report_requests(
    mut requests: MessageReader<fe_renderer::terrain_overlay::VolumeReportRequest>,
    active: Res<ActivePetalTerrain>,
    height_field: Option<Res<TerrainHeightField>>,
    scene: Option<Res<fe_renderer::scene_analysis::SceneAnalysisSnapshot>>,
    mut results: MessageWriter<fe_renderer::terrain_overlay::VolumeReportResult>,
    mut retained: Local<RetainedVolumeRequest>,
) {
    let mut dirty = retained.revision != Some(active.revision)
        || scene.as_ref().is_some_and(|snapshot| snapshot.is_changed())
        || height_field
            .as_ref()
            .is_some_and(|field| field.is_changed());
    if retained
        .request
        .as_ref()
        .is_some_and(|request| active.petal_id.as_deref() != Some(request.petal_id.as_str()))
        || !active.config.as_ref().is_some_and(|config| config.enabled)
    {
        retained.request = None;
    }
    retained.revision = Some(active.revision);
    for request in requests.read() {
        let unavailable = if active.petal_id.as_deref() != Some(request.petal_id.as_str()) {
            Some(fe_renderer::terrain_overlay::VolumeUnavailable::WrongPetal)
        } else if !active.config.as_ref().is_some_and(|config| config.enabled) {
            Some(fe_renderer::terrain_overlay::VolumeUnavailable::NoMap)
        } else {
            None
        };
        match unavailable {
            // A request is always answered: unavailable is a result, not silence.
            Some(reason) => {
                results.write(unavailable_volume_result(request, reason));
            }
            None => {
                retained.request = Some(request.clone());
                dirty = true;
            }
        }
    }
    if !dirty {
        return;
    }
    let Some(request) = retained.request.as_ref() else {
        return;
    };
    let Some(config) = active.config.as_ref().filter(|config| config.enabled) else {
        return;
    };

    let selection = crate::sculpt::Footprint::Polygon {
        verts: request.footprint.clone(),
    };
    let step = region_grid_step(selection.bounds());
    let world_scale = config.effective_world_scale();
    let base = |x: f32, z: f32| height_field.as_ref().and_then(|f| f.height_at(x, z));

    let proposals: Vec<crate::terrain_proposal::TerrainProposal> = config
        .proposals
        .iter()
        .filter(|p| !p.is_earthwork_region())
        .cloned()
        .collect();
    let regions = config_earthwork_regions(config);

    let report = crate::volume_report::selection_volume_report(
        &base,
        &proposals,
        &regions,
        &selection,
        step,
        world_scale,
    );

    // Match scene geometry to this selected combined terrain surface.
    let mut object_impacts = fe_renderer::scene_analysis::ObjectImpactReport {
        scene_revision: scene.as_ref().map_or(0, |scene| scene.revision),
        ..Default::default()
    };
    if let Some(scene) = scene
        .as_ref()
        .filter(|scene| scene.petal_id.as_deref() == Some(request.petal_id.as_str()))
    {
        object_impacts.scene_revision = scene.revision;
        object_impacts.omitted_objects = scene.omitted_objects
            + scene
                .objects
                .len()
                .saturating_sub(fe_renderer::scene_analysis::SCENE_OBJECT_LIMIT);
        object_impacts.excluded_non_geometric = scene.excluded_non_geometric;
        object_impacts.available =
            report.expected_samples > 0 && report.sampled_pairs == report.expected_samples;
        if object_impacts.available {
            let objects: Vec<_> = scene
                .objects
                .iter()
                .take(fe_renderer::scene_analysis::SCENE_OBJECT_LIMIT)
                .collect();
            let boxes: Vec<_> = objects.iter().filter_map(|object| object.bounds).collect();
            let overlaps = crate::volume_report::changed_space_overlaps(
                &base,
                &proposals,
                &regions,
                &selection,
                step,
                world_scale,
                &boxes,
            );
            let mut overlaps = overlaps.into_iter();
            for object in objects {
                let overlap = if object.bounds.is_some() {
                    overlaps.next().flatten()
                } else {
                    None
                };
                if overlap.is_some() {
                    object_impacts.checked_objects += 1;
                }
                if overlap.is_none_or(|volume| volume > 0.0) {
                    object_impacts
                        .objects
                        .push(fe_renderer::scene_analysis::ObjectImpact {
                            object: object.clone(),
                            overlap_m3: overlap,
                        });
                }
            }
        }
    }
    let items = report
        .items
        .into_iter()
        .map(|item| fe_renderer::terrain_overlay::VolumeItem {
            id: item.id,
            kind: match item.kind {
                crate::volume_report::VolumeItemKind::Proposal => item.op_snake.to_string(),
                crate::volume_report::VolumeItemKind::Region => "earthwork_region".to_string(),
            },
            operation: item.op_snake.to_string(),
            material: item.material,
            cut_m3: item.cut_m3,
            fill_m3: item.fill_m3,
            overlap_area_m2: item.overlap_area_m2,
        })
        .collect();

    results.write(fe_renderer::terrain_overlay::VolumeReportResult {
        correlation_id: request.correlation_id.clone(),
        petal_id: request.petal_id.clone(),
        expected_samples: report.expected_samples,
        sampled_pairs: report.sampled_pairs,
        area_m2: report.area_m2,
        cut_m3: report.cut_m3,
        fill_m3: report.fill_m3,
        net_m3: report.net_m3,
        step_m: report.step_m as f32,
        items,
        has_overlap: report.overlap,
        scaled: report.scaled,
        object_impacts,
        unavailable: None,
    });
}

/// Zero-figure reply telling the panel why a request cannot be measured.
fn unavailable_volume_result(
    request: &fe_renderer::terrain_overlay::VolumeReportRequest,
    reason: fe_renderer::terrain_overlay::VolumeUnavailable,
) -> fe_renderer::terrain_overlay::VolumeReportResult {
    fe_renderer::terrain_overlay::VolumeReportResult {
        correlation_id: request.correlation_id.clone(),
        petal_id: request.petal_id.clone(),
        expected_samples: 0,
        sampled_pairs: 0,
        area_m2: 0.0,
        cut_m3: 0.0,
        fill_m3: 0.0,
        net_m3: 0.0,
        step_m: 0.0,
        items: Vec::new(),
        has_overlap: false,
        scaled: false,
        object_impacts: Default::default(),
        unavailable: Some(reason),
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
    fn auto_ramp_max_rounds_up_to_a_nice_value_and_floors_near_zero() {
        // Table-driven: every case's ramp must be >= the input depth (never
        // clips what it draws) and snap to a 1/2/5 x 10^n value.
        let cases: &[(f32, f32)] = &[
            (0.0, 0.1),
            (0.05, 0.1),
            (0.9, 1.0),
            (1.4, 2.0),
            (3.0, 5.0),
            (7.0, 10.0),
            (12.0, 20.0),
        ];
        for &(input, expected) in cases {
            let got = auto_ramp_max_m(input);
            assert!(
                (got - expected).abs() < 1e-4,
                "auto_ramp_max_m({input}) = {got}, expected {expected}"
            );
            assert!(
                got >= input,
                "ramp must never clip its own input: {got} < {input}"
            );
        }
    }

    #[test]
    fn heatmap_vertex_colors_sign_matches_cut_and_fill() {
        // dh in SCENE units at world_scale 2 -> dh_m = dh/2. Mutation check: a
        // flipped sign comparison here would misclassify fill as cut.
        let dh = [-4.0, 0.0, 4.0];
        let colors = heatmap_vertex_colors(&dh, 2.0);
        assert_eq!(colors.len(), 3);
        let [r_cut, _, b_cut, a_cut] = colors[0];
        assert!(
            a_cut > 0.0 && r_cut > b_cut,
            "cut (dh<0) must read red-dominant"
        );
        let [_, _, _, a_zero] = colors[1];
        assert_eq!(a_zero, 0.0, "dh=0 is inside the neutral band");
        let [r_fill, _, b_fill, a_fill] = colors[2];
        assert!(
            a_fill > 0.0 && b_fill > r_fill,
            "fill (dh>0) must read blue-dominant"
        );
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

    // --- proposal ghost rebuild gate + residency budget (TERRAIN-REGROUND/RESIDENCY) ---

    use crate::terrain_proposal::{TerrainOp, TerrainProposal};
    use fe_renderer::terrain_height::HeightTile;

    fn square_proposal(id: &str, op: TerrainOp, origin: f32) -> TerrainProposal {
        TerrainProposal {
            id: id.into(),
            op,
            footprint: vec![
                [origin, origin],
                [origin + 10.0, origin],
                [origin + 10.0, origin + 10.0],
                [origin, origin + 10.0],
            ],
            target_height: None,
            delta: Some(2.0),
            material: None,
        }
    }

    /// Flat 2x2 height tile at `anchor` (ground height = `anchor.y`).
    fn flat_tile(anchor: Vec3, tile_size: f32) -> HeightTile {
        HeightTile {
            anchor,
            tile_size,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        }
    }

    /// Headless app running only `render_terrain_proposals` over `proposals`.
    fn proposal_app(proposals: Vec<TerrainProposal>, allowance: usize) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_resource::<ProposalRenderState>()
            .init_resource::<HeatmapOverlayToggle>()
            .init_resource::<HeatmapVertexUsage>()
            .insert_resource(ProposalOverlayBudget { allowance })
            .insert_resource(LayerStack::new())
            .insert_resource(TerrainHeightField::default())
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: None,
                assignment_fingerprint: None,
                input_scale_valid: false,
                petal_id: Some("petal".into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    proposals,
                    ..TerrainConfig::default()
                }),
                revision: 1,
            })
            .add_systems(Update, render_terrain_proposals);
        app
    }

    /// Like `proposal_app` but with one covering height tile already resident,
    /// so lattice-based ghosts/heatmaps have something to render from frame
    /// one (the re-ground SEQUENCING itself is `relative_ghost_regrounds_once_
    /// covering_chunks_arrive`'s job; most other tests just need grounding).
    fn proposal_app_grounded(
        proposals: Vec<TerrainProposal>,
        allowance: usize,
        tile: HeightTile,
    ) -> App {
        let mut app = proposal_app(proposals, allowance);
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), tile);
        app
    }

    /// Resident ghosts as `(entity, proposal_id, vertex Ys)`, sorted by id.
    fn ghosts(app: &mut App) -> Vec<(Entity, String, Vec<f32>)> {
        let world = app.world_mut();
        let mut query = world.query::<(Entity, &Mesh3d, &ProposalOverlayEntity)>();
        let rows: Vec<(Entity, Handle<Mesh>, String)> = query
            .iter(world)
            .map(|(e, m, p)| (e, m.0.clone(), p.proposal_id.clone()))
            .collect();
        let meshes = world.resource::<Assets<Mesh>>();
        let mut out: Vec<_> = rows
            .into_iter()
            .map(|(e, handle, id)| {
                let ys = meshes
                    .get(&handle)
                    .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
                    .and_then(|a| a.as_float3())
                    .map(|p| p.iter().map(|v| v[1]).collect())
                    .unwrap_or_default();
                (e, id, ys)
            })
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    /// Full XYZ ghost positions for one proposal (sorted for determinism).
    fn ghost_positions(app: &mut App, proposal_id: &str) -> Vec<[f32; 3]> {
        let world = app.world_mut();
        let mut query = world.query::<(&Mesh3d, &ProposalOverlayEntity)>();
        let handle = query
            .iter(world)
            .find(|(_, p)| p.proposal_id == proposal_id)
            .map(|(m, _)| m.0.clone());
        let Some(handle) = handle else {
            return Vec::new();
        };
        let meshes = world.resource::<Assets<Mesh>>();
        let mut positions: Vec<[f32; 3]> = meshes
            .get(&handle)
            .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
            .and_then(|a| a.as_float3())
            .map(|p| p.to_vec())
            .unwrap_or_default();
        positions.sort_by(|a, b| {
            a[0].partial_cmp(&b[0])
                .unwrap()
                .then(a[2].partial_cmp(&b[2]).unwrap())
        });
        positions
    }

    /// Resident committed-proposal heatmap drape entities.
    fn heatmaps(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        let mut query = world.query_filtered::<Entity, With<ProposalHeatmapEntity>>();
        query.iter(world).collect()
    }

    #[test]
    fn committed_raise_renders_lattice_ghost_not_flat_lid() {
        // Sloped base (NOT flat): a flat-lid ghost (R-1) would have exactly 4
        // perimeter vertices and slice through this relief. The lattice ghost
        // must carry interior samples that each sit at base(x,z)+2.
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 100.0,
            grid: vec![0.0, 20.0, 0.0, 20.0], // east edge 20 higher than west
            width: 2,
            height: 2,
        };
        let mut app =
            proposal_app_grounded(vec![square_proposal("p", TerrainOp::Raise, 0.0)], 64, tile);
        app.update();

        let positions = ghost_positions(&mut app, "p");
        assert!(
            positions.len() > 4,
            "expected an interior lattice beyond the 4 footprint corners, got {}",
            positions.len()
        );
        for &[x, y, z] in &positions {
            let expected = x * 0.2 + 2.0; // slope 20/100 * x, raised by 2
            assert!(
                (y - expected).abs() < 0.05,
                "ghost at ({x},{z}) y={y}, expected ~{expected} (relief-following, not a flat lid)"
            );
        }
        {
            use crate::iot::animation::{TimestampedRoutePoint, TrackRoute};
            app.init_resource::<TrackRouteMap>()
                .init_resource::<TrackStyleMap>()
                .add_systems(Update, render_gpx_tracks);
            let points = vec![[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [4.0, 0.0, 6.0]]
                .into_iter()
                .map(|position| TimestampedRoutePoint {
                    position,
                    ..Default::default()
                })
                .collect();
            app.world_mut()
                .resource_mut::<TrackRouteMap>()
                .routes
                .insert(
                    "footprint".into(),
                    TrackRoute {
                        points,
                        total_duration_secs: 0.0,
                    },
                );
            app.world_mut()
                .resource_mut::<TrackStyleMap>()
                .styles
                .insert(
                    "footprint".into(),
                    crate::iot::TrackStyle {
                        closed: true,
                        width: 1.0,
                        ..Default::default()
                    },
                );
            let line = app
                .world_mut()
                .spawn(GpxTrackLine {
                    track_node_id: "footprint".into(),
                })
                .id();
            app.update();
            let mesh = app.world().get::<Mesh3d>(line).unwrap().clone();
            let geometry = app.world().get::<RenderedTrackGeometry>(line).unwrap();
            assert!(geometry.matches_mesh(&mesh));
            assert!(!geometry.zone().unwrap().triangle_indices.is_empty());
            assert_eq!(geometry.source_points()[0].position, [0.0; 3]);
            app.world_mut()
                .resource_mut::<TrackRouteMap>()
                .routes
                .get_mut("footprint")
                .unwrap()
                .points[0]
                .position[0] = 99.0;
            app.world_mut()
                .resource_mut::<TrackStyleMap>()
                .styles
                .get_mut("footprint")
                .unwrap()
                .width = 5.0;
            app.update();
            let geometry = app.world().get::<RenderedTrackGeometry>(line).unwrap();
            assert_eq!(
                geometry.source_points()[0].position,
                [0.0; 3],
                "a later live-map update cannot relabel an existing mesh's source"
            );
            assert_eq!(geometry.style().width, 1.0);
            assert!(geometry.matches_mesh(app.world().get::<Mesh3d>(line).unwrap()));
        }
    }

    #[test]
    fn relative_ghost_regrounds_once_covering_chunks_arrive() {
        let mut app = proposal_app(vec![square_proposal("p", TerrainOp::Raise, 10.0)], 64);
        app.update();
        assert!(
            ghosts(&mut app).is_empty(),
            "no terrain yet -> the lattice has nothing honest to show (R-1: no fallback decal)"
        );

        // A chunk that misses the footprint changes the field but grounds nothing.
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 9, 9), flat_tile(Vec3::new(1000.0, 5.0, 1000.0), 100.0));
        app.update();
        assert!(ghosts(&mut app).is_empty(), "no rebuild without coverage");

        // The covering chunk arrives -> the ghost appears at base + delta.
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::new(0.0, 5.0, 0.0), 100.0));
        app.update();
        let grounded = ghosts(&mut app);
        assert_eq!(grounded.len(), 1);
        assert!(
            grounded[0].2.iter().all(|y| (*y - 7.0).abs() < 1e-5),
            "ghost sits at base 5 + delta 2: {:?}",
            grounded[0].2
        );

        // Steady state: idle frames and further chunk churn keep the same entity.
        let stable_id = grounded[0].0;
        app.update();
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 1, 1), flat_tile(Vec3::new(-500.0, 1.0, -500.0), 100.0));
        app.update();
        assert_eq!(
            ghosts(&mut app)[0].0,
            stable_id,
            "grounded ghost never churns"
        );
    }

    #[test]
    fn proposal_ghosts_stop_at_the_residency_budget_and_grow_with_it() {
        let proposals: Vec<_> = (0..5)
            .map(|i| square_proposal(&format!("p{i}"), TerrainOp::Raise, i as f32 * 20.0))
            .collect();
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 300.0, // covers every square AND the p5 edit below
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let mut app = proposal_app_grounded(proposals, 2, tile);
        app.update();
        let first = ghosts(&mut app);
        assert_eq!(first.len(), 2, "allowance 2 -> exactly 2 ghosts");

        // Budget exhausted: no growth and no churn.
        app.world_mut()
            .resource_mut::<ProposalOverlayBudget>()
            .allowance = 0;
        app.update();
        let idle: Vec<Entity> = ghosts(&mut app).iter().map(|g| g.0).collect();
        assert_eq!(idle, first.iter().map(|g| g.0).collect::<Vec<_>>());

        // Headroom appears -> the truncated set rebuilds to the full 5.
        app.world_mut()
            .resource_mut::<ProposalOverlayBudget>()
            .allowance = 10;
        app.update();
        assert_eq!(ghosts(&mut app).len(), 5);

        // At the ceiling, a proposal edit may re-spend the resident ghosts'
        // slots but never spawn past them.
        app.world_mut()
            .resource_mut::<ProposalOverlayBudget>()
            .allowance = 0;
        {
            let mut active = app.world_mut().resource_mut::<ActivePetalTerrain>();
            let config = active.config.as_mut().unwrap();
            config
                .proposals
                .push(square_proposal("p5", TerrainOp::Raise, 200.0));
            active.revision += 1;
        }
        app.update();
        assert_eq!(ghosts(&mut app).len(), 5, "edit at the ceiling holds at 5");
    }

    #[test]
    fn heatmap_build_stops_at_residency_budget() {
        // Extends `proposal_ghosts_stop_at_the_residency_budget_and_grow_with_it`:
        // each resident ghost also gets its heatmap drape, and both scale
        // together under the SAME per-proposal budget (not a separate pool).
        let proposals: Vec<_> = (0..5)
            .map(|i| square_proposal(&format!("p{i}"), TerrainOp::Raise, i as f32 * 20.0))
            .collect();
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 300.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let mut app = proposal_app_grounded(proposals, 2, tile);
        app.update();
        assert_eq!(ghosts(&mut app).len(), 2);
        assert_eq!(
            heatmaps(&mut app).len(),
            2,
            "each resident ghost also gets a heatmap drape"
        );

        app.world_mut()
            .resource_mut::<ProposalOverlayBudget>()
            .allowance = 10;
        app.update();
        assert_eq!(ghosts(&mut app).len(), 5);
        assert_eq!(
            heatmaps(&mut app).len(),
            5,
            "heatmap drapes grow in lockstep with ghosts"
        );
    }

    #[test]
    fn heatmap_toggle_off_hides_drapes_without_touching_ghosts() {
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 100.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let mut app =
            proposal_app_grounded(vec![square_proposal("p", TerrainOp::Raise, 0.0)], 64, tile);
        app.update();
        assert_eq!(ghosts(&mut app).len(), 1);
        assert_eq!(heatmaps(&mut app).len(), 1);

        app.world_mut()
            .resource_mut::<HeatmapOverlayToggle>()
            .enabled = false;
        app.update();
        assert_eq!(ghosts(&mut app).len(), 1, "ghost stays visible");
        assert!(heatmaps(&mut app).is_empty(), "heatmap drape is gone");
    }

    #[test]
    fn grounded_vertices_count_base_samples_except_absolute_level() {
        let half = |x: f32, _z: f32| (x < 15.0).then_some(1.0);
        let none = |_x: f32, _z: f32| None;
        let raise = square_proposal("r", TerrainOp::Raise, 10.0);
        assert_eq!(proposal_grounded_vertices(&half, &raise), 2);
        assert_eq!(proposal_grounded_vertices(&none, &raise), 0);
        let mut flatten = square_proposal("f", TerrainOp::Flatten, 10.0);
        assert_eq!(proposal_grounded_vertices(&none, &flatten), 0, "no target");
        flatten.target_height = Some(3.0);
        assert_eq!(proposal_grounded_vertices(&none, &flatten), 4, "absolute");
        let mut pad = square_proposal("p", TerrainOp::Pad, 10.0);
        pad.target_height = Some(3.0);
        assert_eq!(
            proposal_grounded_vertices(&none, &pad),
            0,
            "pad = max(base, target) still depends on base"
        );
    }

    #[test]
    fn rebuild_gate_and_capacity_table() {
        // (last_revision, revision, coverage_grew, truncated, budget_grew, rebuild)
        let cases = [
            (None, 1, false, false, false, true),
            (Some(1), 2, false, false, false, true),
            (Some(1), 1, false, false, false, false),
            (Some(1), 1, true, false, false, true),
            (Some(1), 1, false, true, false, false),
            (Some(1), 1, false, false, true, false),
            (Some(1), 1, false, true, true, true),
        ];
        for (last, rev, cov, trunc, grew, want) in cases {
            assert_eq!(
                proposals_need_rebuild(last, rev, cov, trunc, grew),
                want,
                "{last:?} {rev} {cov} {trunc} {grew}"
            );
        }
        assert_eq!(proposal_overlay_capacity(2, 3), 5);
        assert_eq!(proposal_overlay_capacity(0, 0), 0);
        assert_eq!(
            proposal_overlay_capacity(usize::MAX, 7),
            MAX_PROPOSAL_OVERLAYS
        );
    }

    // --- layer visibility over the delta-viz overlays ---

    #[test]
    fn overlays_hidden_when_proposal_layer_hidden() {
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 100.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let mut app =
            proposal_app_grounded(vec![square_proposal("p", TerrainOp::Raise, 0.0)], 64, tile);
        app.add_systems(Update, sync_layer_visibility);

        let layer_id = app
            .world_mut()
            .resource_mut::<LayerStack>()
            .add_layer(crate::layers::MapLayer::new(LayerType::ProposalOverlay, 0));
        app.update();

        /// Visibility of every committed ghost + heatmap drape.
        fn overlay_visibilities(app: &mut App) -> Vec<Visibility> {
            let world = app.world_mut();
            let mut q = world.query_filtered::<&Visibility, Or<(
                With<ProposalOverlayEntity>,
                With<ProposalHeatmapEntity>,
            )>>();
            q.iter(world).copied().collect()
        }

        let before = overlay_visibilities(&mut app);
        assert_eq!(
            before.len(),
            2,
            "ghost + heatmap drape spawned, both layer-bound"
        );
        assert!(
            before.iter().all(|v| *v != Visibility::Hidden),
            "visible by default (OD-8)"
        );

        app.world_mut()
            .resource_mut::<LayerStack>()
            .get_layer_mut(layer_id)
            .unwrap()
            .visible = false;
        app.update();
        assert!(
            overlay_visibilities(&mut app)
                .iter()
                .all(|v| *v == Visibility::Hidden),
            "hiding the ProposalOverlay layer hides every overlay entity"
        );

        // A rebuild WHILE the layer stays hidden (LayerStack untouched, so
        // `sync_layer_visibility` does not run) must spawn hidden, not pop back.
        app.world_mut()
            .resource_mut::<ActivePetalTerrain>()
            .revision += 1;
        app.update();
        let rebuilt = overlay_visibilities(&mut app);
        assert_eq!(rebuilt.len(), 2, "rebuild respawned ghost + drape");
        assert!(
            rebuilt.iter().all(|v| *v == Visibility::Hidden),
            "overlays spawned under a hidden layer must start hidden: {rebuilt:?}"
        );
    }

    // --- FR-4 baked-region heatmap (BAKED-REGION-HEATMAP) ---

    fn region_proposal(id: &str, op: TerrainOp, origin: f32, target: f32) -> TerrainProposal {
        TerrainProposal {
            id: id.into(),
            op,
            footprint: vec![
                [origin, origin],
                [origin + 10.0, origin],
                [origin + 10.0, origin + 10.0],
                [origin, origin + 10.0],
            ],
            target_height: Some(target),
            delta: None,
            material: Some("earth".into()),
        }
    }

    /// Headless app running only `render_baked_region_heatmaps`.
    fn baked_heatmap_app(proposals: Vec<TerrainProposal>, tile: HeightTile) -> App {
        let mut app = baked_heatmap_app_without_terrain(proposals, MAX_PROPOSAL_OVERLAYS);
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), tile);
        app
    }

    /// `baked_heatmap_app` with an empty height field and a chosen allowance.
    fn baked_heatmap_app_without_terrain(proposals: Vec<TerrainProposal>, allowance: usize) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_resource::<BakedHeatmapRenderState>()
            .init_resource::<HeatmapOverlayToggle>()
            .init_resource::<HeatmapVertexUsage>()
            .insert_resource(ProposalOverlayBudget { allowance })
            .insert_resource(LayerStack::new())
            .insert_resource(TerrainHeightField::default())
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: None,
                assignment_fingerprint: None,
                input_scale_valid: false,
                petal_id: Some("petal".into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    proposals,
                    ..TerrainConfig::default()
                }),
                revision: 1,
            })
            .add_systems(Update, render_baked_region_heatmaps);
        app
    }

    #[test]
    fn baked_heatmap_appears_once_covering_terrain_arrives() {
        // Startup order: the petal's config (revision) lands BEFORE its chunks.
        // The FR-4 drape must still appear when the covering tile arrives —
        // a revision-only gate left the baked region heatmap-less forever.
        let region = region_proposal("r1", TerrainOp::Level, 0.0, 5.0);
        let mut app = baked_heatmap_app_without_terrain(vec![region], MAX_PROPOSAL_OVERLAYS);
        app.update();
        assert!(
            baked_heatmap_entities(&mut app).is_empty(),
            "no terrain yet"
        );

        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::ZERO, 100.0));
        app.update();
        let built = baked_heatmap_entities(&mut app);
        assert_eq!(built.len(), 1, "covering chunk arrived -> drape built");

        // Steady state: further chunk churn does not rebuild a grounded drape.
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 5, 5), flat_tile(Vec3::new(500.0, 0.0, 500.0), 100.0));
        app.update();
        assert_eq!(
            baked_heatmap_entities(&mut app),
            built,
            "grounded drape never churns"
        );
    }

    #[test]
    fn baked_heatmaps_stop_at_residency_budget() {
        // NFR-2: baked-region drapes are meshes too — they honor the SAME
        // `ProposalOverlayBudget` as the committed ghosts (no bypass).
        let regions: Vec<_> = (0..5)
            .map(|i| region_proposal(&format!("r{i}"), TerrainOp::Level, i as f32 * 20.0, 5.0))
            .collect();
        let mut app = baked_heatmap_app_without_terrain(regions, 2);
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::ZERO, 300.0));
        app.update();
        assert_eq!(
            baked_heatmap_entities(&mut app).len(),
            2,
            "allowance 2 -> 2 drapes"
        );
        let usage = *app.world().resource::<HeatmapVertexUsage>();
        assert!(
            usage.baked > 0 && usage.committed == 0,
            "baked usage tracked: {usage:?}"
        );
    }

    fn baked_heatmap_entities(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<BakedRegionHeatmapEntity>>();
        q.iter(world).collect()
    }

    #[test]
    fn baked_region_shows_heatmap_against_pristine_field() {
        // Flat pristine base at 0; a Level region to target 5 is pure fill —
        // every drape vertex must read fill-dominant (blue > red) and opaque
        // (outside the neutral band), against the PRISTINE field (R-3's fix:
        // baking must not erase this before/after comparison).
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 100.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let region = region_proposal("r1", TerrainOp::Level, 0.0, 5.0);
        let mut app = baked_heatmap_app(vec![region], tile);
        app.update();

        let entities = baked_heatmap_entities(&mut app);
        assert_eq!(entities.len(), 1, "one heatmap drape per committed region");

        let world = app.world_mut();
        let handle = world.get::<Mesh3d>(entities[0]).unwrap().0.clone();
        let meshes = world.resource::<Assets<Mesh>>();
        let mesh = meshes.get(&handle).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("heatmap drape must carry vertex colors");
        };
        assert!(!colors.is_empty());
        for &[r, _g, b, a] in colors {
            assert!(
                a > 0.0,
                "a pure fill region must be fully outside the neutral band"
            );
            assert!(b > r, "fill (dh>0) must read blue-dominant: r={r} b={b}");
        }

        // Toggling the heatmap sub-toggle off removes the drape without un-baking.
        app.world_mut()
            .resource_mut::<HeatmapOverlayToggle>()
            .enabled = false;
        app.update();
        assert!(baked_heatmap_entities(&mut app).is_empty());
    }

    // --- FR-3 live preview (PREVIEW-DEBOUNCE/PREVIEW-NFR1) ---

    use bevy::time::TimeUpdateStrategy;
    use fe_renderer::terrain_overlay::{PreviewSource, ProposalPreview, ProposalPreviewReport};
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct CapturedPreviewReports(Vec<ProposalPreviewReport>);

    fn capture_preview_reports(
        mut reader: MessageReader<ProposalPreviewReport>,
        mut sink: ResMut<CapturedPreviewReports>,
    ) {
        for msg in reader.read() {
            sink.0.push(msg.clone());
        }
    }

    fn preview_draft(footprint: Vec<[f32; 2]>, delta_m: f32) -> ProposalPreview {
        ProposalPreview {
            petal_id: "petal".into(),
            op: "raise".into(),
            footprint,
            target_m: 0.0,
            delta_m,
            strength: 1.0,
            source: PreviewSource::PaletteOp,
        }
    }

    /// Headless app running `render_proposal_preview`, with a deterministic
    /// per-`update()` time tick (via `TimeUpdateStrategy::ManualDuration`) so
    /// the OD-3 debounce never depends on real wall-clock timing.
    fn preview_app(preview: Option<ProposalPreview>, tile: Option<HeightTile>) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_resource::<HeatmapOverlayToggle>()
            .init_resource::<CapturedPreviewReports>()
            .insert_resource(LayerStack::new())
            .insert_resource(TerrainHeightField::default())
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: None,
                assignment_fingerprint: None,
                input_scale_valid: false,
                petal_id: Some("petal".into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    ..TerrainConfig::default()
                }),
                revision: 1,
            })
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                20,
            )))
            .add_message::<ProposalPreviewReport>()
            .add_systems(
                Update,
                (render_proposal_preview, capture_preview_reports).chain(),
            );
        if let Some(tile) = tile {
            app.world_mut()
                .resource_mut::<TerrainHeightField>()
                .insert((10, 0, 0), tile);
        }
        if let Some(preview) = preview {
            app.insert_resource(preview);
        }
        app
    }

    fn preview_ghost_entity(app: &mut App) -> Option<Entity> {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, (
            With<ProposalPreviewEntity>,
            With<fe_renderer::terrain_overlay::ProposalGhost>,
        )>();
        q.iter(world).next()
    }

    fn preview_ghost_vertex_count(app: &mut App) -> usize {
        let world = app.world_mut();
        let mut q = world.query_filtered::<&Mesh3d, (
            With<ProposalPreviewEntity>,
            With<fe_renderer::terrain_overlay::ProposalGhost>,
        )>();
        let Some(handle) = q.iter(world).next().cloned() else {
            return 0;
        };
        let meshes = world.resource::<Assets<Mesh>>();
        meshes
            .get(&handle.0)
            .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
            .and_then(|a| a.as_float3())
            .map(|p| p.len())
            .unwrap_or(0)
    }

    fn preview_heatmap_entities(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, (
            With<ProposalPreviewEntity>,
            Without<fe_renderer::terrain_overlay::ProposalGhost>,
        )>();
        q.iter(world).collect()
    }

    #[test]
    fn armed_preview_spawns_ghost_and_heatmap_without_db_or_heightfield_write() {
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 20.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut app = preview_app(Some(preview_draft(footprint, 2.0)), Some(tile));
        let field_len_before = app.world().resource::<TerrainHeightField>().len();
        let revision_before = app.world().resource::<ActivePetalTerrain>().revision;

        app.update(); // first sight -> coarse build immediately (NFR-3)

        assert!(
            preview_ghost_entity(&mut app).is_some(),
            "ghost spawned without waiting for the debounce"
        );
        assert_eq!(
            preview_heatmap_entities(&mut app).len(),
            1,
            "heatmap drape spawned alongside the ghost"
        );
        assert_eq!(
            app.world().resource::<TerrainHeightField>().len(),
            field_len_before,
            "preview never touches the height field (also structurally true: Option<Res<_>>)"
        );
        assert_eq!(
            app.world().resource::<ActivePetalTerrain>().revision,
            revision_before,
            "preview never bumps the revision — fe-terrain has no DbCommand channel to begin with"
        );
    }

    #[test]
    fn clearing_preview_despawns_preview_entities() {
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 20.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut app = preview_app(Some(preview_draft(footprint, 2.0)), Some(tile));
        app.update();
        assert!(preview_ghost_entity(&mut app).is_some(), "preview spawned");

        app.world_mut().remove_resource::<ProposalPreview>();
        app.update();
        assert!(
            preview_ghost_entity(&mut app).is_none(),
            "cleared draft despawns the ghost"
        );
        assert!(
            preview_heatmap_entities(&mut app).is_empty(),
            "and its heatmap drape"
        );
    }

    #[test]
    fn preview_report_matches_committed_volume_for_same_inputs() {
        let tile_grid = vec![0.0, 4.0, 2.0, 6.0];
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 20.0,
            grid: tile_grid.clone(),
            width: 2,
            height: 2,
        };
        let mut app = preview_app(Some(preview_draft(footprint.clone(), 2.0)), Some(tile));
        app.update(); // coarse build
        app.update(); // 20ms later (< debounce) -> no second report yet... settle below

        // Hold steady past the 100 ms debounce (6 x 20ms ticks) to force the
        // settled, FULL-resolution rebuild whose figures this test compares.
        for _ in 0..6 {
            app.update();
        }

        let report = app
            .world()
            .resource::<CapturedPreviewReports>()
            .0
            .last()
            .cloned()
            .expect("a preview report was written");

        // Ground truth: the EXACT SAME machinery the preview itself calls
        // (not an approximation), so this is a bit-identical FR-5 check.
        let base_tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 20.0,
            grid: tile_grid,
            width: 2,
            height: 2,
        };
        let base = |x: f32, z: f32| base_tile.sample(x, z);
        let draft = crate::terrain_proposal::TerrainProposal {
            id: "p".into(),
            op: TerrainOp::Raise,
            footprint: footprint.clone(),
            target_height: Some(0.0),
            delta: Some(2.0),
            material: None,
        };
        let proposed = |x: f32, z: f32| {
            crate::terrain_proposal::apply_proposal_over_base(&base, &draft, &[[x, z]])
                .into_iter()
                .next()
                .flatten()
        };
        let footprint_shape = crate::sculpt::Footprint::Polygon { verts: footprint };
        let grid_step = region_grid_step(footprint_shape.bounds());
        let expected = DeltaLattice::build(&base, &proposed, &footprint_shape, grid_step, 1.0)
            .expect("lattice built");

        assert_eq!(report.cut_m3, expected.cut_fill.cut_m3);
        assert_eq!(report.fill_m3, expected.cut_fill.fill_m3);
    }

    #[test]
    fn rapid_preview_edits_coalesce_within_debounce() {
        let tile = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 20.0,
            grid: vec![0.0; 4],
            width: 2,
            height: 2,
        };
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut app = preview_app(None, Some(tile));

        // Several rapid edits, each well inside the 100 ms window (20 ms ticks).
        for delta in [2.0, 2.1, 2.2, 2.3] {
            app.insert_resource(preview_draft(footprint.clone(), delta));
            app.update();
        }
        let coarse_count = preview_ghost_vertex_count(&mut app);
        assert!(
            coarse_count > 0,
            "a coarse preview is visible on every edit"
        );

        // Hold the LAST value steady past the debounce window -> exactly one
        // upgrade to the full (finer) lattice.
        for _ in 0..6 {
            app.update();
        }
        let full_count = preview_ghost_vertex_count(&mut app);
        assert!(
            full_count > coarse_count,
            "settled preview upgrades to the full lattice: coarse={coarse_count} full={full_count}"
        );

        // Further idle frames do not rebuild again (steady state - no churn).
        let stable = preview_ghost_entity(&mut app);
        app.update();
        assert_eq!(
            preview_ghost_entity(&mut app),
            stable,
            "steady state: no further rebuild once settled"
        );
    }

    // --- Pure PreviewDebounce logic (no App/Time needed) ---

    fn snap(delta_m: f32) -> PreviewSnapshot {
        PreviewSnapshot {
            petal_id: "p".into(),
            op: "raise".into(),
            footprint: vec![[0.0, 0.0], [1.0, 1.0]],
            target_m: 0.0,
            delta_m,
            strength: 1.0,
            source: PreviewSource::PaletteOp,
        }
    }

    #[test]
    fn preview_debounce_builds_coarse_on_change_then_full_once_settled() {
        let mut d = PreviewDebounce::default();
        let a = snap(1.0);
        assert_eq!(
            d.poll(Some(&a), 0.0),
            Some(PreviewAction::Build { full: false }),
            "first sight -> coarse immediately"
        );
        assert_eq!(
            d.poll(Some(&a), 0.05),
            None,
            "unchanged, still inside the debounce window"
        );
        assert_eq!(
            d.poll(Some(&a), 0.1),
            Some(PreviewAction::Build { full: true }),
            "settled -> one full rebuild"
        );
        assert_eq!(
            d.poll(Some(&a), 0.2),
            None,
            "steady state: no further rebuilds"
        );
    }

    #[test]
    fn preview_debounce_coalesces_rapid_edits_until_settled() {
        let mut d = PreviewDebounce::default();
        for (t, delta) in [(0.0, 1.0), (0.02, 1.1), (0.04, 1.2), (0.06, 1.3)] {
            assert_eq!(
                d.poll(Some(&snap(delta)), t),
                Some(PreviewAction::Build { full: false }),
                "each change -> coarse, never full while still changing"
            );
        }
        assert_eq!(
            d.poll(Some(&snap(1.3)), 0.10),
            None,
            "only 0.04s since the last change (0.06 -> 0.10)"
        );
        assert_eq!(
            d.poll(Some(&snap(1.3)), 0.17),
            Some(PreviewAction::Build { full: true }),
            "0.11s stable -> settle to full"
        );
    }

    #[test]
    fn preview_debounce_clear_despawns_once() {
        let mut d = PreviewDebounce::default();
        d.poll(Some(&snap(1.0)), 0.0);
        assert_eq!(d.poll(None, 1.0), Some(PreviewAction::Clear));
        assert_eq!(
            d.poll(None, 2.0),
            None,
            "already cleared — no repeat despawn churn"
        );
    }

    #[test]
    fn selected_proposal_preview_reports_without_duplicate_meshes() {
        // Source (d) is readout-only: its committed ghost + drape already
        // render, so the preview must report cut/fill but spawn nothing (a
        // second identical translucent lattice z-fights / double-tints).
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut draft = preview_draft(footprint, 2.0);
        draft.source = PreviewSource::SelectedProposal;
        let mut app = preview_app(Some(draft), Some(flat_tile(Vec3::ZERO, 20.0)));
        app.update();

        let reports = &app.world().resource::<CapturedPreviewReports>().0;
        let report = reports.last().expect("readout report written");
        assert!(report.fill_m3 > 0.0 && report.scaled);
        assert!(
            preview_ghost_entity(&mut app).is_none(),
            "no duplicate ghost"
        );
        assert!(
            preview_heatmap_entities(&mut app).is_empty(),
            "no duplicate drape"
        );
    }

    #[test]
    fn preview_spawned_under_hidden_layer_starts_hidden() {
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut app = preview_app(
            Some(preview_draft(footprint, 2.0)),
            Some(flat_tile(Vec3::ZERO, 20.0)),
        );
        let mut layer = crate::layers::MapLayer::new(LayerType::ProposalOverlay, 0);
        layer.visible = false;
        app.world_mut()
            .resource_mut::<LayerStack>()
            .add_layer(layer);
        app.update();

        let world = app.world_mut();
        let mut q = world.query_filtered::<&Visibility, With<ProposalPreviewEntity>>();
        let visibilities: Vec<_> = q.iter(world).copied().collect();
        assert_eq!(visibilities.len(), 2, "ghost + drape spawned");
        assert!(
            visibilities.iter().all(|v| *v == Visibility::Hidden),
            "a live preview must not punch through a hidden overlay layer"
        );
    }

    // --- P2 S5 volume-report request/reply round trip (VOLUME-REPORT-SEAM) ---

    use fe_renderer::terrain_overlay::{VolumeReportRequest, VolumeReportResult};

    #[derive(Resource, Default)]
    struct CapturedVolumeResults(Vec<VolumeReportResult>);

    fn capture_volume_results(
        mut reader: MessageReader<VolumeReportResult>,
        mut sink: ResMut<CapturedVolumeResults>,
    ) {
        for msg in reader.read() {
            sink.0.push(msg.clone());
        }
    }

    /// Headless app running `answer_volume_report_requests` for `petal_id`,
    /// with `proposals` on its `TerrainConfig` and an optional covering tile.
    fn volume_report_app(
        proposals: Vec<TerrainProposal>,
        petal_id: &str,
        tile: Option<HeightTile>,
    ) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CapturedVolumeResults>()
            .insert_resource(TerrainHeightField::default())
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: None,
                assignment_fingerprint: None,
                input_scale_valid: false,
                petal_id: Some(petal_id.into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    proposals,
                    ..TerrainConfig::default()
                }),
                revision: 1,
            })
            .add_message::<VolumeReportRequest>()
            .add_message::<VolumeReportResult>()
            .add_systems(
                Update,
                (answer_volume_report_requests, capture_volume_results).chain(),
            );
        if let Some(tile) = tile {
            app.world_mut()
                .resource_mut::<TerrainHeightField>()
                .insert((10, 0, 0), tile);
        }
        app
    }

    #[test]
    fn volume_request_round_trip_answers_matching_correlation() {
        // Same 10x10 footprint, flat ground, Raise delta=2.0 as `square_proposal`
        // — ground truth fill = area(100) * delta(2) = 200 m3 at world_scale=1.0.
        let tile = flat_tile(Vec3::ZERO, 20.0);
        let proposal = square_proposal("p1", TerrainOp::Raise, 0.0);
        let mut app = volume_report_app(vec![proposal], "petal", Some(tile));

        app.world_mut().write_message(VolumeReportRequest {
            correlation_id: "corr-1".into(),
            petal_id: "petal".into(),
            footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            elevation_band: None,
        });
        app.update();

        let results = &app.world().resource::<CapturedVolumeResults>().0;
        assert_eq!(
            results.len(),
            1,
            "exactly one reply for the matching request"
        );
        let result = &results[0];
        assert_eq!(
            result.correlation_id, "corr-1",
            "reply must echo the request's own correlation id"
        );
        assert!(result.scaled, "world_scale=1.0 is a valid scale");
        assert!(
            (result.fill_m3 - 200.0).abs() / 200.0 < 0.05,
            "raise 2 over a flat 10x10 should fill ~200 m3, got {}",
            result.fill_m3
        );
        assert!(result.cut_m3.abs() < 1.0, "a pure raise has no cut");
        assert_eq!(result.items.len(), 1);
        assert_eq!(
            result.items[0].kind, "raise",
            "a plain palette proposal reports its op tag as kind"
        );
        assert!(!result.has_overlap, "a single item cannot overlap itself");
        assert_eq!(result.expected_samples, 128 * 128);
        assert_eq!(result.sampled_pairs, result.expected_samples);
        assert!(result.is_complete());

        app.world_mut().resource_mut::<TerrainHeightField>().clear();
        app.update();
        let missing = app
            .world()
            .resource::<CapturedVolumeResults>()
            .0
            .last()
            .unwrap();
        assert!(!missing.is_complete());
        assert_eq!(missing.sampled_pairs, 0);
        let count = app.world().resource::<CapturedVolumeResults>().0.len();
        app.update();
        assert_eq!(
            app.world().resource::<CapturedVolumeResults>().0.len(),
            count
        );

        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::ZERO, 5.0));
        app.update();
        let partial = app
            .world()
            .resource::<CapturedVolumeResults>()
            .0
            .last()
            .unwrap();
        assert!(!partial.is_complete());
        assert!(partial.sampled_pairs > 0 && partial.sampled_pairs < partial.expected_samples);

        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::ZERO, 20.0));
        app.update();
        let loaded = app
            .world()
            .resource::<CapturedVolumeResults>()
            .0
            .last()
            .unwrap();
        assert_eq!(loaded.correlation_id, "corr-1");
        assert!(loaded.is_complete());
        assert!((loaded.fill_m3 - 200.0).abs() < 0.001);
        {
            let mut active = app.world_mut().resource_mut::<ActivePetalTerrain>();
            active.config.as_mut().unwrap().proposals[0].delta = Some(3.0);
            active.revision += 1;
        }
        app.update();
        let latest = app
            .world()
            .resource::<CapturedVolumeResults>()
            .0
            .last()
            .unwrap();
        assert_eq!(latest.correlation_id, "corr-1");
        assert!((latest.fill_m3 - 300.0).abs() < 0.001);

        app.world_mut().resource_mut::<ActivePetalTerrain>().config = None;
        app.world_mut().write_message(VolumeReportRequest {
            correlation_id: "corr-no-map".into(),
            petal_id: "petal".into(),
            footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            elevation_band: None,
        });
        let before_no_map = app.world().resource::<CapturedVolumeResults>().0.len();
        app.update();
        let no_map_results = &app.world().resource::<CapturedVolumeResults>().0;
        assert_eq!(
            no_map_results.len(),
            before_no_map + 1,
            "a request on a petal without a map gets exactly one reply"
        );
        let no_map = no_map_results.last().unwrap();
        assert_eq!(no_map.correlation_id, "corr-no-map");
        assert_eq!(
            no_map.unavailable,
            Some(fe_renderer::terrain_overlay::VolumeUnavailable::NoMap)
        );
        assert!(!no_map.is_complete() && no_map.items.is_empty());
        let count = app.world().resource::<CapturedVolumeResults>().0.len();
        {
            let mut active = app.world_mut().resource_mut::<ActivePetalTerrain>();
            active.config = Some(TerrainConfig {
                enabled: true,
                ..Default::default()
            });
            active.revision += 1;
        }
        app.update();
        assert_eq!(
            app.world().resource::<CapturedVolumeResults>().0.len(),
            count,
            "unbinding discards the retained request even if the same petal is rebound"
        );
        // The persistence producer must remeasure on coverage changes with no resident mesh.
        use fe_renderer::terrain_overlay::{EarthworkVolumeReport, EarthworkVolumeState};
        let mut bake = App::new();
        bake.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<TerrainHeightField>()
            .init_resource::<EarthworkBakeState>()
            .init_resource::<EarthworkVolumeState>()
            .add_message::<EarthworkVolumeReport>()
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: Some([1; 32]),
                assignment_fingerprint: Some([2; 32]),
                input_scale_valid: true,
                petal_id: Some("measured-petal".into()),
                revision: 1,
                config: Some(TerrainConfig {
                    enabled: true,
                    proposals: vec![TerrainProposal {
                        material: Some("earth".into()),
                        ..square_proposal("measured-region", TerrainOp::Raise, 2.0)
                    }],
                    ..Default::default()
                }),
            })
            .add_systems(Update, bake_earthwork_regions);
        bake.update();
        let missing = &bake.world().resource::<EarthworkVolumeState>().reports["measured-region"];
        assert!(missing.expected_cells > 0);
        assert_eq!(missing.sampled_cells, 0);
        assert!(missing.pair.is_none());
        let missing_source = missing.source.clone();
        bake.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::ZERO, 20.0));
        bake.update();
        let ready = &bake.world().resource::<EarthworkVolumeState>().reports["measured-region"];
        assert!(
            ready.pair.is_some(),
            "height arrival remeasures even when there is no mesh to bake"
        );
        assert_ne!(
            ready.source.height_generation,
            missing_source.height_generation
        );
        assert_eq!(ready.expected_cells, ready.sampled_cells);
        {
            let mut active = bake.world_mut().resource_mut::<ActivePetalTerrain>();
            active.config.as_mut().unwrap().proposals[0].delta = Some(0.0);
            active.revision += 1;
        }
        bake.update();
        let zero = &bake.world().resource::<EarthworkVolumeState>().reports["measured-region"];
        let pair = zero
            .pair
            .expect("complete zero is measured, not unavailable");
        assert_eq!((pair.cut_m3, pair.fill_m3), (0.0, 0.0));
        bake.world_mut()
            .resource_mut::<TerrainHeightField>()
            .clear();
        bake.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert((10, 0, 0), flat_tile(Vec3::ZERO, 5.0));
        bake.update();
        let partial = &bake.world().resource::<EarthworkVolumeState>().reports["measured-region"];
        assert!(partial.sampled_cells > 0 && partial.sampled_cells < partial.expected_cells);
        assert!(partial.pair.is_none());
        {
            let mut active = bake.world_mut().resource_mut::<ActivePetalTerrain>();
            active.petal_id = Some("foreign-petal".into());
            active.revision += 1;
        }
        bake.update();
        assert_eq!(
            bake.world()
                .resource::<EarthworkVolumeState>()
                .source
                .as_ref()
                .unwrap()
                .petal_id,
            "foreign-petal"
        );
    }

    #[test]
    fn request_for_inactive_petal_is_dropped() {
        let tile = flat_tile(Vec3::ZERO, 20.0);
        let proposal = square_proposal("p1", TerrainOp::Raise, 0.0);
        let mut app = volume_report_app(vec![proposal], "petal", Some(tile));

        // Mutation check: removing the petal-scope guard would make this
        // request fall through to the SAME report `volume_request_round_trip_
        // answers_matching_correlation` expects — turning this assertion red.
        app.world_mut().write_message(VolumeReportRequest {
            correlation_id: "corr-2".into(),
            petal_id: "some-other-petal".into(),
            footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            elevation_band: None,
        });
        app.update();

        let results = &app.world().resource::<CapturedVolumeResults>().0;
        assert_eq!(results.len(), 1, "a foreign-petal request gets one reply");
        assert_eq!(results[0].correlation_id, "corr-2");
        assert_eq!(
            results[0].unavailable,
            Some(fe_renderer::terrain_overlay::VolumeUnavailable::WrongPetal)
        );
        app.update();
        assert_eq!(
            app.world().resource::<CapturedVolumeResults>().0.len(),
            1,
            "the reply is not repeated every frame"
        );
    }

    // --- multi-focus streaming flows (agent_vision_20261031 V4) ---
    //
    // Real seam: the chained `update_terrain_lod` → `fetch_and_spawn_terrain_chunks`
    // pass over a real `CompositeTileSource` — an installed in-memory hexon under the
    // main camera and a P2P-streamed tileset under the remote capture target.

    mod multi_focus {
        use std::collections::BTreeSet;

        use super::*;
        use crate::tiles::{CompositeTileSource, HexonTileSource, StreamedTileSource};
        use fe_format::manifest::{
            BundleEntry, ElevationEncoding, HexonManifest, HexonType, TilesetBundleIndex,
            TilesetMeta,
        };
        use fe_renderer::camera::OrbitCameraController;

        /// Fixed zoom so each focus's ring is exactly its 3×3 at this zoom.
        const ZOOM: u8 = 12;
        const ORIGIN_LAT: f64 = 10.0;
        const ORIGIN_LON: f64 = 10.0;
        /// ~110 km east of the origin: far beyond either focus's despawn radius.
        const CAPTURE_LON: f64 = 11.0;
        const CAMERA_HEIGHT: f32 = 100.0;
        const CAPTURE_TILESET: &str = "capture-ts";

        type Key = (u8, u32, u32);

        fn projection() -> Projection {
            Projection::new(ORIGIN_LAT, ORIGIN_LON, 0.0)
        }

        /// World position of a camera hovering over `lon` (lat = origin).
        fn camera_world(lon: f64) -> Vec3 {
            let local = projection()
                .wgs84_to_local(ORIGIN_LAT, lon, 0.0)
                .expect("in range");
            Vec3::new(local[0] as f32, CAMERA_HEIGHT, local[2] as f32)
        }

        /// The ring tile the fetch pass centres on for a camera at `world`.
        fn ring_center(world: Vec3, zoom: u8) -> TileCoord {
            let (lat, lon, _) =
                projection().local_to_wgs84(world.x as f64, world.y as f64, world.z as f64);
            TileCoord::from_lat_lon(lat, lon, zoom)
        }

        /// The 3×3 ring keys around `center` (at its zoom).
        fn ring(center: TileCoord) -> BTreeSet<Key> {
            let mut keys = BTreeSet::new();
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    keys.insert((
                        center.zoom,
                        (center.x as i64 + dx) as u32,
                        (center.y as i64 + dy) as u32,
                    ));
                }
            }
            keys
        }

        /// Geographic bounds `[min_lat, min_lon, max_lat, max_lon]` of a key set.
        fn bounds_of(keys: &BTreeSet<Key>) -> [f64; 4] {
            let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for &(z, x, y) in keys {
                let (nw_lat, nw_lon) = TileCoord::new(x, y, z).to_lat_lon();
                let (se_lat, se_lon) = TileCoord::new(x + 1, y + 1, z).to_lat_lon();
                bounds[0] = bounds[0].min(se_lat);
                bounds[1] = bounds[1].min(nw_lon);
                bounds[2] = bounds[2].max(nw_lat);
                bounds[3] = bounds[3].max(se_lon);
            }
            bounds
        }

        /// A decodable 4×4 Terrain-RGB tile at 0 m.
        fn elevation_png() -> Vec<u8> {
            let image = image::RgbImage::from_pixel(4, 4, image::Rgb([1, 134, 160]));
            let mut bytes = Vec::new();
            image
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .expect("encode png");
            bytes
        }

        fn meta(keys: &BTreeSet<Key>, name: &str) -> TilesetMeta {
            TilesetMeta {
                bounds: bounds_of(keys),
                min_zoom: keys.iter().map(|k| k.0).min().unwrap_or(ZOOM),
                max_zoom: keys.iter().map(|k| k.0).max().unwrap_or(ZOOM),
                tile_size: 4,
                elevation_encoding: ElevationEncoding::TerrainRgb,
                tile_count: keys.len() as u32,
                region_name: name.into(),
                ..Default::default()
            }
        }

        fn manifest(id: &str) -> HexonManifest {
            HexonManifest {
                schema_version: "1.0.0".into(),
                hexon_id: id.into(),
                hexon_type: HexonType::TerrainTileset,
                publisher_did: "did:key:ztest".into(),
                publisher_name: None,
                version: "0.1.0".into(),
                build_id: None,
                name: id.into(),
                description: None,
                tags: vec![],
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                source_peer_did: None,
                approx_size_bytes: None,
                min_engine_version: None,
                homepage_url: None,
                dependencies: vec![],
                platforms: vec![],
                address: None,
                integrity: None,
                signature: None,
            }
        }

        /// An in-memory tileset holding a real elevation tile for every key.
        fn tileset(keys: &BTreeSet<Key>, name: &str) -> HexonTileSource {
            let png = elevation_png();
            let tiles: Vec<(String, Vec<u8>)> = keys
                .iter()
                .map(|&(z, x, y)| (format!("{z}/{x}/{y}"), png.clone()))
                .collect();
            let archive = fe_format::HexonArchive::export_tileset(
                manifest(name),
                &meta(keys, name),
                &tiles,
                &[],
                None,
            )
            .expect("export tileset");
            HexonTileSource::from_archive(&archive).expect("import tileset")
        }

        /// The capture area as a not-installed tileset: one bundle over its ring.
        fn streamed_capture_source(keys: &BTreeSet<Key>) -> StreamedTileSource {
            let xs = keys.iter().map(|k| k.1);
            let ys = keys.iter().map(|k| k.2);
            let x_range = [xs.clone().min().unwrap(), xs.max().unwrap()];
            let y_range = [ys.clone().min().unwrap(), ys.max().unwrap()];
            StreamedTileSource::new(
                CAPTURE_TILESET.into(),
                TilesetBundleIndex {
                    schema: 1,
                    tileset_id: CAPTURE_TILESET.into(),
                    tileset_meta: meta(keys, CAPTURE_TILESET),
                    bundles: vec![BundleEntry {
                        seq: 0,
                        blake3_hex: "unused-in-this-flow".into(),
                        byte_len: 0,
                        tile_count: keys.len() as u32,
                        zoom: ZOOM,
                        x_range,
                        y_range,
                        hilbert_range: [0, keys.len() as u64 - 1],
                    }],
                },
            )
        }

        struct Fixture {
            app: App,
            main_ring: BTreeSet<Key>,
            capture_ring: BTreeSet<Key>,
        }

        /// Headless app running the LOD → fetch chain on the two-area composite.
        fn fixture(capture_bundle_resident: bool) -> Fixture {
            let main_ring = ring(ring_center(camera_world(ORIGIN_LON), ZOOM));
            let capture_ring = ring(ring_center(camera_world(CAPTURE_LON), ZOOM));
            assert!(main_ring.is_disjoint(&capture_ring));

            let mut composite = CompositeTileSource::new();
            composite.add_hexon_source(tileset(&main_ring, "main"));
            composite.add_streamed_source(streamed_capture_source(&capture_ring));
            if capture_bundle_resident {
                composite.insert_streamed_bundle(
                    CAPTURE_TILESET,
                    0,
                    tileset(&capture_ring, CAPTURE_TILESET),
                );
            }
            Fixture {
                app: streaming_app(composite, ZOOM),
                main_ring,
                capture_ring,
            }
        }

        /// The LOD → fetch chain over `composite`, zooms `min_zoom..=ZOOM`.
        fn streaming_app(composite: CompositeTileSource, min_zoom: u8) -> App {
            let mut app = App::new();
            app.add_plugins((
                MinimalPlugins,
                AssetPlugin::default(),
                bevy::transform::TransformPlugin,
            ))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_resource::<TerrainLodConfig>()
            .init_resource::<TileResidency>()
            .init_resource::<TileBundleDemand>()
            .init_resource::<FailedTiles>()
            .init_resource::<CaptureFocusResidency>()
            .insert_resource(LayerStack::new())
            .insert_resource(TerrainHeightField::default())
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: None,
                assignment_fingerprint: None,
                input_scale_valid: false,
                petal_id: Some("petal".into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    origin: projection(),
                    elevation_source: ElevationSourceKind::TerrainRgb,
                    min_zoom,
                    max_zoom: ZOOM,
                    world_scale: 1.0,
                    ..TerrainConfig::default()
                }),
                revision: 1,
            })
            .insert_resource(ActiveTileSource {
                composite: Some(composite),
                projection: Some(projection()),
            })
            .add_systems(
                Update,
                (update_terrain_lod, fetch_and_spawn_terrain_chunks).chain(),
            );
            app
        }

        /// A camera with an explicit `GlobalTransform` so frame one already sees its pose.
        fn camera_at(world: Vec3) -> impl Bundle {
            let transform = Transform::from_translation(world).looking_to(Vec3::NEG_Y, Vec3::Z);
            (
                Camera::default(),
                CameraProjection::Perspective(PerspectiveProjection::default()),
                transform,
                GlobalTransform::from(transform),
            )
        }

        fn spawn_main(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    camera_at(camera_world(ORIGIN_LON)),
                    OrbitCameraController::default(),
                    StreamingFocus,
                ))
                .id()
        }

        fn spawn_capture(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    camera_at(camera_world(CAPTURE_LON)),
                    CaptureCamera,
                    StreamingFocus,
                ))
                .id()
        }

        fn run(app: &mut App, frames: usize) {
            for _ in 0..frames {
                app.update();
            }
        }

        fn chunk_keys(app: &mut App) -> BTreeSet<Key> {
            let world = app.world_mut();
            let mut query = world.query::<&TerrainChunk>();
            query.iter(world).map(|chunk| chunk.tile_coords).collect()
        }

        fn chunk_entities(app: &mut App) -> BTreeSet<Entity> {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<TerrainChunk>>();
            query.iter(world).collect()
        }

        fn union(a: &BTreeSet<Key>, b: &BTreeSet<Key>) -> BTreeSet<Key> {
            a.union(b).copied().collect()
        }

        /// Regression: a second `Camera` (e.g. a glb scene camera) used to make the
        /// single-camera `.single()` fail and stop ALL streaming silently.
        #[test]
        fn two_camera_entities_do_not_stall_streaming() {
            let mut fx = fixture(true);
            spawn_main(&mut fx.app);
            fx.app
                .world_mut()
                .spawn(camera_at(camera_world(CAPTURE_LON)));

            run(&mut fx.app, 3);

            assert_eq!(
                chunk_keys(&mut fx.app),
                fx.main_ring,
                "the orbit focus streams its ring; a camera without StreamingFocus streams nothing"
            );
        }

        /// A host without fe-renderer's orbit camera (no `StreamingFocus` anywhere)
        /// keeps streaming around its one camera; two such cameras are ambiguous.
        #[test]
        fn a_lone_plain_camera_still_streams_but_two_are_ambiguous() {
            let mut fx = fixture(true);
            let lone = fx
                .app
                .world_mut()
                .spawn(camera_at(camera_world(ORIGIN_LON)))
                .id();
            run(&mut fx.app, 3);
            assert_eq!(chunk_keys(&mut fx.app), fx.main_ring);

            // Either camera, if picked, would stream the remote ring and evict this one.
            fx.app.world_mut().despawn(lone);
            for _ in 0..2 {
                fx.app
                    .world_mut()
                    .spawn(camera_at(camera_world(CAPTURE_LON)));
            }
            run(&mut fx.app, 3);
            assert_eq!(
                chunk_keys(&mut fx.app),
                fx.main_ring,
                "no focus is chosen between two plain cameras: nothing new streams"
            );
        }

        #[test]
        fn capture_focus_spawns_chunks_at_its_remote_target() {
            let mut fx = fixture(true);
            spawn_main(&mut fx.app);
            let capture = spawn_capture(&mut fx.app);

            run(&mut fx.app, 3);

            assert_eq!(
                chunk_keys(&mut fx.app),
                union(&fx.main_ring, &fx.capture_ring),
                "both foci stream their own ring"
            );
            assert_eq!(
                *fx.app.world().resource::<CaptureFocusResidency>(),
                CaptureFocusResidency {
                    capture_focus: Some(capture),
                    pending: 0,
                }
            );
        }

        #[test]
        fn main_view_chunks_survive_while_a_capture_focus_streams_elsewhere() {
            let mut fx = fixture(true);
            spawn_main(&mut fx.app);
            run(&mut fx.app, 3);
            let main_chunks = chunk_entities(&mut fx.app);
            assert_eq!(main_chunks.len(), fx.main_ring.len());

            spawn_capture(&mut fx.app);
            run(&mut fx.app, 5);

            let now = chunk_entities(&mut fx.app);
            assert!(
                main_chunks.is_subset(&now),
                "no chunk of the user's view may be evicted (or respawned) by a capture"
            );
            assert_eq!(
                chunk_keys(&mut fx.app),
                union(&fx.main_ring, &fx.capture_ring)
            );
        }

        /// The user's view keeps its own budget: a capture that already holds
        /// chunks (its full allowance, with `max_chunks` = the view's ring)
        /// never pauses the view's fill, and stays inside its allowance.
        #[test]
        fn a_capture_never_spends_the_user_views_chunk_budget() {
            let mut fx = fixture(true);
            let max_chunks = fx.main_ring.len();
            fx.app.insert_resource(TerrainLodConfig {
                max_chunks,
                ..TerrainLodConfig::default()
            });
            spawn_capture(&mut fx.app);
            run(&mut fx.app, 3);
            let capture_chunks = chunk_keys(&mut fx.app);
            assert!(!capture_chunks.is_empty());
            assert!(capture_chunks.len() <= capture_chunk_allowance(max_chunks));

            spawn_main(&mut fx.app);
            run(&mut fx.app, 3);
            let keys = chunk_keys(&mut fx.app);
            assert!(
                fx.main_ring.is_subset(&keys),
                "the user's ring fills in full beside the capture's chunks"
            );
            assert!(
                capture_chunks.is_subset(&keys),
                "the capture keeps what it holds"
            );
        }

        #[test]
        fn removing_the_capture_focus_releases_its_chunks_on_the_next_lod_pass() {
            let mut fx = fixture(true);
            spawn_main(&mut fx.app);
            let capture = spawn_capture(&mut fx.app);
            run(&mut fx.app, 3);
            assert_eq!(
                chunk_keys(&mut fx.app),
                union(&fx.main_ring, &fx.capture_ring)
            );
            let streamed_resident = |app: &App| {
                app.world()
                    .resource::<ActiveTileSource>()
                    .composite
                    .as_ref()
                    .expect("composite")
                    .streamed_resident_bundles()
            };
            assert_eq!(
                streamed_resident(&fx.app),
                1,
                "capture chunks pin their bundle"
            );

            fx.app.world_mut().despawn(capture);
            run(&mut fx.app, 1);

            assert_eq!(chunk_keys(&mut fx.app), fx.main_ring);
            assert_eq!(
                fx.app.world().resource::<TerrainHeightField>().len(),
                fx.main_ring.len(),
                "the capture chunks' height entries left with them"
            );
            assert_eq!(
                streamed_resident(&fx.app),
                0,
                "the bundle's bytes leave RAM with the last capture chunk (exit ring)"
            );
            assert_eq!(
                fx.app
                    .world()
                    .resource::<CaptureFocusResidency>()
                    .capture_focus,
                None
            );
        }

        #[test]
        fn capture_pending_count_reaches_zero_when_its_ring_is_resident() {
            let mut fx = fixture(false);
            spawn_main(&mut fx.app);
            let capture = spawn_capture(&mut fx.app);
            run(&mut fx.app, 2);

            assert_eq!(
                *fx.app.world().resource::<CaptureFocusResidency>(),
                CaptureFocusResidency {
                    capture_focus: Some(capture),
                    pending: fx.capture_ring.len(),
                },
                "every capture ring tile awaits the not-yet-arrived bundle"
            );
            let wanted: Vec<(String, u32)> = fx
                .app
                .world()
                .resource::<TileBundleDemand>()
                .wanted()
                .cloned()
                .collect();
            assert_eq!(wanted, vec![(CAPTURE_TILESET.to_string(), 0)]);
            assert_eq!(chunk_keys(&mut fx.app), fx.main_ring);

            let bundle = tileset(&fx.capture_ring, CAPTURE_TILESET);
            fx.app
                .world_mut()
                .resource_mut::<ActiveTileSource>()
                .composite
                .as_mut()
                .expect("composite")
                .insert_streamed_bundle(CAPTURE_TILESET, 0, bundle);
            run(&mut fx.app, 2);

            assert_eq!(
                fx.app.world().resource::<CaptureFocusResidency>().pending,
                0
            );
            assert_eq!(
                chunk_keys(&mut fx.app),
                union(&fx.main_ring, &fx.capture_ring)
            );
        }

        /// Overlapping foci at different zooms: the capture hovers higher over the
        /// user's view (wants zoom 11; main wants 12). A zoom-11 tile whose four
        /// zoom-12 children the main ring already holds is stale for its nearest
        /// focus (main), so the capture must not spawn it — else the LOD pass would
        /// evict it and the fetch respawn it, every frame.
        #[test]
        fn overlapping_foci_at_different_zooms_settle_without_churn() {
            let main_world = camera_world(ORIGIN_LON);
            let capture_world = main_world + Vec3::Y * 400.0;
            let main_ring = ring(ring_center(main_world, ZOOM));
            let capture_ring = ring(ring_center(capture_world, ZOOM - 1));
            let mut composite = CompositeTileSource::new();
            composite.add_hexon_source(tileset(&union(&main_ring, &capture_ring), "both"));
            let mut app = streaming_app(composite, ZOOM - 1);
            app.world_mut().spawn((
                camera_at(main_world),
                OrbitCameraController::default(),
                StreamingFocus,
            ));
            app.world_mut()
                .spawn((camera_at(capture_world), CaptureCamera, StreamingFocus));

            run(&mut app, 4);
            let settled = chunk_entities(&mut app);
            run(&mut app, 3);

            assert_eq!(
                chunk_entities(&mut app),
                settled,
                "no chunk is despawned and respawned once both rings are in"
            );
            let covered_by_main: Vec<Key> = capture_ring
                .iter()
                .copied()
                .filter(|&(z, x, y)| {
                    crate::lod_ring::covering_tiles(TileCoord::new(x, y, z), ZOOM)
                        .iter()
                        .all(|child| main_ring.contains(child))
                })
                .collect();
            assert_eq!(
                covered_by_main.len(),
                1,
                "a 3×3 ring always holds one aligned 2×2"
            );
            let keys = chunk_keys(&mut app);
            assert!(main_ring.is_subset(&keys), "the user's view is intact");
            assert!(
                !keys.contains(&covered_by_main[0]),
                "the ceded tile is never spawned"
            );
            assert_eq!(keys.len(), main_ring.len() + capture_ring.len() - 1);
            assert_eq!(app.world().resource::<CaptureFocusResidency>().pending, 0);
        }
    }
}
