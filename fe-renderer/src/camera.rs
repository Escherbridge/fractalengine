//! Orbit camera controller for the 3D viewport.
//!
//! Provides a Blender-style orbit camera with middle-mouse orbit, shift+middle-mouse pan,
//! and scroll-wheel zoom. All math is extracted into pure public functions for testability.

use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::EguiContexts;

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
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        let limit = 89.0_f32.to_radians();
        Self {
            focus: Vec3::ZERO,
            distance: 8.66,
            yaw: 0.7854,   // ~45 degrees
            pitch: 0.4636, // ~26.6 degrees (camera at roughly (5,4,5))
            sensitivity: 0.005,
            pan_speed: 0.01,
            zoom_speed: 1.0,
            min_distance: 0.5,
            max_distance: 500.0,
            min_pitch: -limit,
            max_pitch: limit,
            move_speed: 10.0,
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
        app.add_systems(Startup, spawn_orbit_camera);
        app.add_systems(Update, orbit_camera_system);
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
            ..default()
        }),
        transform,
        controller,
    ));
}

/// Update system: reads mouse/keyboard input and applies orbit / pan / zoom / WASD.
///
/// Skips pointer-based controls when egui wants the pointer, and keyboard
/// controls when egui wants keyboard input (e.g. text field focused).
fn orbit_camera_system(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut scroll: MessageReader<MouseWheel>,
    mut query: Query<(&mut OrbitCameraController, &mut Transform)>,
    mut egui_ctx: EguiContexts,
    time: Res<Time>,
) {
    // Check egui input state.
    let (egui_wants_pointer, egui_wants_keyboard) =
        if let Ok(ctx) = egui_ctx.ctx_mut() {
            (ctx.wants_pointer_input(), ctx.wants_keyboard_input())
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

        // Pan: Shift + middle mouse button.
        if mouse_button.pressed(MouseButton::Middle) && keyboard.pressed(KeyCode::ShiftLeft) {
            controller.focus = apply_pan(
                controller.focus,
                &transform,
                delta.x,
                delta.y,
                controller.pan_speed,
                controller.distance,
            );
        }

        // Zoom: scroll wheel.
        for ev in scroll.read() {
            controller.distance = apply_zoom(
                controller.distance,
                ev.y,
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
        let right_xz = Vec3::new(forward_xz.z, 0.0, -forward_xz.x);

        let mut movement = Vec3::ZERO;

        if keyboard.pressed(KeyCode::KeyW) {
            movement += forward_xz;
        }
        if keyboard.pressed(KeyCode::KeyS) {
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
            controller.focus += movement.normalize() * speed * sprint;
        }
    }

    // Recompute transform from updated parameters.
    *transform = compute_orbit_transform(
        controller.focus,
        controller.distance,
        controller.yaw,
        controller.pitch,
    );
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
}
