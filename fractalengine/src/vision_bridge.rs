//! Agent-vision capture orchestrator: the Bevy end of fe-api's posed
//! `POST /api/v1/debug/screenshot` and `GET /api/v1/debug/camera` — see
//! src/AGENTS.md §vision (state machine, restore/teardown guarantees).

use std::time::Duration;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::ecs::system::SystemParam;
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::screenshot::Screenshot;
use bevy::time::Real;
use bevy::transform::TransformSystems;
use bevy::window::PrimaryWindow;
use fe_renderer::camera::{
    compute_orbit_transform, frame_world_box, orbit_from_look_at, pose_transform,
    CameraCaptureLock, CameraScaleSettings, CaptureCamera, LookAtPose, OrbitCameraController,
    StreamingFocus,
};
use fe_renderer::terrain_height::TerrainHeightField;
use fe_runtime::vision::{
    denied, ActiveHierarchy, AltitudeReference, CameraState, CaptureMode, FocusTarget, PoseSource,
    ProjectionState, ResolvedGeo, ResolvedOrbit, ResolvedPose, ValidCaptureSpec, ValidPoseSpec,
    VisionFailure, VisionFailureClass, VisionOp, VisionOutput, VisionRequest,
    VisionRequestReceiver, WindowState, DEFAULT_FOV_DEG, DEFAULT_HEIGHT, DEFAULT_WIDTH, SETTLE_MAX,
};
use fe_terrain::camera_framing::{
    coverage_check, frame_bounds_top_down, geo_pose, tile_bounds, union_bounds,
    within_projection_range, Coverage, FramedPose,
};
use fe_terrain::petal_binding::{ActivePetalTerrain, ActiveTileSource};
use fe_terrain::projection::Projection as GeoProjection;
use fe_terrain::scale::sanitize_world_scale;
use fe_terrain::terrain_plugin::CaptureFocusResidency;
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::plugin::SpawnedNodeMarker;
use fe_ui::verse_manager::VerseManager;
use fe_ui::visibility::{effective_visibility, NodeAncestry, VisibilityState};

use crate::screenshot_bridge::{capture_png, capture_refusal};

/// Bevy-side settle wall-clock cap (spec §5.4(f)): past it the capture is
/// taken anyway and reported `settled: false`.
pub const SETTLE_WALL_CLOCK_CAP: Duration = Duration::from_secs(20);
/// Hard job cap: past it the job is torn down (camera restored, lock
/// released) — under fe-api's 5 s + 20 s reply timeout, so the agent reads
/// a typed failure instead of a 504.
pub const JOB_WALL_CLOCK_CAP: Duration = Duration::from_secs(24);
/// A render-to-texture image must be prepared on the GPU before a
/// `Screenshot` of it is extracted, or Bevy skips it and leaks the capture.
const SCENE_MIN_FRAMES: u32 = 2;
/// `focus.navigate`: frames to wait for the target petal's nodes to spawn.
const NAVIGATE_FRAME_CAP: u32 = 300;
/// Frames a freshly spawned target must exist before its transforms are read
/// (one propagation pass + one bounds pass).
const NAVIGATE_GRACE_FRAMES: u32 = 2;
/// The render layer every gizmo group draws on: the main camera sees it,
/// a capture camera (default layer 0 only) does not.
pub const GIZMO_RENDER_LAYER: usize = 1;

/// Bevy side of the API→Bevy vision bridge.
#[derive(Resource)]
pub struct VisionBridge(pub VisionRequestReceiver);

/// The single in-flight posed capture (spec §5.5 `VisionJob`); `None` = idle.
#[derive(Resource, Default)]
pub struct VisionJob(Option<ActiveJob>);

impl VisionJob {
    /// A posed capture is in flight (the render loop keeps waking for it).
    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }

    /// True while a window-mode job holds the main camera at its pose.
    fn holds_camera(&self) -> bool {
        self.0.as_ref().is_some_and(|job| job.holds_camera)
    }
}

/// A resolved camera pose in world units (`fov` in radians).
#[derive(Debug, Clone, Copy, PartialEq)]
struct PoseTarget {
    position: Vec3,
    target: Vec3,
    up: Vec3,
    fov: f32,
}

impl PoseTarget {
    fn transform(&self) -> Option<Transform> {
        pose_transform(self.position, self.target, self.up)
    }

    /// A pure look-at pose (fe-renderer framing math) at this job's `fov`.
    fn from_look_at(pose: LookAtPose, fov: f32) -> Self {
        Self {
            position: pose.position,
            target: pose.target,
            up: pose.up,
            fov,
        }
    }
}

enum JobPhase {
    /// `focus.navigate`: navigation issued, waiting for the target to spawn.
    Navigating {
        target: FocusTarget,
        frames: u32,
        seen_frames: u32,
    },
    /// Resolved; applied this frame (main camera or a new capture camera).
    Posing,
    Settling,
    /// A `Screenshot` is queued; its encoded PNG arrives on `result`.
    Capturing {
        result: crossbeam::channel::Receiver<Result<Vec<u8>, String>>,
    },
}

/// The main camera as it was before a window-mode job posed it.
struct CameraSnapshot {
    transform: Transform,
    focus: Vec3,
    distance: f32,
    yaw: f32,
    pitch: f32,
    target_distance: f32,
    target_focus: Option<Vec3>,
    fov: Option<f32>,
}

impl CameraSnapshot {
    fn take(
        transform: &Transform,
        controller: &OrbitCameraController,
        projection: &Projection,
    ) -> Self {
        Self {
            transform: *transform,
            focus: controller.focus,
            distance: controller.distance,
            yaw: controller.yaw,
            pitch: controller.pitch,
            target_distance: controller.target_distance,
            target_focus: controller.target_focus,
            fov: perspective_fov(projection),
        }
    }

    fn restore(
        &self,
        transform: &mut Transform,
        controller: &mut OrbitCameraController,
        projection: &mut Projection,
    ) {
        *transform = self.transform;
        controller.focus = self.focus;
        controller.distance = self.distance;
        controller.yaw = self.yaw;
        controller.pitch = self.pitch;
        controller.target_distance = self.target_distance;
        controller.target_focus = self.target_focus;
        if let (Some(fov), Projection::Perspective(perspective)) = (self.fov, projection) {
            perspective.fov = fov;
        }
    }
}

struct ActiveJob {
    request: VisionRequest,
    /// The caller's token scope, re-checked against the active verse at
    /// capture time (SEC-ADDENDUM).
    caller_scope: String,
    /// The user's navigation before a `focus.navigate` moved it; put back on
    /// failure, timeout, or `restore: true`.
    nav_snapshot: Option<NavSnapshot>,
    mode: CaptureMode,
    source: PoseSource,
    settle_frames: u32,
    restore: bool,
    /// Window mode with a pose: `CameraCaptureLock` is held for the job.
    holds_camera: bool,
    /// Scene-mode output size.
    size: Option<UVec2>,
    started: Duration,
    phase: JobPhase,
    pose: Option<PoseTarget>,
    snapshot: Option<CameraSnapshot>,
    capture_camera: Option<Entity>,
    image: Option<Handle<Image>>,
    screenshot: Option<Entity>,
    frames_waited: u32,
    settled: bool,
    tiles_pending: u32,
}

/// The main (orbit) camera.
type MainCamera<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut OrbitCameraController,
        &'static mut Projection,
    ),
>;

/// Hierarchy + scene reads for `focus` resolution and navigation.
#[derive(SystemParam)]
pub struct SceneLookup<'w, 's> {
    nav: Option<ResMut<'w, NavigationManager>>,
    verses: Option<Res<'w, VerseManager>>,
    visibility: Option<Res<'w, VisibilityState>>,
    markers: Query<
        'w,
        's,
        (
            Entity,
            &'static SpawnedNodeMarker,
            Option<&'static Visibility>,
        ),
    >,
    transforms: Query<'w, 's, &'static GlobalTransform>,
    aabbs: Query<'w, 's, &'static Aabb>,
    children: Query<'w, 's, &'static Children>,
}

/// Terrain reads: the live (grounded) projection, scale, heights, residency.
#[derive(SystemParam)]
pub struct TerrainLookup<'w> {
    tile_source: Option<Res<'w, ActiveTileSource>>,
    terrain: Option<Res<'w, ActivePetalTerrain>>,
    heights: Option<Res<'w, TerrainHeightField>>,
    camera_scale: Option<Res<'w, CameraScaleSettings>>,
    residency: Option<Res<'w, CaptureFocusResidency>>,
}

impl TerrainLookup<'_> {
    /// The RUNTIME projection (`origin_ele` grounded), never the persisted config.
    fn projection(&self) -> Option<&GeoProjection> {
        self.tile_source.as_deref()?.projection.as_ref()
    }

    fn world_scale(&self) -> f64 {
        let from_config = self
            .terrain
            .as_deref()
            .and_then(|terrain| terrain.config.as_ref())
            .filter(|config| config.enabled)
            .map(|config| config.effective_world_scale());
        let from_camera = || {
            self.camera_scale
                .as_deref()
                .map(|settings| f64::from(settings.world_scale))
        };
        sanitize_world_scale(from_config.or_else(from_camera).unwrap_or(1.0))
    }

    /// `[min_lat, min_lon, max_lat, max_lon]` of every loaded tileset.
    fn tileset_bounds(&self) -> Vec<[f64; 4]> {
        self.tile_source
            .as_deref()
            .and_then(|source| source.composite.as_ref())
            .map(|composite| composite.source_metas().iter().map(|m| m.bounds).collect())
            .unwrap_or_default()
    }

    fn zoom_range(&self) -> Option<[u8; 2]> {
        let metas_range = self
            .tile_source
            .as_deref()
            .and_then(|source| source.composite.as_ref())
            .and_then(|composite| {
                let metas = composite.source_metas();
                let min = metas.iter().map(|m| m.min_zoom).min()?;
                let max = metas.iter().map(|m| m.max_zoom).max()?;
                Some([min, max])
            });
        metas_range.or_else(|| {
            self.terrain
                .as_deref()
                .and_then(|terrain| terrain.config.as_ref())
                .map(|config| [config.min_zoom, config.max_zoom])
        })
    }

    /// Resident terrain height (world units) under world `(x, z)`.
    fn ground_world(&self, x: f32, z: f32) -> Option<f32> {
        self.heights.as_deref()?.height_at(x, z)
    }
}

/// Register the vision bridge resources and systems (the one wiring `main.rs`
/// and the tests share — src/AGENTS.md §vision "Schedule").
pub fn install(app: &mut App, receiver: VisionRequestReceiver) {
    app.insert_resource(VisionBridge(receiver))
        .init_resource::<VisionJob>()
        .add_systems(PostStartup, isolate_gizmos_from_capture_cameras)
        .add_systems(
            Update,
            drain_vision_requests.after(fe_terrain::terrain_plugin::fetch_and_spawn_terrain_chunks),
        )
        .add_systems(
            PostUpdate,
            (apply_vision_pose, advance_vision_job)
                .chain()
                .before(bevy::camera::CameraUpdateSystems)
                .before(TransformSystems::Propagate),
        );
}

/// PostStartup, once: every gizmo group draws on [`GIZMO_RENDER_LAYER`] and
/// the main camera sees layers 0 + that one, so scene captures (layer 0
/// only) carry no grid/axis/gimbal overlays.
pub fn isolate_gizmos_from_capture_cameras(
    store: Option<ResMut<GizmoConfigStore>>,
    main_cameras: Query<Entity, (With<OrbitCameraController>, Without<RenderLayers>)>,
    mut commands: Commands,
) {
    if let Some(mut store) = store {
        for (_, config, _) in store.iter_mut() {
            config.render_layers = RenderLayers::layer(GIZMO_RENDER_LAYER);
        }
    }
    for camera in &main_cameras {
        commands
            .entity(camera)
            .insert(RenderLayers::from_layers(&[0, GIZMO_RENDER_LAYER]));
    }
}

// ---------------------------------------------------------------------------
// Scope confinement (SEC-ADDENDUM)
// ---------------------------------------------------------------------------

/// `caller_scope` covers verse `verse_id` — fe-policy's containment rule
/// (the one `TokenScopePolicy` uses), so a verse-B token never sees verse A.
pub(crate) fn scope_covers_verse(caller_scope: &str, verse_id: &str) -> bool {
    !verse_id.is_empty() && fe_database::scope_contains(caller_scope, &format!("VERSE#{verse_id}"))
}

/// The caller covers the ACTIVE verse (nothing open = denied).
fn scope_covers_active_verse(caller_scope: &str, nav: Option<&NavigationManager>) -> bool {
    nav.and_then(|nav| nav.active_verse_id.as_deref())
        .is_some_and(|verse_id| scope_covers_verse(caller_scope, verse_id))
}

/// The caller covers the active verse AND every petal still rendered: a
/// rendered petal that is not the active one must sit in a covered verse
/// (unknown = denied) — src/AGENTS.md §vision "Scope".
pub(crate) fn scope_covers_view<'a>(
    caller_scope: &str,
    nav: Option<&NavigationManager>,
    verses: Option<&VerseManager>,
    rendered_petals: impl IntoIterator<Item = &'a str>,
) -> bool {
    if !scope_covers_active_verse(caller_scope, nav) {
        return false;
    }
    let active_petal = nav.and_then(|nav| nav.active_petal_id.as_deref());
    rendered_petals.into_iter().all(|petal| {
        Some(petal) == active_petal
            || verses
                .and_then(|verses| locate_petal(verses, petal))
                .is_some_and(|verse| scope_covers_verse(caller_scope, verse))
    })
}

/// What a frame can show besides the cursor: the terrain binding (re-bound
/// only when the next petal's terrain loads, so it outlives a verse switch)
/// and every spawned node (despawned a frame late). THE one scope gate both
/// capture paths and the camera read pass, at arrival and at frame time.
#[derive(SystemParam)]
pub struct ViewScope<'w, 's> {
    verses: Option<Res<'w, VerseManager>>,
    terrain: Option<Res<'w, ActivePetalTerrain>>,
    markers: Query<'w, 's, &'static SpawnedNodeMarker>,
}

impl ViewScope<'_, '_> {
    /// SEC-ADDENDUM: may `caller_scope` see the view `nav` has open right now?
    pub(crate) fn covers(&self, caller_scope: &str, nav: Option<&NavigationManager>) -> bool {
        // The QA local operator sees exactly what this window shows (src/AGENTS.md §qa-operator).
        if fe_runtime::qa_debug::qa_operator_covers(caller_scope) {
            return true;
        }
        let terrain_petal = self
            .terrain
            .as_deref()
            .filter(|terrain| terrain.config.is_some())
            .and_then(|terrain| terrain.petal_id.as_deref());
        let node_petals = self.markers.iter().map(|marker| marker.petal_id.as_str());
        scope_covers_view(
            caller_scope,
            nav,
            self.verses.as_deref(),
            terrain_petal.into_iter().chain(node_petals),
        )
    }
}

/// The user's verse/fractal/petal cursor before a capture navigated it.
struct NavSnapshot {
    verse: Option<(String, String)>,
    fractal: Option<(String, String)>,
    petal: Option<String>,
    /// The petal the job navigated to: the snapshot is only put back while
    /// the cursor is still there, so a user who moved on is never yanked back.
    navigated_to: String,
}

impl NavSnapshot {
    fn take(nav: &NavigationManager, navigated_to: String) -> Self {
        Self {
            navigated_to,
            verse: nav
                .active_verse_id
                .clone()
                .map(|id| (id, nav.active_verse_name.clone())),
            fractal: nav
                .active_fractal_id
                .clone()
                .map(|id| (id, nav.active_fractal_name.clone())),
            petal: nav.active_petal_id.clone(),
        }
    }

    /// Put the cursor back through the manager's own methods (root AGENTS.md
    /// anti-pattern 1: never write `NavigationManager` fields directly).
    fn restore(&self, nav: &mut NavigationManager) {
        match &self.verse {
            Some((id, name)) => nav.navigate_to_verse(id.clone(), name.clone()),
            None => nav.back_from_verse(),
        }
        if let Some((id, name)) = &self.fractal {
            nav.navigate_to_fractal(id.clone(), name.clone());
        }
        if let Some(petal) = &self.petal {
            nav.navigate_to_petal(petal.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

fn failure(
    status_class: VisionFailureClass,
    code: &'static str,
    message: impl Into<String>,
) -> VisionFailure {
    VisionFailure {
        status_class,
        code,
        message: message.into(),
    }
}

fn unresolvable(code: &'static str, message: impl Into<String>) -> VisionFailure {
    failure(VisionFailureClass::Unresolvable, code, message)
}

fn failed(code: &'static str, message: impl Into<String>) -> VisionFailure {
    failure(VisionFailureClass::Failed, code, message)
}

fn no_main_camera() -> VisionFailure {
    failed("no_main_camera", "the main camera does not exist (yet)")
}

/// Window-mode refusal (`no_window` / `window_minimized`), sharing
/// `capture_refusal`'s 0x0 rule with the plain GET path.
fn window_refusal(window: Option<&Window>) -> Option<VisionFailure> {
    let reason = capture_refusal(window)?;
    let code = if window.is_none() {
        "no_window"
    } else {
        "window_minimized"
    };
    Some(failure(VisionFailureClass::NoWindow, code, reason))
}

// ---------------------------------------------------------------------------
// Pose resolution
// ---------------------------------------------------------------------------

/// What resolving a pose spec produced.
enum Resolution {
    Ready(PoseTarget),
    /// `focus.navigate`: the target lives in another petal; navigate first.
    Navigate(FocusTarget),
}

/// The main camera's view: framing keeps its direction and FOV.
struct ViewContext {
    transform: Transform,
    focus: Vec3,
    fov: f32,
    aspect: f32,
}

impl ViewContext {
    fn current_pose(&self) -> PoseTarget {
        let position = self.transform.translation;
        let target = if (self.focus - position).length() > 1e-6 {
            self.focus
        } else {
            position + Vec3::from(self.transform.forward())
        };
        PoseTarget {
            position,
            target,
            up: Vec3::from(self.transform.up()),
            fov: self.fov,
        }
    }
}

fn perspective_fov(projection: &Projection) -> Option<f32> {
    match projection {
        Projection::Perspective(perspective) => Some(perspective.fov),
        _ => None,
    }
}

fn default_fov() -> f32 {
    (DEFAULT_FOV_DEG as f32).to_radians()
}

fn view_context(
    cameras: &MainCamera,
    window: Option<&Window>,
    size: Option<UVec2>,
) -> Result<ViewContext, VisionFailure> {
    let (transform, controller, projection) = cameras.single().map_err(|_| no_main_camera())?;
    let aspect = match (size, window) {
        (Some(size), _) => size.x as f32 / size.y as f32,
        (None, Some(window)) if window.physical_width() > 0 && window.physical_height() > 0 => {
            window.physical_width() as f32 / window.physical_height() as f32
        }
        _ => DEFAULT_WIDTH as f32 / DEFAULT_HEIGHT as f32,
    };
    Ok(ViewContext {
        transform: *transform,
        focus: controller.focus,
        fov: perspective_fov(projection).unwrap_or_else(default_fov),
        aspect,
    })
}

/// `[f64; 3]` → `Vec3`, `None` if the `f32` cast overflows (a validated-finite
/// `f64` such as `1e300` is not a finite `f32`).
fn finite_vec3(v: [f64; 3]) -> Option<Vec3> {
    let out = Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
    out.is_finite().then_some(out)
}

fn pose_from_framed(framed: FramedPose, fov: f32) -> Result<PoseTarget, VisionFailure> {
    let (Some(position), Some(target), Some(up)) = (
        finite_vec3(framed.position),
        finite_vec3(framed.target),
        finite_vec3(framed.up),
    ) else {
        return Err(unresolvable(
            "geo_out_of_projection_range",
            "the framed pose does not fit in world space",
        ));
    };
    Ok(PoseTarget {
        position,
        target,
        up,
        fov,
    })
}

/// Frame a world box with the view's direction and FOV (fe-renderer's pure
/// `frame_world_box`, spec §5.3).
fn frame_box(min: Vec3, max: Vec3, view: &ViewContext) -> PoseTarget {
    let view_dir = Vec3::from(view.transform.forward());
    PoseTarget::from_look_at(frame_world_box(min, max, view_dir, view.fov), view.fov)
}

fn source_of(pose: Option<&ValidPoseSpec>) -> PoseSource {
    match pose {
        None | Some(ValidPoseSpec::Orbit { .. }) => PoseSource::Orbit,
        Some(ValidPoseSpec::Camera { .. }) => PoseSource::Camera,
        Some(ValidPoseSpec::Geo { .. }) => PoseSource::Geo,
        Some(ValidPoseSpec::Tile { .. }) => PoseSource::Tile,
        Some(ValidPoseSpec::Focus { .. }) => PoseSource::Focus,
    }
}

fn no_projection() -> VisionFailure {
    unresolvable(
        "no_projection",
        "the active petal has no terrain projection (no map is bound)",
    )
}

fn resolve_pose(
    pose: Option<&ValidPoseSpec>,
    caller_scope: &str,
    view: &ViewContext,
    scene: &SceneLookup,
    terrain: &TerrainLookup,
) -> Result<Resolution, VisionFailure> {
    let pose = match pose {
        None => view.current_pose(),
        Some(ValidPoseSpec::Camera {
            position,
            target,
            up,
            fov_deg,
        }) => {
            let (Some(position), Some(target), Some(up)) = (
                finite_vec3(*position),
                finite_vec3(*target),
                finite_vec3(*up),
            ) else {
                return Err(unresolvable(
                    "degenerate_camera",
                    "camera coordinates exceed the world's float range",
                ));
            };
            PoseTarget {
                position,
                target,
                up,
                fov: (*fov_deg as f32).to_radians(),
            }
        }
        Some(ValidPoseSpec::Orbit {
            focus,
            yaw_deg,
            pitch_deg,
            distance,
        }) => {
            let Some(focus) = finite_vec3(*focus) else {
                return Err(unresolvable(
                    "orbit_out_of_range",
                    "orbit focus exceeds the world's float range",
                ));
            };
            let transform = compute_orbit_transform(
                focus,
                *distance as f32,
                (*yaw_deg as f32).to_radians(),
                (*pitch_deg as f32).to_radians(),
            );
            PoseTarget {
                position: transform.translation,
                target: focus,
                up: Vec3::Y,
                fov: view.fov,
            }
        }
        Some(ValidPoseSpec::Geo {
            lat,
            lon,
            altitude_m,
            heading_deg,
            pitch_deg,
        }) => resolve_geo(
            terrain,
            [*lat, *lon, *altitude_m, *heading_deg, *pitch_deg],
            view.fov,
        )?,
        Some(ValidPoseSpec::Tile { z, x, y }) => resolve_tile(terrain, (*z, *x, *y), view)?,
        Some(ValidPoseSpec::Focus { target, navigate }) => {
            return resolve_focus(target, *navigate, caller_scope, view, scene, terrain);
        }
    };
    Ok(Resolution::Ready(pose))
}

/// `geo`: position = `wgs84_to_local(lat, lon, ground + altitude) × scale`,
/// ground from the resident height field (else the grounded origin).
fn resolve_geo(
    terrain: &TerrainLookup,
    [lat, lon, altitude_m, heading_deg, pitch_deg]: [f64; 5],
    fov: f32,
) -> Result<PoseTarget, VisionFailure> {
    let projection = terrain.projection().ok_or_else(no_projection)?;
    if !within_projection_range(projection, lat, lon) {
        return Err(unresolvable(
            "geo_out_of_projection_range",
            format!("({lat}, {lon}) is more than 500 km from the petal's projection origin"),
        ));
    }
    let scale = terrain.world_scale();
    let ground_abs = projection
        .wgs84_to_local(lat, lon, projection.origin_ele)
        .ok()
        .and_then(|local| {
            terrain.ground_world((local[0] * scale) as f32, (local[2] * scale) as f32)
        })
        .map(|ground| f64::from(ground) / scale + projection.origin_ele)
        .unwrap_or(projection.origin_ele);
    let framed = geo_pose(
        projection,
        scale,
        lat,
        lon,
        ground_abs,
        altitude_m,
        heading_deg,
        pitch_deg,
    )
    .ok_or_else(|| unresolvable("geo_out_of_projection_range", "geo pose did not project"))?;
    pose_from_framed(framed, fov)
}

/// `tile`: frame the slippy tile top-down, north up, fitting both axes.
fn resolve_tile(
    terrain: &TerrainLookup,
    (z, x, y): (u8, u32, u32),
    view: &ViewContext,
) -> Result<PoseTarget, VisionFailure> {
    let projection = terrain.projection().ok_or_else(no_projection)?;
    let bounds = tile_bounds(z, x, y);
    if coverage_check(bounds, &terrain.tileset_bounds()) == Coverage::Outside {
        return Err(unresolvable(
            "tile_outside_tileset",
            format!("tile {z}/{x}/{y} lies outside every tileset bound to the active petal"),
        ));
    }
    frame_geo_bounds(projection, terrain, bounds, view)
}

fn frame_geo_bounds(
    projection: &GeoProjection,
    terrain: &TerrainLookup,
    bounds: [f64; 4],
    view: &ViewContext,
) -> Result<PoseTarget, VisionFailure> {
    let center_lat = (bounds[0] + bounds[2]) / 2.0;
    let center_lon = (bounds[1] + bounds[3]) / 2.0;
    if !within_projection_range(projection, center_lat, center_lon) {
        return Err(unresolvable(
            "geo_out_of_projection_range",
            "the area's centre is more than 500 km from the petal's projection origin",
        ));
    }
    let scale = terrain.world_scale();
    let ground_y = projection
        .wgs84_to_local(center_lat, center_lon, projection.origin_ele)
        .ok()
        .and_then(|local| {
            terrain.ground_world((local[0] * scale) as f32, (local[2] * scale) as f32)
        })
        .map(f64::from)
        .unwrap_or(0.0);
    let framed = frame_bounds_top_down(
        projection,
        scale,
        bounds,
        ground_y,
        f64::from(view.fov),
        f64::from(view.aspect),
    )
    .ok_or_else(|| unresolvable("geo_out_of_projection_range", "the area did not project"))?;
    pose_from_framed(framed, view.fov)
}

fn active_ancestry(nav: Option<&NavigationManager>) -> NodeAncestry<'_> {
    NodeAncestry {
        verse_id: nav.and_then(|n| n.active_verse_id.as_deref()).unwrap_or(""),
        fractal_id: nav
            .and_then(|n| n.active_fractal_id.as_deref())
            .unwrap_or(""),
        petal_id: nav.and_then(|n| n.active_petal_id.as_deref()).unwrap_or(""),
    }
}

/// D-14 (the shared picker rule): only effectively visible, rendered nodes
/// may be framed.
fn node_visible(scene: &SceneLookup, node_id: &str, own: Option<&Visibility>) -> bool {
    if matches!(own, Some(Visibility::Hidden)) {
        return false;
    }
    match scene.visibility.as_deref() {
        Some(state) => effective_visibility(node_id, active_ancestry(scene.nav.as_deref()), state),
        None => true,
    }
}

fn node_box(scene: &SceneLookup, entity: Entity) -> Option<(Vec3, Vec3)> {
    fe_ui::gimbal::subtree_world_aabb(entity, &scene.transforms, &scene.aabbs, &scene.children)
        .or_else(|| {
            let point = scene.transforms.get(entity).ok()?.translation();
            Some((point, point))
        })
}

/// `(verse_id, petal_id)` of a node in the hierarchy tree.
fn locate_node<'a>(verses: &'a VerseManager, node_id: &str) -> Option<(&'a str, &'a str)> {
    verses.verses.iter().find_map(|verse| {
        verse
            .fractals
            .iter()
            .flat_map(|f| &f.petals)
            .find(|p| p.nodes.iter().any(|n| n.id == node_id))
            .map(|p| (verse.id.as_str(), p.id.as_str()))
    })
}

/// `verse_id` of a petal in the hierarchy tree.
fn locate_petal<'a>(verses: &'a VerseManager, petal_id: &str) -> Option<&'a str> {
    verses.verses.iter().find_map(|verse| {
        verse
            .fractals
            .iter()
            .flat_map(|f| &f.petals)
            .any(|p| p.id == petal_id)
            .then_some(verse.id.as_str())
    })
}

/// SEC-ADDENDUM item 2: a focus target's verse + petal, resolved BEFORE any
/// capture; `None` when it is unknown OR out of the caller's scope — the two
/// are deliberately indistinguishable (the uniform [`denied`]).
fn authorized_target<'a>(
    target: &FocusTarget,
    caller_scope: &str,
    scene: &'a SceneLookup,
) -> Option<(&'a str, &'a str)> {
    let nav = scene.nav.as_deref()?;
    let active = nav
        .active_verse_id
        .as_deref()
        .zip(nav.active_petal_id.as_deref());
    let verses = scene.verses.as_deref();
    let located = match target {
        FocusTarget::Node(node_id) => verses.and_then(|v| locate_node(v, node_id)).or_else(|| {
            // A spawned marker the tree lacks lives in the active petal.
            let (verse, petal) = active?;
            scene
                .markers
                .iter()
                .any(|(_, m, _)| m.node_id == *node_id && m.petal_id == petal)
                .then_some((verse, petal))
        }),
        FocusTarget::Petal(petal_id) => verses
            .and_then(|v| locate_petal(v, petal_id))
            .zip(
                verses
                    .and_then(|v| v.find_petal(petal_id))
                    .map(|p| p.id.as_str()),
            )
            .or_else(|| active.filter(|(_, petal)| petal == petal_id)),
    };
    located.filter(|(verse, _)| scope_covers_verse(caller_scope, verse))
}

fn resolve_focus(
    target: &FocusTarget,
    navigate: bool,
    caller_scope: &str,
    view: &ViewContext,
    scene: &SceneLookup,
    terrain: &TerrainLookup,
) -> Result<Resolution, VisionFailure> {
    let (target_verse, target_petal) =
        authorized_target(target, caller_scope, scene).ok_or_else(denied)?;
    let nav = scene.nav.as_deref();
    let active_verse = nav.and_then(|nav| nav.active_verse_id.as_deref());
    let active_petal = nav.and_then(|nav| nav.active_petal_id.as_deref());
    if Some(target_verse) != active_verse {
        // Never switch the user's view to another verse (SEC-ADDENDUM 2/5).
        return Err(if navigate {
            unresolvable(
                "cross_verse_navigate_refused",
                "`navigate` never leaves the active verse — the target is in another verse",
            )
        } else {
            unresolvable("not_active_petal", "the target is not in the active verse")
        });
    }
    if Some(target_petal) != active_petal {
        return if navigate {
            Ok(Resolution::Navigate(target.clone()))
        } else {
            Err(unresolvable(
                "not_active_petal",
                format!(
                    "the target is in petal {target_petal}, not the active petal — pass `navigate: true` (with include_ui) to go there"
                ),
            ))
        };
    }
    match target {
        FocusTarget::Node(node_id) => {
            let spawned = scene.markers.iter().find(|(_, marker, _)| {
                marker.node_id == *node_id && marker.petal_id == target_petal
            });
            let Some((entity, _, own_visibility)) = spawned else {
                return Err(unresolvable(
                    "node_not_found",
                    format!("node {node_id} is not spawned in the active petal (yet)"),
                ));
            };
            if !node_visible(scene, node_id, own_visibility) {
                return Err(unresolvable(
                    "node_hidden",
                    format!("node {node_id} is hidden — show it before framing it"),
                ));
            }
            let (min, max) = node_box(scene, entity)
                .ok_or_else(|| unresolvable("node_not_found", "node has no transform"))?;
            Ok(Resolution::Ready(frame_box(min, max, view)))
        }
        FocusTarget::Petal(petal_id) => {
            let mut bounds: Option<(Vec3, Vec3)> = None;
            for (entity, marker, own_visibility) in &scene.markers {
                if marker.petal_id != *petal_id
                    || !node_visible(scene, &marker.node_id, own_visibility)
                {
                    continue;
                }
                if let Some((lo, hi)) = node_box(scene, entity) {
                    bounds = Some(match bounds {
                        Some((min, max)) => (min.min(lo), max.max(hi)),
                        None => (lo, hi),
                    });
                }
            }
            if let Some((min, max)) = bounds {
                return Ok(Resolution::Ready(frame_box(min, max, view)));
            }
            let tilesets = terrain.tileset_bounds();
            match (terrain.projection(), union_bounds(&tilesets)) {
                (Some(projection), Some(area)) => Ok(Resolution::Ready(frame_geo_bounds(
                    projection, terrain, area, view,
                )?)),
                _ => Err(unresolvable(
                    "nothing_to_frame",
                    format!("petal {petal_id} has no visible nodes and no tileset to frame"),
                )),
            }
        }
    }
}

/// Navigate to `petal_id` within the ACTIVE verse (only the levels that
/// differ); `false` if unknown or in another verse.
fn navigate_to_petal(nav: &mut NavigationManager, verses: &VerseManager, petal_id: &str) -> bool {
    for verse in &verses.verses {
        for fractal in &verse.fractals {
            if !fractal.petals.iter().any(|p| p.id == petal_id) {
                continue;
            }
            if nav.active_verse_id.as_deref() != Some(verse.id.as_str()) {
                // Cross-verse navigation is refused outright (SEC-ADDENDUM).
                return false;
            }
            if nav.active_fractal_id.as_deref() != Some(fractal.id.as_str()) {
                nav.navigate_to_fractal(fractal.id.clone(), fractal.name.clone());
            }
            nav.navigate_to_petal(petal_id.to_string());
            return true;
        }
    }
    false
}

fn target_spawned(target: &FocusTarget, scene: &SceneLookup) -> bool {
    scene.markers.iter().any(|(_, marker, _)| match target {
        FocusTarget::Node(node_id) => marker.node_id == *node_id,
        FocusTarget::Petal(petal_id) => marker.petal_id == *petal_id,
    })
}

// ---------------------------------------------------------------------------
// Read-back
// ---------------------------------------------------------------------------

fn v3(v: Vec3) -> [f64; 3] {
    [f64::from(v.x), f64::from(v.y), f64::from(v.z)]
}

fn window_state(window: Option<&Window>) -> WindowState {
    match window {
        Some(window) => WindowState {
            width: window.physical_width(),
            height: window.physical_height(),
            minimized: window.physical_width() == 0 || window.physical_height() == 0,
        },
        None => WindowState {
            width: 0,
            height: 0,
            minimized: true,
        },
    }
}

/// Geographic read-back of a world position (altitude above resident
/// terrain when there is any, else above the grounded origin).
fn geo_of(position: Vec3, terrain: &TerrainLookup) -> Option<ResolvedGeo> {
    let projection = terrain.projection()?;
    let scale = terrain.world_scale();
    let local = v3(position).map(|c| c / scale);
    let (lat, lon, _ele) = projection.local_to_wgs84(local[0], local[1], local[2]);
    let (altitude_m, altitude_reference) = match terrain.ground_world(position.x, position.z) {
        Some(ground) => (
            (f64::from(position.y) - f64::from(ground)) / scale,
            AltitudeReference::Terrain,
        ),
        None => (local[1], AltitudeReference::Origin),
    };
    [lat, lon, altitude_m]
        .iter()
        .all(|v| v.is_finite())
        .then_some(ResolvedGeo {
            lat,
            lon,
            altitude_m,
            altitude_reference,
        })
}

fn orbit_of(position: Vec3, target: Vec3) -> Option<ResolvedOrbit> {
    let (focus, distance, yaw, pitch) = orbit_from_look_at(position, target)?;
    Some(ResolvedOrbit {
        focus: v3(focus),
        yaw_deg: f64::from(yaw.to_degrees()),
        pitch_deg: f64::from(pitch.to_degrees()),
        distance: f64::from(distance),
    })
}

/// `GET /api/v1/debug/camera`: answered the frame it arrives, in any job state.
fn read_camera(
    locked: bool,
    window: Option<&Window>,
    cameras: &MainCamera,
    scene: &SceneLookup,
    terrain: &TerrainLookup,
) -> Result<VisionOutput, VisionFailure> {
    let (transform, controller, projection) = cameras.single().map_err(|_| no_main_camera())?;
    let window = window_state(window);
    let world_scale = terrain.world_scale();
    let fov = perspective_fov(projection).unwrap_or_else(default_fov);
    let pose = ResolvedPose {
        mode: CaptureMode::Window,
        position: v3(transform.translation),
        target: v3(controller.focus),
        up: v3(Vec3::from(transform.up())),
        fov_deg: f64::from(fov.to_degrees()),
        orbit: Some(ResolvedOrbit {
            focus: v3(controller.focus),
            yaw_deg: f64::from(controller.yaw.to_degrees()),
            pitch_deg: f64::from(controller.pitch.to_degrees()),
            distance: f64::from(controller.distance),
        }),
        geo: geo_of(transform.translation, terrain),
        world_scale,
        width: window.width,
        height: window.height,
        settle_frames_used: 0,
        settled: true,
        tiles_pending: 0,
        restored: false,
        source: PoseSource::Orbit,
    };
    let nav = scene.nav.as_deref();
    let active = ActiveHierarchy {
        verse_id: nav.and_then(|n| n.active_verse_id.clone()),
        verse_name: nav.map(|n| n.active_verse_name.clone()).unwrap_or_default(),
        fractal_id: nav.and_then(|n| n.active_fractal_id.clone()),
        fractal_name: nav
            .map(|n| n.active_fractal_name.clone())
            .unwrap_or_default(),
        petal_id: nav.and_then(|n| n.active_petal_id.clone()),
    };
    let projection = terrain.projection().map(|p| ProjectionState {
        origin_lat: p.origin_lat,
        origin_lon: p.origin_lon,
        origin_ele_grounded: p.origin_ele,
        world_scale,
        tileset_bounds: union_bounds(&terrain.tileset_bounds()),
        zoom_range: terrain.zoom_range(),
    });
    Ok(VisionOutput::Camera(CameraState {
        pose,
        locked,
        active,
        projection,
        window,
    }))
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Update: answer `ReadCamera` at once; start (or refuse) a `Capture`. The
/// idle path is one empty `try_recv`.
#[allow(clippy::too_many_arguments)]
pub fn drain_vision_requests(
    bridge: Res<VisionBridge>,
    mut job: ResMut<VisionJob>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: MainCamera,
    mut scene: SceneLookup,
    terrain: TerrainLookup,
    view: ViewScope,
    mut commands: Commands,
) {
    while let Ok(request) = bridge.0.try_recv() {
        // The caller already timed out or hung up: never move the camera for it.
        if request.reply_tx.is_closed() {
            continue;
        }
        // SEC-ADDENDUM: every read and capture needs the whole view covered.
        if !view.covers(&request.caller_scope, scene.nav.as_deref()) {
            request.reply(Err(denied()));
            continue;
        }
        let spec = match &request.op {
            VisionOp::ReadCamera => {
                let result = read_camera(
                    job.holds_camera(),
                    windows.single().ok(),
                    &cameras,
                    &scene,
                    &terrain,
                );
                request.reply(result);
                continue;
            }
            VisionOp::Capture(spec) => spec.clone(),
        };
        if job.0.is_some() {
            request.reply(Err(failure(
                VisionFailureClass::Busy,
                "capture_in_progress",
                "a posed capture already holds the camera",
            )));
            continue;
        }
        let caller_scope = request.caller_scope.clone();
        match start_job(
            &spec,
            &caller_scope,
            windows.single().ok(),
            &cameras,
            &mut scene,
            &terrain,
        ) {
            Ok((phase, pose, nav_snapshot)) => {
                let holds_camera = spec.mode() == CaptureMode::Window && spec.pose().is_some();
                let source = source_of(spec.pose());
                tracing::info!(
                    correlation_id = %request.correlation_id,
                    mode = ?spec.mode(),
                    source = ?source,
                    "agent vision capture started"
                );
                if holds_camera {
                    commands.insert_resource(CameraCaptureLock);
                }
                job.0 = Some(ActiveJob {
                    request,
                    caller_scope,
                    nav_snapshot,
                    mode: spec.mode(),
                    source,
                    settle_frames: spec.settle_frames(),
                    restore: spec.restore().unwrap_or(false),
                    holds_camera,
                    size: spec
                        .width()
                        .zip(spec.height())
                        .map(|(w, h)| UVec2::new(w, h)),
                    started: time.elapsed(),
                    phase,
                    pose,
                    snapshot: None,
                    capture_camera: None,
                    image: None,
                    screenshot: None,
                    frames_waited: 0,
                    settled: false,
                    tiles_pending: 0,
                });
            }
            Err(refusal) => request.reply(Err(refusal)),
        }
    }
}

/// Resolve a capture's pose (or start its navigation) — every dynamic
/// refusal is decided here, before anything moves.
fn start_job(
    spec: &ValidCaptureSpec,
    caller_scope: &str,
    window: Option<&Window>,
    cameras: &MainCamera,
    scene: &mut SceneLookup,
    terrain: &TerrainLookup,
) -> Result<(JobPhase, Option<PoseTarget>, Option<NavSnapshot>), VisionFailure> {
    if spec.mode() == CaptureMode::Window {
        if let Some(refusal) = window_refusal(window) {
            return Err(refusal);
        }
    }
    let size = spec
        .width()
        .zip(spec.height())
        .map(|(w, h)| UVec2::new(w, h));
    let view = view_context(cameras, window, size)?;
    match resolve_pose(spec.pose(), caller_scope, &view, scene, terrain)? {
        Resolution::Ready(pose) => {
            pose.transform().ok_or_else(|| {
                unresolvable(
                    "degenerate_camera",
                    "the resolved pose has no valid orientation",
                )
            })?;
            Ok((JobPhase::Posing, Some(pose), None))
        }
        Resolution::Navigate(target) => {
            // `resolve_focus` proved the target is covered and in the ACTIVE
            // verse, so this only ever moves fractal/petal.
            let petal = authorized_target(&target, caller_scope, scene)
                .map(|(_, petal)| petal.to_string())
                .ok_or_else(denied)?;
            let (Some(nav), Some(verses)) = (scene.nav.as_deref_mut(), scene.verses.as_deref())
            else {
                return Err(denied());
            };
            let snapshot = NavSnapshot::take(nav, petal.clone());
            if !navigate_to_petal(nav, verses, &petal) {
                return Err(denied());
            }
            Ok((
                JobPhase::Navigating {
                    target,
                    frames: 0,
                    seen_frames: 0,
                },
                None,
                Some(snapshot),
            ))
        }
    }
}

/// What one PostUpdate step decided for the active job.
enum Step {
    Continue,
    Finish(Result<Vec<u8>, VisionFailure>),
}

/// PostUpdate (before transform propagation, so the pose renders this
/// frame): finish a navigation wait, then apply the pose — the main camera
/// for window mode, a fresh render-to-texture camera for scene mode.
#[allow(clippy::too_many_arguments)]
pub fn apply_vision_pose(
    mut job: ResMut<VisionJob>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: MainCamera,
    mut scene: SceneLookup,
    terrain: TerrainLookup,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    let Some(active) = job.0.as_mut() else {
        return;
    };
    let window = windows.single().ok();
    let step = 'step: {
        if let JobPhase::Navigating {
            target,
            frames,
            seen_frames,
        } = &mut active.phase
        {
            *frames += 1;
            if target_spawned(target, &scene) {
                *seen_frames += 1;
            }
            if *seen_frames < NAVIGATE_GRACE_FRAMES && *frames < NAVIGATE_FRAME_CAP {
                break 'step Step::Continue;
            }
            let target = target.clone();
            let resolved = view_context(&cameras, window, active.size).and_then(|view| {
                resolve_focus(
                    &target,
                    false,
                    &active.caller_scope,
                    &view,
                    &scene,
                    &terrain,
                )
            });
            match resolved {
                Ok(Resolution::Ready(pose)) => {
                    active.pose = Some(pose);
                    active.phase = JobPhase::Posing;
                }
                Ok(Resolution::Navigate(_)) => {
                    break 'step Step::Finish(Err(unresolvable(
                        "not_active_petal",
                        "navigation did not reach the target petal",
                    )));
                }
                Err(refusal) => break 'step Step::Finish(Err(refusal)),
            }
        }
        if !matches!(active.phase, JobPhase::Posing) {
            break 'step Step::Continue;
        }
        match pose_job(active, &mut cameras, &mut images, &mut commands) {
            Ok(()) => Step::Continue,
            Err(refusal) => Step::Finish(Err(refusal)),
        }
    };
    if let Step::Finish(outcome) = step {
        if let Some(active) = job.0.take() {
            let nav = scene.nav.as_deref_mut();
            finish_job(
                active,
                outcome,
                window,
                &mut cameras,
                nav,
                &terrain,
                &mut images,
                &mut commands,
            );
        }
    }
}

/// Apply the resolved pose: snapshot + write the main camera (window mode),
/// or spawn the capture camera into a fresh render target (scene mode).
fn pose_job(
    active: &mut ActiveJob,
    cameras: &mut MainCamera,
    images: &mut Assets<Image>,
    commands: &mut Commands,
) -> Result<(), VisionFailure> {
    let pose = active
        .pose
        .ok_or_else(|| failed("unresolved_pose", "no pose to apply"))?;
    let posed = pose.transform().ok_or_else(|| {
        unresolvable(
            "degenerate_camera",
            "the resolved pose has no valid orientation",
        )
    })?;
    match active.mode {
        CaptureMode::Window => {
            if active.holds_camera {
                let (mut transform, controller, mut projection) =
                    cameras.single_mut().map_err(|_| no_main_camera())?;
                if active.snapshot.is_none() {
                    active.snapshot =
                        Some(CameraSnapshot::take(&transform, &controller, &projection));
                }
                *transform = posed;
                if let Projection::Perspective(perspective) = projection.as_mut() {
                    perspective.fov = pose.fov;
                }
            }
        }
        CaptureMode::Scene => {
            let size = active
                .size
                .unwrap_or(UVec2::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));
            let mut image =
                Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
            image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
            let handle = images.add(image);
            let main_perspective = cameras
                .single()
                .ok()
                .and_then(|(_, _, projection)| match projection {
                    Projection::Perspective(perspective) => Some(perspective.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let projection = Projection::Perspective(PerspectiveProjection {
                fov: pose.fov,
                aspect_ratio: size.x as f32 / size.y as f32,
                ..main_perspective
            });
            let camera = commands
                .spawn((
                    Camera3d::default(),
                    Camera {
                        order: -1,
                        ..default()
                    },
                    RenderTarget::Image(ImageRenderTarget::from(handle.clone())),
                    projection,
                    posed,
                    CaptureCamera,
                    StreamingFocus,
                    Name::new("agent vision capture camera"),
                ))
                .id();
            active.capture_camera = Some(camera);
            active.image = Some(handle);
        }
    }
    active.phase = JobPhase::Settling;
    active.frames_waited = 0;
    Ok(())
}

impl ActiveJob {
    /// Scene mode: is the capture focus's terrain ring resident (and how many
    /// tiles are still pending)? Window mode has no capture focus.
    fn residency(&self, terrain: &TerrainLookup) -> (bool, u32) {
        if self.mode == CaptureMode::Window {
            return (true, 0);
        }
        match (terrain.residency.as_deref(), self.capture_camera) {
            (None, _) => (true, 0),
            (Some(published), Some(camera)) if published.capture_focus == Some(camera) => (
                published.pending == 0,
                u32::try_from(published.pending).unwrap_or(u32::MAX),
            ),
            // The streamer has not sampled this camera yet.
            (Some(_), _) => (false, 0),
        }
    }
}

/// PostUpdate: settle (frames, then terrain residency for scene mode),
/// queue the capture, then finish the job when its PNG arrives — or tear it
/// down at [`JOB_WALL_CLOCK_CAP`].
#[allow(clippy::too_many_arguments)]
pub fn advance_vision_job(
    mut job: ResMut<VisionJob>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: MainCamera,
    mut nav: Option<ResMut<NavigationManager>>,
    terrain: TerrainLookup,
    view: ViewScope,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    let Some(active) = job.0.as_mut() else {
        return;
    };
    let window = windows.single().ok();
    let elapsed = time.elapsed().saturating_sub(active.started);
    let step = 'step: {
        if active.request.reply_tx.is_closed() {
            // The caller hung up (client gone, API timeout): stop holding the
            // user's camera for a reply nobody will read.
            break 'step Step::Finish(Err(failed(
                "caller_gone",
                "the caller stopped waiting; the capture was abandoned",
            )));
        }
        if elapsed >= JOB_WALL_CLOCK_CAP {
            break 'step Step::Finish(Err(failed(
                "capture_timed_out",
                format!(
                    "the capture did not complete within {}s; the camera was restored",
                    JOB_WALL_CLOCK_CAP.as_secs()
                ),
            )));
        }
        match &active.phase {
            JobPhase::Navigating { .. } | JobPhase::Posing => Step::Continue,
            JobPhase::Settling => {
                let scene_mode = active.mode == CaptureMode::Scene;
                let min_frames = if scene_mode {
                    active.settle_frames.max(SCENE_MIN_FRAMES)
                } else {
                    active.settle_frames
                };
                let (resident, pending) = active.residency(&terrain);
                let ready = active.frames_waited >= min_frames && resident;
                let capped = active.frames_waited >= SETTLE_MAX.max(min_frames)
                    || elapsed >= SETTLE_WALL_CLOCK_CAP;
                let target_prepared = !scene_mode || active.frames_waited >= SCENE_MIN_FRAMES;
                if !((ready || capped) && target_prepared) {
                    active.frames_waited += 1;
                    break 'step Step::Continue;
                }
                // Re-check at capture time: the user may have navigated to a
                // verse this caller does not cover while the job settled.
                if !view.covers(&active.caller_scope, nav.as_deref()) {
                    break 'step Step::Finish(Err(denied()));
                }
                active.settled = ready;
                active.tiles_pending = pending;
                let screenshot = match (active.mode, active.image.as_ref()) {
                    (CaptureMode::Window, _) => {
                        if let Some(refusal) = window_refusal(window) {
                            break 'step Step::Finish(Err(refusal));
                        }
                        Screenshot::primary_window()
                    }
                    (CaptureMode::Scene, Some(image)) => Screenshot::image(image.clone()),
                    (CaptureMode::Scene, None) => {
                        break 'step Step::Finish(Err(failed(
                            "capture_failed",
                            "the scene render target is missing",
                        )));
                    }
                };
                let (result_tx, result_rx) = crossbeam::channel::bounded(1);
                active.screenshot = Some(capture_png(&mut commands, screenshot, move |png| {
                    if result_tx.try_send(png).is_err() {
                        tracing::warn!("vision capture result dropped — the job already finished");
                    }
                }));
                active.phase = JobPhase::Capturing { result: result_rx };
                Step::Continue
            }
            JobPhase::Capturing { result } => match result.try_recv() {
                Ok(Ok(png)) => Step::Finish(Ok(png)),
                Ok(Err(error)) => Step::Finish(Err(failed("capture_failed", error))),
                Err(crossbeam::channel::TryRecvError::Empty) => Step::Continue,
                Err(crossbeam::channel::TryRecvError::Disconnected) => Step::Finish(Err(failed(
                    "capture_dropped",
                    "the renderer dropped the capture request",
                ))),
            },
        }
    };
    if let Step::Finish(outcome) = step {
        if let Some(active) = job.0.take() {
            let nav = nav.as_deref_mut();
            finish_job(
                active,
                outcome,
                window,
                &mut cameras,
                nav,
                &terrain,
                &mut images,
                &mut commands,
            );
        }
    }
}

/// THE single job exit (spec §5.5, plan tripwire): releases the lock,
/// restores (or, on `restore: false` success, re-derives) the main camera,
/// puts back any navigation the job made (same rule), despawns the capture
/// camera/image/stale screenshot, then replies.
#[allow(clippy::too_many_arguments)]
fn finish_job(
    mut active: ActiveJob,
    outcome: Result<Vec<u8>, VisionFailure>,
    window: Option<&Window>,
    cameras: &mut MainCamera,
    nav: Option<&mut NavigationManager>,
    terrain: &TerrainLookup,
    images: &mut Assets<Image>,
    commands: &mut Commands,
) {
    if active.holds_camera {
        commands.remove_resource::<CameraCaptureLock>();
    }
    let succeeded = outcome.is_ok();
    let put_back = active.restore || !succeeded;
    if let (Some(snapshot), Some(nav)) = (active.nav_snapshot.take(), nav) {
        // Undo only the job's own move: a user who navigated since stays put.
        if put_back && nav.active_petal_id.as_deref() == Some(snapshot.navigated_to.as_str()) {
            snapshot.restore(nav);
        }
    }
    let mut restored = false;
    if let Some(snapshot) = active.snapshot.take() {
        if let Ok((mut transform, mut controller, mut projection)) = cameras.single_mut() {
            if put_back {
                snapshot.restore(&mut transform, &mut controller, &mut projection);
                restored = true;
            } else if let Some(pose) = active.pose {
                rederive_controller(&mut controller, pose);
            }
        }
    }
    if let Some(camera) = active.capture_camera.take() {
        if let Ok(mut entity) = commands.get_entity(camera) {
            entity.try_despawn();
        }
    }
    if let Some(image) = active.image.take() {
        images.remove(&image);
    }
    if !succeeded {
        if let Some(screenshot) = active.screenshot.take() {
            crate::screenshot_bridge::release_unread_screenshot(commands, screenshot);
        }
    }
    let result = outcome.and_then(|png| {
        let pose = active
            .pose
            .ok_or_else(|| failed("unresolved_pose", "the capture finished without a pose"))?;
        let (width, height) = match active.size {
            Some(size) => (size.x, size.y),
            None => {
                let state = window_state(window);
                (state.width, state.height)
            }
        };
        Ok(VisionOutput::Capture {
            png,
            pose: ResolvedPose {
                mode: active.mode,
                position: v3(pose.position),
                target: v3(pose.target),
                up: v3(pose.up),
                fov_deg: f64::from(pose.fov.to_degrees()),
                orbit: orbit_of(pose.position, pose.target),
                geo: geo_of(pose.position, terrain),
                world_scale: terrain.world_scale(),
                width,
                height,
                settle_frames_used: active.frames_waited,
                settled: active.settled,
                tiles_pending: active.tiles_pending,
                restored,
                source: active.source,
            },
        })
    });
    if let Err(failure) = &result {
        tracing::info!(code = failure.code, "agent vision capture refused/failed");
    }
    active.request.reply(result);
}

/// `restore: false`: hand the posed view back to the orbit controller so the
/// next user drag continues from it (roll and pitch beyond ±89° are dropped).
fn rederive_controller(controller: &mut OrbitCameraController, pose: PoseTarget) {
    let Some((focus, distance, yaw, pitch)) = orbit_from_look_at(pose.position, pose.target) else {
        return;
    };
    controller.focus = focus;
    controller.distance = distance;
    controller.target_distance = distance;
    controller.yaw = yaw;
    controller.pitch = pitch.clamp(controller.min_pitch, controller.max_pitch);
    controller.target_focus = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_resource::{Extent3d, TextureDimension};
    use bevy::render::view::screenshot::ScreenshotCaptured;
    use bevy::tasks::{AsyncComputeTaskPool, TaskPool};
    use fe_runtime::vision::{
        vision_bridge, CaptureSpec, FocusPose, OrbitPose, PoseSpec, TilePose, VisionReply,
        VisionRequestSender,
    };
    use fe_ui::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
    use fe_ui::visibility::OverrideState;

    struct Harness {
        app: App,
        tx: VisionRequestSender,
        main: Entity,
    }

    fn harness(window_size: (u32, u32)) -> Harness {
        AsyncComputeTaskPool::get_or_init(TaskPool::new);
        let mut app = App::new();
        let (tx, rx) = vision_bridge();
        install(&mut app, rx);
        let mut time = Time::<Real>::default();
        time.update_with_duration(Duration::ZERO);
        app.insert_resource(time);
        app.init_resource::<Assets<Image>>();
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v", "Verse");
        app.insert_resource(nav);
        app.init_resource::<VisibilityState>();
        app.insert_resource(VerseManager::default());
        let mut window = Window::default();
        window
            .resolution
            .set_physical_resolution(window_size.0, window_size.1);
        app.world_mut().spawn((window, PrimaryWindow));
        let controller = OrbitCameraController::default();
        let transform = compute_orbit_transform(
            controller.focus,
            controller.distance,
            controller.yaw,
            controller.pitch,
        );
        let main = app
            .world_mut()
            .spawn((
                transform,
                controller,
                Projection::Perspective(PerspectiveProjection {
                    fov: std::f32::consts::FRAC_PI_4,
                    near: 0.01,
                    ..default()
                }),
            ))
            .id();
        Harness { app, tx, main }
    }

    impl Harness {
        fn send(&self, op: VisionOp) -> tokio::sync::oneshot::Receiver<VisionReply> {
            self.send_as("VERSE#v", op)
        }

        fn send_as(
            &self,
            scope: &str,
            op: VisionOp,
        ) -> tokio::sync::oneshot::Receiver<VisionReply> {
            let (request, reply_rx) = VisionRequest::new("c1", scope, op);
            self.tx.try_send(request).expect("bridge has room");
            reply_rx
        }

        fn capture(&self, spec: CaptureSpec) -> tokio::sync::oneshot::Receiver<VisionReply> {
            self.send(VisionOp::Capture(spec.validate().expect("valid spec")))
        }

        fn active_petal(&self) -> Option<String> {
            self.app
                .world()
                .resource::<NavigationManager>()
                .active_petal_id
                .clone()
        }

        fn main_state(&self) -> (Transform, [f32; 6], f32) {
            let world = self.app.world();
            let transform = *world.get::<Transform>(self.main).unwrap();
            let c = world.get::<OrbitCameraController>(self.main).unwrap();
            let fov = perspective_fov(world.get::<Projection>(self.main).unwrap()).unwrap();
            (
                transform,
                [c.focus.x, c.focus.y, c.focus.z, c.distance, c.yaw, c.pitch],
                fov,
            )
        }

        fn screenshots(&mut self) -> Vec<Entity> {
            let world = self.app.world_mut();
            world
                .query_filtered::<Entity, With<Screenshot>>()
                .iter(world)
                .collect()
        }

        fn capture_cameras(&mut self) -> Vec<Entity> {
            let world = self.app.world_mut();
            world
                .query_filtered::<Entity, (With<CaptureCamera>, With<StreamingFocus>)>()
                .iter(world)
                .collect()
        }

        fn locked(&self) -> bool {
            self.app.world().contains_resource::<CameraCaptureLock>()
        }

        /// Update until a `Screenshot` is queued (bounded), returning it.
        fn update_until_screenshot(&mut self) -> Entity {
            for _ in 0..50 {
                self.app.update();
                if let Some(shot) = self.screenshots().first().copied() {
                    return shot;
                }
            }
            panic!("no Screenshot was queued within 50 frames");
        }

        /// Fire the renderer's `ScreenshotCaptured` for `shot` with a tiny frame.
        fn deliver_frame(&mut self, shot: Entity) {
            let image = Image::new_fill(
                Extent3d {
                    width: 4,
                    height: 2,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                &[10, 20, 30, 255],
                TextureFormat::Rgba8UnormSrgb,
                bevy::asset::RenderAssetUsages::default(),
            );
            self.app.world_mut().trigger(ScreenshotCaptured {
                entity: shot,
                image,
            });
        }

        /// Update until `reply` resolves (the PNG encodes off-thread).
        fn update_until_reply(
            &mut self,
            reply: &mut tokio::sync::oneshot::Receiver<VisionReply>,
        ) -> VisionReply {
            for _ in 0..500 {
                self.app.update();
                if let Ok(answer) = reply.try_recv() {
                    return answer;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("no vision reply within 500 frames");
        }

        fn advance_clock(&mut self, by: Duration) {
            self.app
                .world_mut()
                .resource_mut::<Time<Real>>()
                .update_with_duration(by);
        }
    }

    fn orbit_capture(mode: CaptureMode, settle: u32, restore: Option<bool>) -> CaptureSpec {
        CaptureSpec {
            mode: Some(mode),
            pose: Some(PoseSpec {
                orbit: Some(OrbitPose {
                    focus: [10.0, 0.0, -4.0],
                    yaw_deg: 30.0,
                    pitch_deg: 20.0,
                    distance: 25.0,
                }),
                ..Default::default()
            }),
            settle_frames: Some(settle),
            restore,
            ..Default::default()
        }
    }

    fn tile_capture(z: u8, x: u32, y: u32) -> CaptureSpec {
        CaptureSpec {
            mode: Some(CaptureMode::Scene),
            pose: Some(PoseSpec {
                tile: Some(TilePose { z, x, y }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn focus_capture(node_id: &str, navigate: bool) -> CaptureSpec {
        CaptureSpec {
            pose: Some(PoseSpec {
                focus: Some(FocusPose {
                    node_id: Some(node_id.to_string()),
                    petal_id: None,
                    navigate: navigate.then_some(true),
                }),
                ..Default::default()
            }),
            settle_frames: Some(0),
            ..Default::default()
        }
    }

    fn expect_failure(reply: VisionReply) -> VisionFailure {
        match reply.result {
            Err(failure) => failure,
            Ok(_) => panic!("expected a refusal"),
        }
    }

    fn expect_capture(reply: VisionReply) -> (Vec<u8>, ResolvedPose) {
        match reply.result {
            Ok(VisionOutput::Capture { png, pose }) => (png, pose),
            Ok(other) => panic!("expected a capture, got {other:?}"),
            Err(failure) => panic!("expected a capture, got {failure:?}"),
        }
    }

    /// Verse v / fractal f with petals p (active, node n1) and q (node n2),
    /// plus verse w / petal r (node n3).
    fn two_petal_tree(h: &mut Harness) {
        let node = |id: &str| NodeEntry {
            transform: Default::default(),
            id: id.to_string(),
            name: id.to_string(),
            ..Default::default()
        };
        let petal = |id: &str, nodes| PetalEntry {
            id: id.to_string(),
            name: id.to_string(),
            nodes,
            ..Default::default()
        };
        h.app.insert_resource(VerseManager::from_verses(vec![
            VerseEntry {
                id: "v".into(),
                name: "Verse".into(),
                fractals: vec![FractalEntry {
                    id: "f".into(),
                    name: "Fractal".into(),
                    petals: vec![petal("p", vec![node("n1")]), petal("q", vec![node("n2")])],
                    ..Default::default()
                }],
                ..Default::default()
            },
            // A second verse the verse-v token does not cover.
            VerseEntry {
                id: "w".into(),
                name: "Other".into(),
                fractals: vec![FractalEntry {
                    id: "g".into(),
                    name: "Other fractal".into(),
                    petals: vec![petal("r", vec![node("n3")])],
                    ..Default::default()
                }],
                ..Default::default()
            },
        ]));
        let mut nav = h.app.world_mut().resource_mut::<NavigationManager>();
        nav.navigate_to_verse("v", "Verse");
        nav.navigate_to_fractal("f", "Fractal");
        nav.navigate_to_petal("p");
    }

    fn spawn_node(h: &mut Harness, node_id: &str, petal_id: &str, at: Vec3) -> Entity {
        h.app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: node_id.to_string(),
                    petal_id: petal_id.to_string(),
                },
                Transform::from_translation(at),
                GlobalTransform::from_translation(at),
                Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            ))
            .id()
    }

    /// A projection at (47.5, -121.5) with one tileset over [47,-122]..[48,-121].
    fn bind_tileset(h: &mut Harness, grounded_origin_ele: f64) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let meta = fe_format::manifest::TilesetMeta {
            bounds: [47.0, -122.0, 48.0, -121.0],
            min_zoom: 10,
            max_zoom: 14,
            ..Default::default()
        };
        std::fs::write(
            dir.path().join("tileset_meta.json"),
            serde_json::to_string(&meta).expect("meta serializes"),
        )
        .expect("meta written");
        let source = fe_terrain::tiles::HexonTileSource::from_directory(dir.path())
            .expect("dev-layout tileset loads");
        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.add_hexon_source(source);
        h.app.insert_resource(ActiveTileSource {
            composite: Some(composite),
            projection: Some(GeoProjection::new(47.5, -121.5, grounded_origin_ele)),
        });
        let mut config = fe_terrain::config::TerrainConfig {
            enabled: true,
            ..Default::default()
        };
        // The persisted origin elevation the runtime grounding superseded.
        config.origin = GeoProjection::new(47.5, -121.5, 0.0);
        h.app.insert_resource(ActivePetalTerrain {
            raw_source_fingerprint: None,
            assignment_fingerprint: None,
            input_scale_valid: false,
            petal_id: Some("p".into()),
            config: Some(config),
            revision: 1,
        });
        dir
    }

    /// Slippy tile containing (lat, lon) at zoom `z`.
    fn tile_of(lat: f64, lon: f64, z: u8) -> (u32, u32) {
        let n = f64::from(1u32 << z);
        let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
        let lat_rad = lat.to_radians();
        let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0
            * n)
            .floor() as u32;
        (x, y)
    }

    /// AC-6: scene mode spawns exactly one capture camera (streaming focus,
    /// image target of the requested size) and never touches the user's
    /// camera; on completion the camera and its image are gone.
    #[test]
    fn scene_mode_spawns_one_capture_camera_and_leaves_the_main_camera_untouched() {
        let mut h = harness((1280, 720));
        let before = h.main_state();
        let mut spec = orbit_capture(CaptureMode::Scene, 3, None);
        spec.width = Some(640);
        spec.height = Some(360);
        let mut reply = h.capture(spec);
        h.app.update();

        let cameras = h.capture_cameras();
        assert_eq!(
            cameras.len(),
            1,
            "exactly one CaptureCamera + StreamingFocus"
        );
        let target = h.app.world().get::<RenderTarget>(cameras[0]).cloned();
        let Some(RenderTarget::Image(image_target)) = target else {
            panic!("capture camera must render to an image, got {target:?}");
        };
        let image = h
            .app
            .world()
            .resource::<Assets<Image>>()
            .get(&image_target.handle)
            .expect("render target image exists");
        assert_eq!((image.width(), image.height()), (640, 360));
        assert!(!h.locked(), "scene mode never locks the user's camera");
        assert_eq!(h.main_state(), before, "main camera bit-identical");

        let shot = h.update_until_screenshot();
        h.deliver_frame(shot);
        let (png, pose) = expect_capture(h.update_until_reply(&mut reply));
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert_eq!(pose.mode, CaptureMode::Scene);
        assert_eq!((pose.width, pose.height), (640, 360));
        assert!(pose.settled && pose.settle_frames_used >= 3);
        assert!(!pose.restored);
        h.app.update();
        assert!(h.capture_cameras().is_empty(), "capture camera despawned");
        assert_eq!(
            h.app.world().resource::<Assets<Image>>().len(),
            0,
            "image freed"
        );
        assert_eq!(h.main_state(), before, "main camera still bit-identical");
    }

    /// Spec §5.4(f): a scene capture waits past `settle_frames` until the
    /// capture focus's terrain ring is resident; at the settle cap it is
    /// taken anyway and honestly reports `settled: false` + the pending count.
    #[test]
    fn scene_settle_waits_for_the_capture_focus_ring_to_be_resident() {
        for resolves in [true, false] {
            let mut h = harness((1280, 720));
            h.app.init_resource::<CaptureFocusResidency>();
            let mut reply = h.capture(orbit_capture(CaptureMode::Scene, 2, None));
            h.app.update();
            let camera = h.capture_cameras()[0];
            // What fetch_and_spawn_terrain_chunks publishes for this focus.
            *h.app.world_mut().resource_mut::<CaptureFocusResidency>() = CaptureFocusResidency {
                capture_focus: Some(camera),
                pending: 3,
            };
            for _ in 0..10 {
                h.app.update();
            }
            assert!(
                h.screenshots().is_empty(),
                "settle_frames passed but tiles are pending"
            );
            if resolves {
                h.app
                    .world_mut()
                    .resource_mut::<CaptureFocusResidency>()
                    .pending = 0;
            } else {
                h.advance_clock(SETTLE_WALL_CLOCK_CAP);
            }
            let shot = h.update_until_screenshot();
            h.deliver_frame(shot);
            let (_, pose) = expect_capture(h.update_until_reply(&mut reply));
            assert_eq!(pose.settled, resolves);
            assert_eq!(pose.tiles_pending, if resolves { 0 } else { 3 });
        }
    }

    /// AC-6: window mode holds the pose under `CameraCaptureLock` for the
    /// whole settle, then restores the snapshot exactly.
    #[test]
    fn window_mode_locks_applies_the_pose_and_restores_the_snapshot() {
        use bevy::asset::AssetEvent;
        use bevy::camera::CameraProjection;
        use bevy::render::{camera::CameraPlugin, texture::ManualTextureViews};
        use bevy::window::{WindowCreated, WindowResized, WindowScaleFactorChanged};
        let mut h = harness((1280, 720));
        h.app.init_resource::<ManualTextureViews>()
            .add_message::<AssetEvent<Image>>()
            .add_message::<WindowCreated>()
            .add_message::<WindowResized>()
            .add_message::<WindowScaleFactorChanged>()
            .add_plugins((bevy::transform::TransformPlugin, CameraPlugin));
        crate::terrain_bridge::install_ruler_hud(&mut h.app);
        h.app.world_mut().entity_mut(h.main).insert(Camera::default());
        {
            let mut projection = h.app.world_mut().get_mut::<Projection>(h.main).unwrap();
            let Projection::Perspective(projection) = &mut *projection else { panic!("perspective camera required"); };
            projection.fov = 60.0_f32.to_radians();
        }
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = observed.clone();
        h.app.add_systems(PostUpdate, (move |cameras: Query<(&Camera, &Projection, &GlobalTransform), With<OrbitCameraController>>| {
            let (camera, projection, pose) = cameras.single().unwrap();
            let Projection::Perspective(projection) = projection else { panic!("perspective camera required"); };
            sink.lock().unwrap().push((projection.fov, camera.clip_from_view(),
                projection.get_clip_from_view(), pose.translation()));
        }).in_set(bevy_egui::EguiPostUpdateSet::EndPass));
        h.app.update();
        let before = h.main_state();
        assert_eq!(before.2, 60.0_f32.to_radians());
        let mut reply = h.capture(orbit_capture(CaptureMode::Window, 4, None));
        h.app.update();
        assert!(h.locked(), "the pose is held under the lock");
        let expected = compute_orbit_transform(
            Vec3::new(10.0, 0.0, -4.0),
            25.0,
            30f32.to_radians(),
            20f32.to_radians(),
        );
        let (posed, _, _) = h.main_state();
        assert!((posed.translation - expected.translation).length() < 1e-4);
        let first_applied = observed.lock().unwrap().last().copied().unwrap();
        assert_eq!(first_applied.0, (DEFAULT_FOV_DEG as f32).to_radians());
        assert_eq!(first_applied.1, first_applied.2,
            "first applied EndPass uses the projection actually cached for rendering");
        assert!((first_applied.3 - expected.translation).length() < 1e-4);
        assert!(h.screenshots().is_empty(), "still settling");

        let shot = h.update_until_screenshot();
        assert!(h.locked(), "still held while the frame is captured");
        h.deliver_frame(shot);
        let (_, pose) = expect_capture(h.update_until_reply(&mut reply));
        assert_eq!(pose.source, PoseSource::Orbit);
        assert!(pose.restored);
        assert_eq!(pose.settle_frames_used, 4);
        assert!(
            (Vec3::new(
                pose.position[0] as f32,
                pose.position[1] as f32,
                pose.position[2] as f32
            ) - expected.translation)
                .length()
                < 1e-4
        );
        h.app.update();
        assert!(!h.locked(), "lock released");
        assert_eq!(h.main_state(), before, "snapshot restored bit-identically");
        let frames = observed.lock().unwrap();
        let applied = frames.iter().position(|frame| frame.0 == first_applied.0).unwrap();
        let first_restored = frames[applied + 1..].iter().find(|frame| frame.0 == before.2).unwrap();
        assert_eq!(first_restored.1, first_restored.2,
            "first actual job-restore EndPass uses the restored cached render projection");
        assert!((first_restored.3 - before.0.translation).length() < 1e-4);
        assert!(frames.iter().all(|frame| frame.1 == frame.2),
            "no intermediate EndPass can observe a raw FOV ahead of its cached render matrix");
    }

    #[test]
    fn window_mode_restore_false_rederives_the_orbit_controller() {
        let mut h = harness((1280, 720));
        let mut reply = h.capture(orbit_capture(CaptureMode::Window, 0, Some(false)));
        let shot = h.update_until_screenshot();
        h.deliver_frame(shot);
        let (_, pose) = expect_capture(h.update_until_reply(&mut reply));
        assert!(!pose.restored);
        assert!(!h.locked());
        let world = h.app.world();
        let c = world.get::<OrbitCameraController>(h.main).unwrap();
        assert!((c.focus - Vec3::new(10.0, 0.0, -4.0)).length() < 1e-3);
        assert!((c.distance - 25.0).abs() < 1e-3 && (c.target_distance - 25.0).abs() < 1e-3);
        assert!((c.yaw - 30f32.to_radians()).abs() < 1e-4);
        assert!((c.pitch - 20f32.to_radians()).abs() < 1e-4);
        // The next orbit frame recomputes exactly the posed transform: no jump.
        let next = compute_orbit_transform(c.focus, c.distance, c.yaw, c.pitch);
        let posed = world.get::<Transform>(h.main).unwrap();
        assert!((next.translation - posed.translation).length() < 1e-3);
    }

    /// AC-6: a minimized window refuses window mode in the same frame (no
    /// lock, no capture) but still serves scene mode off-screen.
    #[test]
    fn minimized_window_refuses_window_mode_at_once_but_serves_scene_mode() {
        let mut h = harness((0, 0));
        let mut window_reply = h.capture(orbit_capture(CaptureMode::Window, 0, None));
        h.app.update();
        let refusal = expect_failure(window_reply.try_recv().expect("answered in one frame"));
        assert_eq!(refusal.code, "window_minimized");
        assert_eq!(refusal.status_class, VisionFailureClass::NoWindow);
        assert!(!h.locked());
        assert!(h.screenshots().is_empty());

        let mut scene_reply = h.capture(orbit_capture(CaptureMode::Scene, 0, None));
        let shot = h.update_until_screenshot();
        assert_eq!(h.capture_cameras().len(), 1);
        h.deliver_frame(shot);
        let (_, pose) = expect_capture(h.update_until_reply(&mut scene_reply));
        assert_eq!(pose.mode, CaptureMode::Scene);
    }

    #[test]
    fn a_second_capture_while_a_job_is_active_replies_capture_in_progress() {
        let mut h = harness((1280, 720));
        let mut first = h.capture(orbit_capture(CaptureMode::Window, 100, None));
        h.app.update();
        let mut second = h.capture(orbit_capture(CaptureMode::Scene, 0, None));
        h.app.update();
        let busy = expect_failure(second.try_recv().expect("refused at once"));
        assert_eq!(busy.code, "capture_in_progress");
        assert_eq!(busy.status_class, VisionFailureClass::Busy);
        assert!(
            h.capture_cameras().is_empty(),
            "the refused job spawned nothing"
        );
        assert!(first.try_recv().is_err(), "the first job is still running");
    }

    /// Spec §5.4(f) + the teardown guarantee: past the settle cap the frame
    /// is captured anyway (`settled: false`); if the renderer never answers,
    /// the job cap tears down, restores the camera and releases the lock.
    #[test]
    fn wall_clock_cap_tears_down_and_restores_the_camera() {
        let mut h = harness((1280, 720));
        let before = h.main_state();
        let mut reply = h.capture(orbit_capture(CaptureMode::Window, 600, None));
        h.app.update();
        h.app.update();
        assert!(h.locked() && h.screenshots().is_empty());

        h.advance_clock(SETTLE_WALL_CLOCK_CAP + Duration::from_millis(1));
        h.app.update();
        assert_eq!(h.screenshots().len(), 1, "settle cap → captured unsettled");

        h.advance_clock(JOB_WALL_CLOCK_CAP - SETTLE_WALL_CLOCK_CAP);
        h.app.update();
        let failure = expect_failure(reply.try_recv().expect("torn down this frame"));
        assert_eq!(failure.code, "capture_timed_out");
        h.app.update();
        assert!(!h.locked(), "lock released on teardown");
        assert_eq!(h.main_state(), before, "camera restored on teardown");
        assert!(
            h.screenshots().is_empty(),
            "the orphaned Screenshot is despawned"
        );
    }

    #[test]
    fn tile_pose_without_a_projection_refuses_no_projection() {
        let mut h = harness((1280, 720));
        let mut reply = h.capture(tile_capture(12, 665, 1432));
        h.app.update();
        let refusal = expect_failure(reply.try_recv().expect("answered in one frame"));
        assert_eq!(refusal.code, "no_projection");
        assert_eq!(refusal.status_class, VisionFailureClass::Unresolvable);
        assert!(h.capture_cameras().is_empty());
    }

    /// A tile outside every bound tileset is refused before anything spawns;
    /// one inside frames top-down over the tile's projected centre.
    #[test]
    fn tile_pose_outside_every_tileset_refuses() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let _tileset = bind_tileset(&mut h, 120.0);
        let (x, y) = tile_of(0.5, 0.5, 12);
        let mut outside = h.capture(tile_capture(12, x, y));
        h.app.update();
        let refusal = expect_failure(outside.try_recv().expect("answered in one frame"));
        assert_eq!(refusal.code, "tile_outside_tileset");
        assert!(h.capture_cameras().is_empty());

        let (x, y) = tile_of(47.5, -121.5, 12);
        let mut inside = h.capture(tile_capture(12, x, y));
        h.app.update();
        assert!(
            inside.try_recv().is_err(),
            "an inside tile is captured, not refused"
        );
        let cameras = h.capture_cameras();
        assert_eq!(cameras.len(), 1);
        let posed = *h.app.world().get::<Transform>(cameras[0]).unwrap();
        let bounds = tile_bounds(12, x, y);
        let projection = GeoProjection::new(47.5, -121.5, 120.0);
        let centre = projection
            .wgs84_to_local(
                (bounds[0] + bounds[2]) / 2.0,
                (bounds[1] + bounds[3]) / 2.0,
                120.0,
            )
            .unwrap();
        assert!((posed.translation.x - centre[0] as f32).abs() < 1e-2);
        assert!((posed.translation.z - centre[2] as f32).abs() < 1e-2);
        assert!(posed.translation.y > 0.0);
        assert!(
            Vec3::from(posed.forward()).dot(Vec3::NEG_Y) > 0.999,
            "looks straight down"
        );
    }

    #[test]
    fn focus_on_a_hidden_node_refuses_node_hidden() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        spawn_node(&mut h, "n1", "p", Vec3::new(3.0, 0.0, 3.0));
        h.app
            .world_mut()
            .resource_mut::<VisibilityState>()
            .set_node_override("n1", OverrideState::Hide);
        let mut reply = h.capture(focus_capture("n1", false));
        h.app.update();
        let refusal = expect_failure(reply.try_recv().expect("answered in one frame"));
        assert_eq!(refusal.code, "node_hidden");
        assert!(!h.locked(), "a refused focus never locks the camera");

        // The same node, shown again, frames: the refusal was the hide rule.
        h.app
            .world_mut()
            .resource_mut::<VisibilityState>()
            .set_node_override("n1", OverrideState::Auto);
        let mut reply = h.capture(focus_capture("n1", false));
        let shot = h.update_until_screenshot();
        h.deliver_frame(shot);
        let (_, pose) = expect_capture(h.update_until_reply(&mut reply));
        assert_eq!(pose.source, PoseSource::Focus);
        assert!((pose.target[0] - 3.0).abs() < 1e-4 && (pose.target[2] - 3.0).abs() < 1e-4);
    }

    #[test]
    fn focus_on_another_petal_without_navigate_refuses() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let mut reply = h.capture(focus_capture("n2", false));
        h.app.update();
        let refusal = expect_failure(reply.try_recv().expect("answered in one frame"));
        assert_eq!(refusal.code, "not_active_petal");
        let nav = h.app.world().resource::<NavigationManager>();
        assert_eq!(
            nav.active_petal_id.as_deref(),
            Some("p"),
            "no navigation happened"
        );
    }

    /// `navigate: true` (window mode) switches the active petal, waits for
    /// the node to spawn there, then frames it under the lock.
    #[test]
    fn focus_with_navigate_switches_petal_then_frames_the_node() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let mut reply = h.capture(focus_capture("n2", true));
        h.app.update();
        assert_eq!(
            h.app
                .world()
                .resource::<NavigationManager>()
                .active_petal_id
                .as_deref(),
            Some("q"),
            "navigated to the node's petal"
        );
        assert!(reply.try_recv().is_err(), "waiting for the node to spawn");
        spawn_node(&mut h, "n2", "q", Vec3::new(-8.0, 1.0, 2.0));
        let shot = h.update_until_screenshot();
        assert!(h.locked());
        h.deliver_frame(shot);
        let (_, pose) = expect_capture(h.update_until_reply(&mut reply));
        assert!((pose.target[0] + 8.0).abs() < 1e-4 && (pose.target[1] - 1.0).abs() < 1e-4);
        assert_eq!(
            h.active_petal().as_deref(),
            Some("p"),
            "restore (default) puts the user's navigation back"
        );
    }

    /// `restore: false` keeps both the posed camera AND the navigation.
    #[test]
    fn focus_with_navigate_and_restore_false_stays_in_the_target_petal() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let mut spec = focus_capture("n2", true);
        spec.restore = Some(false);
        let mut reply = h.capture(spec);
        h.app.update();
        spawn_node(&mut h, "n2", "q", Vec3::new(-8.0, 1.0, 2.0));
        let shot = h.update_until_screenshot();
        h.deliver_frame(shot);
        expect_capture(h.update_until_reply(&mut reply));
        assert_eq!(h.active_petal().as_deref(), Some("q"));
    }

    /// A navigating job that fails (here: the job cap, the node never
    /// spawned) never leaves the user somewhere else.
    #[test]
    fn navigate_then_fail_returns_navigation_to_the_original_petal() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let before = h.main_state();
        let mut spec = focus_capture("n2", true);
        spec.restore = Some(false);
        let mut reply = h.capture(spec);
        h.app.update();
        assert_eq!(h.active_petal().as_deref(), Some("q"), "navigated");
        h.advance_clock(JOB_WALL_CLOCK_CAP);
        h.app.update();
        let failure = expect_failure(reply.try_recv().expect("torn down"));
        assert_eq!(failure.code, "capture_timed_out");
        assert_eq!(
            h.active_petal().as_deref(),
            Some("p"),
            "failure restores navigation"
        );
        h.app.update();
        assert!(!h.locked());
        assert_eq!(h.main_state(), before);
    }

    /// SEC-ADDENDUM 1: a token that does not cover the ACTIVE verse gets the
    /// uniform denial for captures and camera reads alike — nothing moves.
    #[test]
    fn a_token_outside_the_active_verse_is_denied_capture_and_camera_read() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let before = h.main_state();
        let spec = orbit_capture(CaptureMode::Scene, 0, None)
            .validate()
            .unwrap();
        let mut capture = h.send_as("VERSE#w", VisionOp::Capture(spec));
        let mut read = h.send_as("VERSE#w", VisionOp::ReadCamera);
        h.app.update();
        for reply in [&mut capture, &mut read] {
            let refusal = expect_failure(reply.try_recv().expect("answered at once"));
            assert_eq!(refusal, denied());
        }
        assert!(h.capture_cameras().is_empty() && !h.locked());
        assert_eq!(h.main_state(), before);
        // The covering token is served (the denial was the scope, not the op).
        let mut read = h.send_as("VERSE#v", VisionOp::ReadCamera);
        h.app.update();
        assert!(read.try_recv().expect("answered").result.is_ok());
        // Under a validated QA root the local-operator scope sees this window's view,
        // and a foreign verse token is still denied (src/AGENTS.md §qa-operator).
        fe_runtime::qa_debug::enable_qa_operator();
        let mut operator = h.send_as(fe_runtime::qa_debug::QA_OPERATOR_SCOPE, VisionOp::ReadCamera);
        let mut foreign = h.send_as("VERSE#w", VisionOp::ReadCamera);
        h.app.update();
        assert!(operator.try_recv().expect("answered").result.is_ok());
        assert_eq!(expect_failure(foreign.try_recv().expect("answered")), denied());
    }

    /// SEC-ADDENDUM 2/5: an existing target in an uncovered verse and a
    /// target that does not exist get the SAME error (no existence oracle);
    /// navigating toward another verse is refused and never moves the user.
    #[test]
    fn foreign_and_nonexistent_focus_targets_get_the_same_denial() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let mut foreign = h.capture(focus_capture("n3", false));
        let mut ghost = h.capture(focus_capture("ghost", false));
        h.app.update();
        h.app.update();
        let foreign = expect_failure(foreign.try_recv().expect("answered"));
        let ghost = expect_failure(ghost.try_recv().expect("answered"));
        assert_eq!(foreign, denied());
        assert_eq!(foreign, ghost, "indistinguishable");

        for target in ["n3", "ghost"] {
            let mut reply = h.capture(focus_capture(target, true));
            h.app.update();
            assert_eq!(
                expect_failure(reply.try_recv().expect("answered")),
                denied()
            );
            assert_eq!(h.active_petal().as_deref(), Some("p"), "never navigated");
            assert!(!h.locked());
        }
    }

    /// The scope is re-checked at capture time: if the user opens a verse the
    /// caller does not cover while the job settles, no frame is taken.
    #[test]
    fn leaving_the_covered_verse_mid_settle_denies_the_capture() {
        {
            use crate::qa_portal_bridge::context;
            use fe_ui::{actions::UiManager, node_manager::NodeManager, plugin::LocalUserRole, portal::PortalState};
            let mut nav = NavigationManager::default();
            nav.navigate_to_petal_in_fractal("v", "V", "f", "F", "p");
            let entity = Entity::from_bits(1);
            let mut nodes = NodeManager::default();
            nodes.select(entity, "n");
            let mut ui = UiManager::default();
            ui.portal = PortalState::Open { current_url:"http://127.0.0.1:49152/web/one".into(),cached_hostname:"127.0.0.1".into(),opened_for_entity:entity };
            let mut role = LocalUserRole { role: Some(fe_database::RoleLevel::Owner) };
            let captured = context(&nav, &nodes, &ui, &role);
            assert!(captured.is_some());
            role.role = Some(fe_database::RoleLevel::Viewer);
            assert!(context(&nav, &nodes, &ui, &role).is_none(), "role loss prevents native result release");
            role.role = Some(fe_database::RoleLevel::Owner);
            nav.navigate_to_petal("other");
            assert_ne!(captured, context(&nav, &nodes, &ui, &role), "changed Petal invalidates the pending native job");
            nav.navigate_to_petal("p");
            ui.portal = PortalState::Closed;
            assert!(context(&nav, &nodes, &ui, &role).is_none(), "closed Portal cannot release bytes");
        }
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let before = h.main_state();
        let mut reply = h.capture(orbit_capture(CaptureMode::Window, 5, None));
        h.app.update();
        assert!(h.locked());
        h.app
            .world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse("w", "Other");
        for _ in 0..10 {
            h.app.update();
        }
        assert_eq!(
            expect_failure(reply.try_recv().expect("answered")),
            denied()
        );
        assert!(
            h.screenshots().is_empty(),
            "no frame of the uncovered verse"
        );
        assert!(!h.locked());
        assert_eq!(h.main_state(), before);
    }

    /// Verse w open (petal r), as after the user switched there from v.
    fn open_verse_w(h: &mut Harness) {
        let mut nav = h.app.world_mut().resource_mut::<NavigationManager>();
        nav.navigate_to_verse("w", "Other");
        nav.navigate_to_fractal("g", "Other fractal");
        nav.navigate_to_petal("r");
    }

    /// A verse-w capture and camera read, both expected to get the denial.
    fn assert_verse_w_denied(h: &mut Harness) {
        let spec = orbit_capture(CaptureMode::Scene, 0, None)
            .validate()
            .unwrap();
        let mut capture = h.send_as("VERSE#w", VisionOp::Capture(spec));
        let mut read = h.send_as("VERSE#w", VisionOp::ReadCamera);
        h.app.update();
        for reply in [&mut capture, &mut read] {
            assert_eq!(
                expect_failure(reply.try_recv().expect("answered at once")),
                denied()
            );
        }
        assert!(h.capture_cameras().is_empty());
    }

    fn verse_w_read_is_served(h: &mut Harness) -> bool {
        let mut read = h.send_as("VERSE#w", VisionOp::ReadCamera);
        h.app.update();
        read.try_recv().expect("answered").result.is_ok()
    }

    /// SEC-ADDENDUM, the lingering view: terrain is re-bound only when the
    /// next petal's terrain loads and nodes despawn a frame late, so just
    /// after the user opens verse w the screen can still show verse v's map
    /// or nodes. A verse-w token is denied while anything of v is rendered —
    /// at arrival and again at frame time — and served once only w is.
    #[test]
    fn a_view_still_rendering_an_uncovered_verse_is_denied() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let _tileset = bind_tileset(&mut h, 120.0); // verse v's petal p map
        open_verse_w(&mut h);
        assert_verse_w_denied(&mut h);

        h.app
            .world_mut()
            .resource_mut::<ActivePetalTerrain>()
            .petal_id = Some("r".into());
        assert!(
            verse_w_read_is_served(&mut h),
            "only verse w is on screen now"
        );

        let stale = spawn_node(&mut h, "n1", "p", Vec3::ZERO);
        assert_verse_w_denied(&mut h);
        h.app.world_mut().despawn(stale);
        assert!(verse_w_read_is_served(&mut h));

        // Frame-time re-check: v content reappearing mid-settle denies.
        let spec = orbit_capture(CaptureMode::Window, 5, None)
            .validate()
            .unwrap();
        let mut reply = h.send_as("VERSE#w", VisionOp::Capture(spec));
        h.app.update();
        assert!(h.locked(), "admitted at arrival");
        spawn_node(&mut h, "n1", "p", Vec3::ZERO);
        for _ in 0..10 {
            h.app.update();
        }
        assert_eq!(
            expect_failure(reply.try_recv().expect("answered")),
            denied()
        );
        assert!(h.screenshots().is_empty(), "no frame showing verse v");
        assert!(!h.locked());
    }

    /// A caller that hung up (client gone, API timeout) never holds the
    /// user's camera: a request abandoned before the drain moves nothing, and
    /// a job whose caller leaves mid-settle is torn down and restored at once.
    #[test]
    fn an_abandoned_capture_never_holds_the_users_camera() {
        let mut h = harness((1280, 720));
        let before = h.main_state();
        drop(h.capture(orbit_capture(CaptureMode::Window, 100, None)));
        h.app.update();
        assert!(!h.locked(), "an abandoned request never starts");
        assert_eq!(h.main_state(), before);

        let reply = h.capture(orbit_capture(CaptureMode::Window, 100, None));
        h.app.update();
        assert!(h.locked());
        assert_ne!(h.main_state(), before, "posed");
        drop(reply);
        h.app.update();
        h.app.update();
        assert!(!h.locked(), "torn down once the caller left");
        assert_eq!(h.main_state(), before, "camera restored");
        assert!(h.screenshots().is_empty());
    }

    /// A failing navigating job puts the user back only while they are still
    /// where the job took them; a user who moved on meanwhile stays put.
    #[test]
    fn a_user_who_navigated_away_mid_job_is_not_yanked_back() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let mut reply = h.capture(focus_capture("n2", true));
        h.app.update();
        assert_eq!(h.active_petal().as_deref(), Some("q"), "navigated");
        open_verse_w(&mut h);
        h.advance_clock(JOB_WALL_CLOCK_CAP);
        h.app.update();
        let failure = expect_failure(reply.try_recv().expect("torn down"));
        assert_eq!(failure.code, "capture_timed_out");
        let nav = h.app.world().resource::<NavigationManager>();
        assert_eq!(nav.active_verse_id.as_deref(), Some("w"));
        assert_eq!(
            nav.active_petal_id.as_deref(),
            Some("r"),
            "the user's own move stands"
        );
    }

    /// AC-8: the read-back names the active hierarchy and the RUNTIME-grounded
    /// origin elevation, not the persisted one.
    #[test]
    fn read_camera_reports_active_ids_and_the_grounded_origin() {
        let mut h = harness((1280, 720));
        two_petal_tree(&mut h);
        let _tileset = bind_tileset(&mut h, 312.5);
        let mut reply = h.send(VisionOp::ReadCamera);
        h.app.update();
        let state = match reply.try_recv().expect("answered in one frame").result {
            Ok(VisionOutput::Camera(state)) => state,
            other => panic!("expected camera state, got {other:?}"),
        };
        assert_eq!(state.active.verse_id.as_deref(), Some("v"));
        assert_eq!(state.active.fractal_id.as_deref(), Some("f"));
        assert_eq!(state.active.petal_id.as_deref(), Some("p"));
        let projection = state.projection.expect("a map is bound");
        assert_eq!(projection.origin_ele_grounded, 312.5);
        assert_eq!(
            projection.tileset_bounds,
            Some([47.0, -122.0, 48.0, -121.0])
        );
        assert_eq!(projection.zoom_range, Some([10, 14]));
        assert!(!state.locked);
        assert_eq!((state.window.width, state.window.height), (1280, 720));
        let (transform, _, _) = h.main_state();
        assert!((state.pose.position[0] as f32 - transform.translation.x).abs() < 1e-5);
        assert!(state.pose.geo.is_some(), "position reads back as lat/lon");

        use fe_renderer::scene_analysis::{
            ObjectGeometry, SceneAnalysisSnapshot, SceneObjectBounds,
        };
        use fe_renderer::terrain_height::{HeightTile, TerrainHeightField};
        let diagnostic = crate::qa_debug_bridge::selected_terrain_diagnostic;
        let world = h.app.world();
        let active = world.resource::<ActivePetalTerrain>();
        let source = world.resource::<ActiveTileSource>();
        let mut scene = SceneAnalysisSnapshot {
            petal_id: Some("p".into()),
            revision: 17,
            objects: vec![SceneObjectBounds {
                node_id: "selected".into(),
                name: "unexported object name".into(),
                bounds: Some([[-1.25, 0.2, -1.25], [1.25, 1.2, 1.25]]),
                geometry: ObjectGeometry::AxisAlignedSolidBox,
                selectable: true,
                material_json: Some("unexported material bag".into()),
                material_loaded: true,
                material_truncated: false,
            }],
            ..Default::default()
        };
        let mut heights = TerrainHeightField::default();
        heights.insert(
            (1, 0, 0),
            HeightTile {
                anchor: Vec3::new(-10.0, -0.13673553, -10.0),
                tile_size: 20.0,
                grid: vec![0.0; 4],
                width: 2,
                height: 2,
            },
        );
        let result = diagnostic(
            Some("selected"),
            Some("p"),
            Some(&scene),
            Some(active),
            Some(source),
            Some(&heights),
            123,
        );
        assert_eq!(result["status"], "available");
        assert_eq!(result["scene_revision"], 17);
        assert_eq!(result["terrain_assignment_revision"], active.revision);
        assert_eq!(result["terrain_epoch_verified"], false);
        assert_eq!(result["sampled_at_frame"], 123);
        assert_eq!(
            result["runtime_origin"]["elevation_m"], 312.5,
            "runtime projection wins over persisted zero"
        );
        assert_eq!(
            result["world_aabb"]["min"],
            serde_json::json!([-1.25, 0.2f32, -1.25])
        );
        assert_eq!(result["geometry"], "axis_aligned_solid_box");
        let probes = result["probes"].as_array().unwrap();
        assert_eq!(probes.len(), 5);
        assert_eq!(probes[0]["xz"], serde_json::json!([0.0, 0.0]));
        for (probe, expected) in
            probes[1..]
                .iter()
                .zip([[-1.25, -1.25], [-1.25, 1.25], [1.25, -1.25], [1.25, 1.25]])
        {
            assert_eq!(probe["xz"], serde_json::json!(expected));
        }
        for probe in probes {
            assert_eq!(probe["status"], "finite");
            assert!((probe["base_height"].as_f64().unwrap() + 0.13673553).abs() < 1e-7);
        }
        assert!(!result.to_string().contains("unexported"));
        assert_eq!(
            result["temporal_scope"],
            "request_time_only_not_a_report_replay"
        );
        for node in [None, Some("not-the-selection")] {
            let refused = diagnostic(
                node,
                Some("p"),
                Some(&scene),
                Some(active),
                Some(source),
                Some(&heights),
                124,
            );
            assert_eq!(refused["status"], "unavailable");
            assert!(refused.get("probes").is_none());
        }
        let empty_scene = SceneAnalysisSnapshot::default();
        for empty in [None, Some(&empty_scene)] {
            let unavailable = diagnostic(
                Some("selected"),
                Some("p"),
                empty,
                Some(active),
                Some(source),
                Some(&heights),
                124,
            );
            assert_eq!(unavailable["status"], "unavailable");
            assert_eq!(unavailable["reason"], "scene_snapshot_unavailable");
            assert!(unavailable.get("world_aabb").is_none());
            assert!(unavailable.get("probes").is_none());
        }
        scene.petal_id = Some("foreign".into());
        let stale = diagnostic(
            Some("selected"),
            Some("p"),
            Some(&scene),
            Some(active),
            Some(source),
            Some(&heights),
            124,
        );
        assert_eq!(stale["reason"], "scene_petal_mismatch");
        assert_eq!(stale["status"], "stale");
        assert!(stale.get("world_aabb").is_none());
        scene.petal_id = Some("p".into());
        let foreign = ActivePetalTerrain {
            raw_source_fingerprint: None,
            assignment_fingerprint: None,
            input_scale_valid: false,
            petal_id: Some("foreign".into()),
            ..Default::default()
        };
        assert_eq!(
            diagnostic(
                Some("selected"),
                Some("p"),
                Some(&scene),
                Some(&foreign),
                Some(source),
                Some(&heights),
                124
            )["reason"],
            "terrain_petal_mismatch"
        );
        let empty_heights = TerrainHeightField::default();
        for unavailable_heights in [None, Some(&empty_heights)] {
            let missing = diagnostic(
                Some("selected"),
                Some("p"),
                Some(&scene),
                Some(active),
                Some(source),
                unavailable_heights,
                125,
            );
            assert_eq!(missing["status"], "incomplete");
            assert_eq!(missing["probes"].as_array().unwrap().len(), 5);
            for probe in missing["probes"].as_array().unwrap() {
                assert_eq!(probe["status"], "missing");
                assert!(probe["base_height"].is_null());
            }
        }
        heights.insert(
            (1, 0, 0),
            HeightTile {
                anchor: Vec3::new(-10.0, 0.0, -10.0),
                tile_size: 20.0,
                grid: vec![f32::NAN; 4],
                width: 2,
                height: 2,
            },
        );
        let invalid_samples = diagnostic(
            Some("selected"),
            Some("p"),
            Some(&scene),
            Some(active),
            Some(source),
            Some(&heights),
            126,
        );
        assert_eq!(invalid_samples["status"], "incomplete");
        for probe in invalid_samples["probes"].as_array().unwrap() {
            assert_eq!(probe["status"], "nonfinite");
            assert!(probe["base_height"].is_null());
        }
        assert_eq!(
            diagnostic(
                Some("selected"),
                Some("p"),
                Some(&scene),
                Some(active),
                None,
                Some(&heights),
                126
            )["reason"],
            "runtime_projection_unavailable"
        );
        let wrong_projection = ActiveTileSource {
            projection: Some(GeoProjection::new(48.0, -121.5, 312.5)),
            ..Default::default()
        };
        let stale_projection = diagnostic(
            Some("selected"),
            Some("p"),
            Some(&scene),
            Some(active),
            Some(&wrong_projection),
            Some(&heights),
            126,
        );
        assert_eq!(stale_projection["status"], "stale");
        assert!(stale_projection.get("runtime_origin").is_none());
        scene.objects[0].bounds.as_mut().unwrap()[1][0] = -1.25;
        assert_eq!(
            diagnostic(
                Some("selected"),
                Some("p"),
                Some(&scene),
                Some(active),
                Some(source),
                Some(&heights),
                127
            )["reason"],
            "selected_bounds_invalid"
        );
        scene.objects[0].bounds = Some([[0.0; 3], [f32::INFINITY; 3]]);
        assert_eq!(
            diagnostic(
                Some("selected"),
                Some("p"),
                Some(&scene),
                Some(active),
                Some(source),
                Some(&heights),
                127
            )["reason"],
            "selected_bounds_invalid"
        );
    }

    #[test]
    fn read_camera_reports_locked_during_a_window_job() {
        let mut h = harness((1280, 720));
        let _job = h.capture(orbit_capture(CaptureMode::Window, 100, None));
        h.app.update();
        let mut reply = h.send(VisionOp::ReadCamera);
        h.app.update();
        let state = match reply
            .try_recv()
            .expect("answered while the job runs")
            .result
        {
            Ok(VisionOutput::Camera(state)) => state,
            other => panic!("expected camera state, got {other:?}"),
        };
        assert!(state.locked, "read-back says a job holds the camera");
    }

    #[test]
    fn gizmo_groups_render_only_to_the_main_camera() {
        use bevy::gizmos::config::{DefaultGizmoConfigGroup, GizmoConfig};
        let mut h = harness((1280, 720));
        let mut store = GizmoConfigStore::default();
        store.insert(GizmoConfig::default(), DefaultGizmoConfigGroup);
        h.app.insert_resource(store);
        h.app.update();
        let world = h.app.world();
        let main_layers = world
            .get::<RenderLayers>(h.main)
            .expect("main camera layered");
        let gizmo_layers = world
            .resource::<GizmoConfigStore>()
            .config::<DefaultGizmoConfigGroup>()
            .0
            .render_layers
            .clone();
        assert!(
            main_layers.intersects(&gizmo_layers),
            "main camera draws gizmos"
        );
        assert!(
            !RenderLayers::default().intersects(&gizmo_layers),
            "a default-layer capture camera does not"
        );
        assert!(
            main_layers.intersects(&RenderLayers::default()),
            "main still sees the scene"
        );
    }
}
