//! Orbit camera controller for the 3D viewport.
//!
//! Provides a Blender-style orbit camera with middle-mouse orbit, shift+middle-mouse pan,
//! and scroll-wheel zoom. All math is extracted into pure public functions for testability.

use crate::terrain_height::TerrainHeightField;
use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy_egui::EguiContexts;

/// Human-scale (1:1) orbit zoom-out limit in world units.
pub const BASE_MAX_DISTANCE: f32 = 500.0;
/// Human-scale (1:1) perspective far plane in world units.
pub const BASE_FAR: f32 = 1000.0;
/// Human-scale (1:1) zoom-in limit in world units (camera_focus_clip_20260716 FR-3).
pub const BASE_MIN_DISTANCE: f32 = 0.05;
/// Upper bound on the scale-driven zoom-out distance (keeps depth precision sane).
const MAX_SCALE_DISTANCE: f32 = 100_000.0;
/// Floor for the scale-driven zoom-in distance: 2× the fixed 0.01 near plane so
/// `near < min_distance` holds at every world scale (camera_focus_clip invariant).
const MIN_SCALED_MIN_DISTANCE: f32 = 0.02;
/// Fraction of the current distance a single scroll notch travels (log zoom).
const ZOOM_FRACTION: f32 = 0.1;
/// Camera body clearance above terrain in real meters (scaled to world units).
const GROUND_MARGIN_M: f32 = 2.0;
/// Ground-margin floor in world units: 3× the fixed near plane so the near
/// volume can never reach the ground the camera is clamped above.
const MIN_GROUND_MARGIN: f32 = 0.03;
/// Default easing rate (per second) for camera distance/focus targets.
const EASING_RATE: f32 = 10.0;
/// Snap threshold for eased values as a fraction of the viewing distance.
const EASING_SNAP_EPS: f32 = 1e-3;

/// Per-world-scale camera limits (world units per real meter). fe-ui writes this
/// when the active petal's terrain scale changes; see `src/AGENTS.md` §camera-scale.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraScaleSettings {
    pub world_scale: f32,
}

impl Default for CameraScaleSettings {
    fn default() -> Self {
        Self { world_scale: 1.0 }
    }
}

/// Orbit camera controller component.
///
/// Attach to a `Camera3d` entity to get orbit/pan/zoom controls.
/// The camera orbits around `focus` at `distance`, parameterised by `yaw` and `pitch`.
#[derive(Component)]
pub struct OrbitCameraController {
    /// The world-space point the camera orbits around.
    pub focus: Vec3,
    /// Distance from the focus point.
    pub distance: f32,
    /// Horizontal angle (radians) around the Y axis.
    pub yaw: f32,
    /// Vertical elevation angle (radians). Positive looks from above.
    pub pitch: f32,
    /// Mouse-motion-to-rotation multiplier.
    pub sensitivity: f32,
    /// Mouse-motion-to-pan multiplier (scaled by distance at runtime).
    pub pan_speed: f32,
    /// Scroll-to-distance multiplier.
    pub zoom_speed: f32,
    /// Closest the camera can get to the focus.
    pub min_distance: f32,
    /// Farthest the camera can get from the focus.
    pub max_distance: f32,
    /// Minimum pitch in radians (looking up from below).
    pub min_pitch: f32,
    /// Maximum pitch in radians (looking down from above).
    pub max_pitch: f32,
    /// WASD movement speed (world units per second).
    pub move_speed: f32,
    /// Distance the easing steers toward each frame (scroll writes this).
    pub target_distance: f32,
    /// Focus fly-to target; eased then cleared, canceled by manual pan/fly.
    pub target_focus: Option<Vec3>,
    /// Easing rate per second; `0.0` = raw mode (targets applied instantly).
    pub easing_rate: f32,
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        let limit = 89.0_f32.to_radians();
        Self {
            focus: Vec3::ZERO,
            distance: 8.66,
            yaw: std::f32::consts::FRAC_PI_4, // 45 degrees
            pitch: 0.4636,                    // ~26.6 degrees (camera at roughly (5,4,5))
            sensitivity: 0.005,
            pan_speed: 0.01,
            zoom_speed: 1.0,
            // camera_focus_clip_20260716 FR-3: was 0.5, clipped through compact
            // GLBs (e.g. duck models) before the near plane got there first.
            min_distance: BASE_MIN_DISTANCE,
            max_distance: 500.0,
            min_pitch: -limit,
            max_pitch: limit,
            move_speed: 10.0,
            target_distance: 8.66,
            target_focus: None,
            easing_rate: EASING_RATE,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure math functions -- no ECS required, easy to unit-test
// ---------------------------------------------------------------------------

/// Compute a camera [`Transform`] from orbit parameters.
///
/// The camera sits on a sphere of radius `distance` centred on `focus`,
/// parameterised by `yaw` (rotation around Y) and `pitch` (elevation).
pub fn compute_orbit_transform(focus: Vec3, distance: f32, yaw: f32, pitch: f32) -> Transform {
    let x = distance * pitch.cos() * yaw.sin();
    let y = distance * pitch.sin();
    let z = distance * pitch.cos() * yaw.cos();
    let position = focus + Vec3::new(x, y, z);
    Transform::from_translation(position).looking_at(focus, Vec3::Y)
}

/// Apply an orbit delta and return the new `(yaw, pitch)` with pitch clamped.
pub fn apply_orbit(
    yaw: f32,
    pitch: f32,
    delta_x: f32,
    delta_y: f32,
    sensitivity: f32,
    min_pitch: f32,
    max_pitch: f32,
) -> (f32, f32) {
    let new_yaw = yaw - delta_x * sensitivity;
    let new_pitch = (pitch + delta_y * sensitivity).clamp(min_pitch, max_pitch);
    (new_yaw, new_pitch)
}

/// Apply a zoom scroll and return the new distance, clamped to `[min, max]`.
pub fn apply_zoom(distance: f32, scroll_delta: f32, zoom_speed: f32, min: f32, max: f32) -> f32 {
    (distance - scroll_delta * zoom_speed).clamp(min, max)
}

/// Log-style zoom: the step is a fraction of the current distance, so a scroll
/// notch feels the same at 5 units or 50 000 units (needed for multi-scale space).
pub fn apply_zoom_proportional(
    distance: f32,
    scroll_delta: f32,
    zoom_speed: f32,
    min: f32,
    max: f32,
) -> f32 {
    let step = scroll_delta * zoom_speed * ZOOM_FRACTION * distance;
    (distance - step).clamp(min, max)
}

/// Orbit zoom-out limit for a world scale: smaller scales let you pull back
/// further so the whole (scaled-down) region fits; clamped to keep 1:1 unchanged.
pub fn scaled_max_distance(base_max_distance: f32, world_scale: f32) -> f32 {
    let s = if world_scale.is_finite() && world_scale > 0.0 {
        world_scale
    } else {
        1.0
    };
    (base_max_distance / s).clamp(base_max_distance, MAX_SCALE_DISTANCE)
}

/// Orbit zoom-in limit for a world scale: constant *real-world* approach across
/// scales (base × scale) until the near-plane floor (2× near) takes over; never
/// above base so 1:1 close-GLB inspection is unchanged.
pub fn scaled_min_distance(base_min_distance: f32, world_scale: f32) -> f32 {
    let s = if world_scale.is_finite() && world_scale > 0.0 {
        world_scale
    } else {
        1.0
    };
    (base_min_distance * s).clamp(MIN_SCALED_MIN_DISTANCE, base_min_distance)
}

/// Ground-clearance margin in world units: scaled real meters, floored above the
/// fixed near plane so it can never reach the ground; see AGENTS.md §terrain-height.
pub fn ground_margin(world_scale: f32) -> f32 {
    let s = if world_scale.is_finite() && world_scale > 0.0 {
        world_scale
    } else {
        1.0
    };
    (GROUND_MARGIN_M * s).max(MIN_GROUND_MARGIN)
}

/// Frame-rate-independent exponential approach with an explicit snap threshold
/// (callers pass a viewing-distance-relative epsilon so arrival never visibly
/// teleports regardless of coordinate magnitude); `rate <= 0` = raw mode.
pub fn exp_approach(current: f32, target: f32, rate: f32, dt: f32, snap_eps: f32) -> f32 {
    if rate <= 0.0 || !rate.is_finite() || !dt.is_finite() {
        return target;
    }
    let next = current + (target - current) * (1.0 - (-rate * dt).exp());
    if (target - next).abs() <= snap_eps.max(f32::EPSILON) {
        target
    } else {
        next
    }
}

/// Component-wise [`exp_approach`] for vectors.
pub fn exp_approach_vec3(current: Vec3, target: Vec3, rate: f32, dt: f32, snap_eps: f32) -> Vec3 {
    Vec3::new(
        exp_approach(current.x, target.x, rate, dt, snap_eps),
        exp_approach(current.y, target.y, rate, dt, snap_eps),
        exp_approach(current.z, target.z, rate, dt, snap_eps),
    )
}

/// Clamp a height to stay at least `margin` above `ground`; no ground data = no clamp.
pub fn clamp_above_ground(y: f32, ground: Option<f32>, margin: f32) -> f32 {
    match ground {
        Some(g) => y.max(g + margin),
        None => y,
    }
}

/// Minimum orbit pitch keeping a camera at `distance` from a focus at `focus_y`
/// at least `margin` above `ground`; `None` when every pitch already satisfies it.
pub fn min_pitch_above_ground(
    focus_y: f32,
    ground: f32,
    margin: f32,
    distance: f32,
) -> Option<f32> {
    if distance <= 0.0 || !distance.is_finite() {
        return None;
    }
    let ratio = (ground + margin - focus_y) / distance;
    if ratio <= -1.0 {
        return None;
    }
    Some(ratio.min(1.0).asin())
}

/// Perspective far plane that always clears the farthest zoom-out plus the scene.
pub fn scaled_far_plane(base_far: f32, max_distance: f32, _world_scale: f32) -> f32 {
    (max_distance * 2.0).max(base_far)
}

/// Apply a pan in the camera-local plane and return the new focus point.
///
/// Pan speed is scaled by `distance` so movement feels consistent regardless of zoom level.
pub fn apply_pan(
    focus: Vec3,
    camera_transform: &Transform,
    delta_x: f32,
    delta_y: f32,
    pan_speed: f32,
    distance: f32,
) -> Vec3 {
    let right = camera_transform.right();
    let up = camera_transform.up();
    let speed = pan_speed * distance;
    focus - right * delta_x * speed + up * delta_y * speed
}

// ---------------------------------------------------------------------------
// Plugin + systems
// ---------------------------------------------------------------------------

/// Plugin that spawns an orbit camera and drives it from mouse input.
pub struct CameraControllerPlugin;

impl Plugin for CameraControllerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraScaleSettings>();
        app.init_resource::<TerrainHeightField>();
        app.add_systems(Startup, spawn_orbit_camera);
        app.add_systems(Update, (orbit_camera_system, apply_camera_scale));
        app.add_systems(PostUpdate, deactivate_foreign_cameras);
    }
}

/// Deactivates cameras embedded in spawned glb scenes before extraction; see AGENTS.md §camera-scale.
fn deactivate_foreign_cameras(
    mut cameras: Query<&mut Camera, (Added<Camera>, Without<OrbitCameraController>)>,
) {
    let mut disabled = 0usize;
    for mut camera in cameras.iter_mut() {
        if camera.is_active {
            camera.is_active = false;
            disabled += 1;
        }
    }
    if disabled > 0 {
        bevy::log::info!("deactivated {disabled} non-orbit camera(s) from spawned scenes");
    }
}

/// Applies [`CameraScaleSettings`] to the orbit controller's zoom-out limit and
/// the perspective far plane whenever the scale changes; see AGENTS.md §camera-scale.
fn apply_camera_scale(
    settings: Res<CameraScaleSettings>,
    mut query: Query<(&mut OrbitCameraController, &mut Projection)>,
) {
    if !settings.is_changed() {
        return;
    }
    let scale = settings.world_scale;
    for (mut controller, mut projection) in query.iter_mut() {
        let max_distance = scaled_max_distance(BASE_MAX_DISTANCE, scale);
        let min_distance = scaled_min_distance(BASE_MIN_DISTANCE, scale);
        controller.max_distance = max_distance;
        controller.min_distance = min_distance;
        controller.distance = controller.distance.clamp(min_distance, max_distance);
        controller.target_distance = controller.target_distance.clamp(min_distance, max_distance);
        if let Projection::Perspective(persp) = projection.as_mut() {
            persp.far = scaled_far_plane(BASE_FAR, max_distance, scale);
        }
    }
}

/// Startup system: spawns a `Camera3d` with [`OrbitCameraController`].
fn spawn_orbit_camera(mut commands: Commands) {
    let controller = OrbitCameraController::default();
    let transform = compute_orbit_transform(
        controller.focus,
        controller.distance,
        controller.yaw,
        controller.pitch,
    );
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: std::f32::consts::FRAC_PI_4,
            // camera_focus_clip_20260716 FR-3: explicit near plane (was Bevy's
            // default 0.1, which clipped close zoom). Reverse-Z keeps depth
            // precision safe at this value; `apply_camera_scale` never touches it.
            near: 0.01,
            ..default()
        }),
        transform,
        controller,
    ));
}

/// Update system: reads mouse/keyboard input and applies orbit / pan / zoom / WASD,
/// then eases toward targets and clamps camera + focus above terrain.
///
/// Skips pointer-based controls when egui wants the pointer, and keyboard
/// controls when egui wants keyboard input (e.g. text field focused).
#[allow(clippy::too_many_arguments)]
fn orbit_camera_system(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut scroll: MessageReader<MouseWheel>,
    mut query: Query<(&mut OrbitCameraController, &mut Transform)>,
    mut egui_ctx: EguiContexts,
    time: Res<Time>,
    height_field: Option<Res<TerrainHeightField>>,
    scale_settings: Option<Res<CameraScaleSettings>>,
) {
    // Check egui input state.
    let (egui_wants_pointer, egui_wants_keyboard) = if let Ok(ctx) = egui_ctx.ctx_mut() {
        // is_using_pointer() instead of wants_pointer_input(): the CentralPanel
        // (3-D viewport) claims pointer ownership even when transparent, so
        // wants_pointer_input() is always true and blocks orbit/zoom/pan.
        // is_using_pointer() is only true when a UI widget is actively held.
        (ctx.is_using_pointer(), ctx.wants_keyboard_input())
    } else {
        (false, false)
    };

    // If egui owns the pointer, consume mouse events and skip mouse controls.
    if egui_wants_pointer {
        mouse_motion.read().count();
        scroll.read().count();
    }

    let (mut controller, mut transform) = match query.single_mut() {
        Ok(result) => result,
        Err(_) => return,
    };

    if !egui_wants_pointer {
        // Accumulate mouse motion for this frame.
        let mut delta = Vec2::ZERO;
        for ev in mouse_motion.read() {
            delta += ev.delta;
        }

        // Orbit: middle mouse button (no Shift).
        if mouse_button.pressed(MouseButton::Middle) && !keyboard.pressed(KeyCode::ShiftLeft) {
            let (new_yaw, new_pitch) = apply_orbit(
                controller.yaw,
                controller.pitch,
                delta.x,
                delta.y,
                controller.sensitivity,
                controller.min_pitch,
                controller.max_pitch,
            );
            controller.yaw = new_yaw;
            controller.pitch = new_pitch;
        }

        // Pan: Shift + middle mouse button. Manual pan cancels a pending fly-to.
        // Effective distance covers ground-clamp fallback frames where the camera
        // sits farther from the focus than `controller.distance`.
        if mouse_button.pressed(MouseButton::Middle) && keyboard.pressed(KeyCode::ShiftLeft) {
            controller.target_focus = None;
            let eff_distance = controller
                .distance
                .max((transform.translation - controller.focus).length());
            controller.focus = apply_pan(
                controller.focus,
                &transform,
                delta.x,
                delta.y,
                controller.pan_speed,
                eff_distance,
            );
        }

        // Zoom: scroll wheel. Normalise pixel vs. line units so the zoom
        // feels consistent across mice and trackpads. Proportional (log) zoom
        // keeps notches usable across scales (5 units .. 100 000 units).
        for ev in scroll.read() {
            let delta = match ev.unit {
                MouseScrollUnit::Line => ev.y,
                MouseScrollUnit::Pixel => ev.y * 0.05,
            };
            // Scroll steers the easing target so rapid notches accumulate cleanly.
            controller.target_distance = apply_zoom_proportional(
                controller.target_distance,
                delta,
                controller.zoom_speed,
                controller.min_distance,
                controller.max_distance,
            );
        }
    }

    // WASD + QE movement (only when egui doesn't want keyboard).
    if !egui_wants_keyboard {
        let dt = time.delta_secs();
        let speed = controller.move_speed * dt;

        // Forward/back direction projected onto XZ plane.
        let forward_xz = Vec3::new(-controller.yaw.sin(), 0.0, -controller.yaw.cos()).normalize();
        let right_xz = Vec3::new(-forward_xz.z, 0.0, forward_xz.x);

        let mut movement = Vec3::ZERO;

        if keyboard.pressed(KeyCode::KeyW) {
            movement += forward_xz;
        }
        // Skip S on the exact frame it is first pressed so the tool-shortcut
        // system (which fires on just_pressed) can claim it without the camera
        // simultaneously moving backward.
        if keyboard.pressed(KeyCode::KeyS) && !keyboard.just_pressed(KeyCode::KeyS) {
            movement -= forward_xz;
        }
        if keyboard.pressed(KeyCode::KeyD) {
            movement += right_xz;
        }
        if keyboard.pressed(KeyCode::KeyA) {
            movement -= right_xz;
        }
        if keyboard.pressed(KeyCode::KeyE) || keyboard.pressed(KeyCode::Space) {
            movement += Vec3::Y;
        }
        if keyboard.pressed(KeyCode::KeyQ) || keyboard.pressed(KeyCode::ControlLeft) {
            movement -= Vec3::Y;
        }

        // Shift to move faster.
        let sprint = if keyboard.pressed(KeyCode::ShiftLeft) {
            2.5
        } else {
            1.0
        };

        if movement.length_squared() > 0.0 {
            // Manual fly cancels a pending fly-to.
            controller.target_focus = None;
            controller.focus += movement.normalize() * speed * sprint;
        }
    }

    // Ease distance/focus toward their targets (dt-independent; raw at rate 0).
    // Snap epsilon is relative to the viewing distance, not coordinate magnitude,
    // so arrival never visibly teleports regardless of world position.
    let dt = time.delta_secs();
    let snap = EASING_SNAP_EPS * controller.distance.max(controller.min_distance);
    controller.distance = exp_approach(
        controller.distance,
        controller.target_distance,
        controller.easing_rate,
        dt,
        snap,
    );
    if let (Some(field), Some(t)) = (height_field.as_ref(), controller.target_focus) {
        // Floor an under-terrain fly-to target so the flight converges instead of stalling.
        let ty = clamp_above_ground(t.y, field.height_at(t.x, t.z), 0.0);
        if ty != t.y {
            controller.target_focus = Some(Vec3::new(t.x, ty, t.z));
        }
    }
    if let Some(target) = controller.target_focus {
        let next = exp_approach_vec3(controller.focus, target, controller.easing_rate, dt, snap);
        controller.focus = next;
        if next == target {
            controller.target_focus = None;
        }
    }

    // Ground avoidance (ux_interaction_hardening FR-5): the focus floors at the
    // terrain surface; the camera stays above ground primarily by clamping pitch
    // (state-space, so zoom/pan speeds never diverge from what is on screen),
    // with a transform-level raise as the steep-slope fallback; see AGENTS.md
    // §terrain-height.
    let world_scale = scale_settings.map(|s| s.world_scale).unwrap_or(1.0);
    let margin = ground_margin(world_scale);
    if let Some(field) = height_field.as_ref() {
        let focus_ground = field.height_at(controller.focus.x, controller.focus.z);
        controller.focus.y = clamp_above_ground(controller.focus.y, focus_ground, 0.0);
    }

    // Recompute transform from updated parameters.
    *transform = compute_orbit_transform(
        controller.focus,
        controller.distance,
        controller.yaw,
        controller.pitch,
    );

    if let Some(field) = height_field.as_ref() {
        if let Some(ground) = field.height_at(transform.translation.x, transform.translation.z) {
            if let Some(min_pitch) =
                min_pitch_above_ground(controller.focus.y, ground, margin, controller.distance)
            {
                if controller.pitch < min_pitch {
                    controller.pitch = min_pitch.min(controller.max_pitch);
                    *transform = compute_orbit_transform(
                        controller.focus,
                        controller.distance,
                        controller.yaw,
                        controller.pitch,
                    );
                }
            }
        }
        // Fallback: a steep slope can still leave the camera under neighboring ground.
        let cam_ground = field.height_at(transform.translation.x, transform.translation.z);
        let clamped_y = clamp_above_ground(transform.translation.y, cam_ground, margin);
        if clamped_y > transform.translation.y {
            transform.translation.y = clamped_y;
            let focus = controller.focus;
            transform.look_at(focus, Vec3::Y);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    const EPSILON: f32 = 1e-4;

    #[test]
    fn default_controller_values_are_sane() {
        let c = OrbitCameraController::default();
        assert!(c.distance > 0.0);
        assert!(c.min_distance > 0.0);
        assert!(c.max_distance > c.min_distance);
        assert!(c.min_pitch < 0.0);
        assert!(c.max_pitch > 0.0);
        assert!(c.sensitivity > 0.0);
        assert!(c.pan_speed > 0.0);
        assert!(c.zoom_speed > 0.0);
    }

    #[test]
    fn orbit_transform_distance_matches() {
        let focus = Vec3::ZERO;
        let distance = 10.0;
        let t = compute_orbit_transform(focus, distance, 0.0, 0.0);
        let actual_dist = (t.translation - focus).length();
        assert!(
            (actual_dist - distance).abs() < EPSILON,
            "expected distance {distance}, got {actual_dist}"
        );
    }

    #[test]
    fn orbit_transform_looks_at_focus() {
        let focus = Vec3::new(1.0, 2.0, 3.0);
        let t = compute_orbit_transform(focus, 5.0, FRAC_PI_4, 0.3);
        // Forward direction should roughly point from position toward focus.
        let to_focus = (focus - t.translation).normalize();
        let forward = t.forward().as_vec3();
        let dot = to_focus.dot(forward);
        assert!(
            dot > 0.99,
            "camera should face focus, dot product was {dot}"
        );
    }

    #[test]
    fn orbit_transform_at_zero_yaw_zero_pitch() {
        // With yaw=0 and pitch=0, the camera should be on the +Z axis.
        let t = compute_orbit_transform(Vec3::ZERO, 10.0, 0.0, 0.0);
        assert!((t.translation.x).abs() < EPSILON);
        assert!((t.translation.y).abs() < EPSILON);
        assert!((t.translation.z - 10.0).abs() < EPSILON);
    }

    #[test]
    fn apply_orbit_clamps_pitch_at_max() {
        let max = 89.0_f32.to_radians();
        let (_, pitch) = apply_orbit(0.0, max - 0.01, 0.0, -10000.0, 1.0, -max, max);
        assert!(
            pitch >= -max - EPSILON && pitch <= max + EPSILON,
            "pitch {pitch} should be clamped to [{}, {}]",
            -max,
            max
        );
    }

    #[test]
    fn apply_orbit_clamps_pitch_at_min() {
        let max = 89.0_f32.to_radians();
        let (_, pitch) = apply_orbit(0.0, -max + 0.01, 0.0, 10000.0, 1.0, -max, max);
        assert!(
            pitch >= -max - EPSILON && pitch <= max + EPSILON,
            "pitch {pitch} should be clamped"
        );
    }

    #[test]
    fn apply_orbit_yaw_changes() {
        let (new_yaw, _) = apply_orbit(1.0, 0.0, 100.0, 0.0, 0.01, -1.5, 1.5);
        assert!(
            (new_yaw - (1.0 - 100.0 * 0.01)).abs() < EPSILON,
            "yaw should decrease by delta_x * sensitivity"
        );
    }

    #[test]
    fn apply_zoom_clamps_min() {
        let d = apply_zoom(1.0, -1000.0, 1.0, 0.5, 100.0);
        assert!(
            (d - 100.0).abs() < EPSILON,
            "zoom should clamp at max, got {d}"
        );
    }

    #[test]
    fn apply_zoom_clamps_max() {
        let d = apply_zoom(1.0, 1000.0, 1.0, 0.5, 100.0);
        assert!(
            (d - 0.5).abs() < EPSILON,
            "zoom should clamp at min, got {d}"
        );
    }

    #[test]
    fn apply_zoom_normal() {
        let d = apply_zoom(10.0, 2.0, 1.0, 0.5, 100.0);
        assert!(
            (d - 8.0).abs() < EPSILON,
            "distance should decrease by scroll * speed, got {d}"
        );
    }

    #[test]
    fn apply_pan_moves_focus() {
        let t = compute_orbit_transform(Vec3::ZERO, 10.0, 0.0, 0.0);
        let new_focus = apply_pan(Vec3::ZERO, &t, 1.0, 0.0, 1.0, 10.0);
        // Panning right (positive delta_x) should subtract the right vector,
        // so focus moves to the left from the camera's perspective.
        assert!(
            new_focus.length() > EPSILON,
            "focus should have moved, got {new_focus}"
        );
    }

    #[test]
    fn apply_pan_scales_with_distance() {
        let t = compute_orbit_transform(Vec3::ZERO, 10.0, 0.0, 0.0);
        let near = apply_pan(Vec3::ZERO, &t, 1.0, 0.0, 1.0, 1.0);
        let far = apply_pan(Vec3::ZERO, &t, 1.0, 0.0, 1.0, 10.0);
        assert!(
            far.length() > near.length(),
            "pan at greater distance should produce larger movement"
        );
    }

    #[test]
    fn scaled_max_distance_unchanged_at_unit_scale() {
        assert!((scaled_max_distance(BASE_MAX_DISTANCE, 1.0) - BASE_MAX_DISTANCE).abs() < EPSILON);
    }

    #[test]
    fn scaled_max_distance_grows_as_scale_shrinks() {
        let human = scaled_max_distance(BASE_MAX_DISTANCE, 1.0);
        let region = scaled_max_distance(BASE_MAX_DISTANCE, 0.001);
        assert!(
            region > human,
            "region scale must allow pulling back further"
        );
        // Clamped so depth precision stays sane.
        assert!(region <= MAX_SCALE_DISTANCE + EPSILON);
    }

    #[test]
    fn scaled_max_distance_guards_bad_scale() {
        assert!((scaled_max_distance(BASE_MAX_DISTANCE, 0.0) - BASE_MAX_DISTANCE).abs() < EPSILON);
        assert!(
            (scaled_max_distance(BASE_MAX_DISTANCE, f32::NAN) - BASE_MAX_DISTANCE).abs() < EPSILON
        );
    }

    #[test]
    fn scaled_far_plane_unchanged_at_unit_scale() {
        let max_d = scaled_max_distance(BASE_MAX_DISTANCE, 1.0);
        assert!((scaled_far_plane(BASE_FAR, max_d, 1.0) - BASE_FAR).abs() < EPSILON);
    }

    #[test]
    fn scaled_far_plane_clears_zoom_out_at_region_scale() {
        let max_d = scaled_max_distance(BASE_MAX_DISTANCE, 0.001);
        let far = scaled_far_plane(BASE_FAR, max_d, 0.001);
        assert!(
            far > max_d,
            "far plane must clear the farthest zoom-out distance"
        );
        assert!(far > BASE_FAR);
    }

    #[test]
    fn apply_zoom_proportional_is_distance_relative() {
        // Same scroll notch moves proportionally more when far out.
        let near = 10.0 - apply_zoom_proportional(10.0, 1.0, 1.0, 0.1, 1e6);
        let far = 10_000.0 - apply_zoom_proportional(10_000.0, 1.0, 1.0, 0.1, 1e6);
        assert!(far > near, "log zoom step scales with current distance");
    }

    #[test]
    fn apply_zoom_proportional_clamps() {
        assert!((apply_zoom_proportional(100.0, 1e6, 1.0, 0.5, 1000.0) - 0.5).abs() < EPSILON);
        assert!((apply_zoom_proportional(100.0, -1e6, 1.0, 0.5, 1000.0) - 1000.0).abs() < EPSILON);
    }

    #[test]
    fn camera_scale_settings_default_is_unit() {
        assert_eq!(CameraScaleSettings::default().world_scale, 1.0);
    }

    #[test]
    fn default_min_distance_allows_close_zoom_without_clipping() {
        // camera_focus_clip_20260716 FR-3: must be small enough to orbit close
        // on a compact GLB without the near plane clipping through it.
        let c = OrbitCameraController::default();
        assert!((c.min_distance - 0.05).abs() < EPSILON);
    }

    #[test]
    fn default_target_distance_matches_distance() {
        let c = OrbitCameraController::default();
        assert!((c.target_distance - c.distance).abs() < EPSILON);
        assert!(c.target_focus.is_none());
        assert!(c.easing_rate > 0.0);
    }

    #[test]
    fn scaled_min_distance_unchanged_at_unit_scale() {
        assert!((scaled_min_distance(BASE_MIN_DISTANCE, 1.0) - BASE_MIN_DISTANCE).abs() < EPSILON);
    }

    #[test]
    fn scaled_min_distance_keeps_constant_real_approach_until_near_floor() {
        // Moderate scale-down: 0.05 world units at 1:1 becomes 0.05 * scale,
        // i.e. the same real-world closeness.
        let d = scaled_min_distance(BASE_MIN_DISTANCE, 0.5);
        assert!((d - 0.025).abs() < 1e-6);
        // Region scales floor at 2x the fixed 0.01 near plane so the near plane
        // can never cross the zoom-in limit (camera_focus_clip invariant).
        assert!((scaled_min_distance(BASE_MIN_DISTANCE, 0.01) - 0.02).abs() < 1e-6);
        assert!(scaled_min_distance(BASE_MIN_DISTANCE, 1e-6) >= 0.02 - EPSILON);
    }

    #[test]
    fn scaled_min_distance_always_clears_near_plane() {
        for scale in [1e-6, 1e-3, 0.01, 0.2, 1.0, 10.0, f32::NAN, 0.0] {
            let d = scaled_min_distance(BASE_MIN_DISTANCE, scale);
            assert!(
                d > 0.01 + EPSILON,
                "min {d} must exceed near 0.01 at scale {scale}"
            );
        }
    }

    #[test]
    fn ground_margin_scales_but_floors_above_near() {
        assert!((ground_margin(1.0) - 2.0).abs() < EPSILON);
        // Region scale: margin floors at 3x near instead of shrinking under it.
        assert!((ground_margin(0.001) - 0.03).abs() < EPSILON);
        assert!((ground_margin(f32::NAN) - 2.0).abs() < EPSILON);
        for scale in [1e-6, 0.01, 1.0, 100.0] {
            assert!(ground_margin(scale) > 0.01 + EPSILON);
        }
    }

    #[test]
    fn min_pitch_above_ground_grazes_terrain() {
        // Flat ground at focus height, margin 2, distance 10: sin(pitch) >= 0.2.
        let p = min_pitch_above_ground(50.0, 50.0, 2.0, 10.0).unwrap();
        assert!((p - 0.2_f32.asin()).abs() < 1e-5);
        // Deep valley below: every pitch is fine.
        assert!(min_pitch_above_ground(50.0, 0.0, 2.0, 10.0).is_none());
        // Ground unreachably high: pitch caps at straight-up.
        let p = min_pitch_above_ground(0.0, 100.0, 2.0, 10.0).unwrap();
        assert!((p - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        // Degenerate distance: no constraint.
        assert!(min_pitch_above_ground(0.0, 10.0, 2.0, 0.0).is_none());
    }

    #[test]
    fn scaled_min_distance_never_exceeds_base() {
        // Magnified worlds must not regress close-GLB inspection.
        assert!(scaled_min_distance(BASE_MIN_DISTANCE, 10.0) <= BASE_MIN_DISTANCE + EPSILON);
        // Bad scales fall back to base.
        assert!((scaled_min_distance(BASE_MIN_DISTANCE, 0.0) - BASE_MIN_DISTANCE).abs() < EPSILON);
        assert!(
            (scaled_min_distance(BASE_MIN_DISTANCE, f32::NAN) - BASE_MIN_DISTANCE).abs() < EPSILON
        );
    }

    #[test]
    fn exp_approach_converges_without_overshoot() {
        let mut x = 0.0;
        let mut prev_gap = 100.0_f32;
        for _ in 0..200 {
            x = exp_approach(x, 100.0, 10.0, 1.0 / 60.0, 0.005);
            let gap = (100.0_f32 - x).abs();
            assert!(gap <= prev_gap + EPSILON, "must approach monotonically");
            assert!(x <= 100.0 + EPSILON, "must not overshoot");
            prev_gap = gap;
        }
        assert!((x - 100.0).abs() < EPSILON, "must snap to target, got {x}");
    }

    #[test]
    fn exp_approach_is_dt_independent() {
        // Two half-steps equal one full step (exponential decay composes).
        let one = exp_approach(0.0, 1000.0, 4.0, 0.1, 0.0);
        let half = exp_approach(0.0, 1000.0, 4.0, 0.05, 0.0);
        let two = exp_approach(half, 1000.0, 4.0, 0.05, 0.0);
        assert!(
            (one - two).abs() < 1e-2,
            "composed steps must match: {one} vs {two}"
        );
    }

    #[test]
    fn exp_approach_rate_zero_is_raw_mode() {
        assert_eq!(exp_approach(0.0, 42.0, 0.0, 0.016, 0.005), 42.0);
    }

    #[test]
    fn exp_approach_never_snaps_farther_than_eps() {
        // Large-coordinate target (a node 50 km from the projection origin) with a
        // viewing-distance-relative epsilon: no premature multi-unit teleport.
        let eps = 0.005; // EASING_SNAP_EPS * distance 5
        let mut x = 0.0_f32;
        // Bounded: at 50 km the f32 ULP (~0.0039) exceeds half the final smooth
        // step, so the approach stalls a hair short of the snap band and the snap
        // may never fire -- that is still the property under test (no early snap).
        for _ in 0..100_000 {
            let next = exp_approach(x, 50_000.0, 10.0, 1.0 / 60.0, eps);
            if next == 50_000.0 {
                // The arrival hop is the smooth step plus at most eps.
                let smooth = x + (50_000.0 - x) * (1.0 - (-10.0_f32 / 60.0).exp());
                assert!(
                    (50_000.0 - smooth).abs() <= eps + EPSILON,
                    "snap fired while {} units short",
                    50_000.0 - smooth
                );
                return;
            }
            if next == x {
                return; // f32 step underflowed before the snap band; no premature teleport.
            }
            x = next;
        }
        panic!("exp_approach neither converged nor stalled within the iteration budget");
    }

    #[test]
    fn exp_approach_vec3_reaches_target() {
        let mut v = Vec3::ZERO;
        let target = Vec3::new(10.0, -5.0, 3.0);
        for _ in 0..300 {
            v = exp_approach_vec3(v, target, 10.0, 1.0 / 60.0, 0.005);
        }
        assert_eq!(v, target, "componentwise snap must reach the exact target");
    }

    #[test]
    fn clamp_above_ground_raises_only_below() {
        assert!((clamp_above_ground(1.0, Some(5.0), 2.0) - 7.0).abs() < EPSILON);
        assert!((clamp_above_ground(10.0, Some(5.0), 2.0) - 10.0).abs() < EPSILON);
        assert!((clamp_above_ground(-3.0, None, 2.0) - (-3.0)).abs() < EPSILON);
    }

    #[test]
    fn orbit_transform_then_ground_clamp_keeps_camera_above_terrain() {
        // Shallow pitch over 50-unit terrain: raw orbit puts the camera below it.
        let t = compute_orbit_transform(Vec3::new(0.0, 50.0, 0.0), 30.0, 0.0, -0.5);
        assert!(t.translation.y < 50.0, "precondition: camera below terrain");
        let clamped = clamp_above_ground(t.translation.y, Some(50.0), 2.0);
        assert!((clamped - 52.0).abs() < EPSILON);
    }
}
