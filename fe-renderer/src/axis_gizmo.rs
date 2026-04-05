use bevy::prelude::*;

/// Plugin that sets up the 3D environment (clear color, lighting) and
/// draws an XYZ axis orientation indicator.
pub struct AxisGizmoPlugin;

impl Plugin for AxisGizmoPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.10)));
        app.add_systems(Startup, setup_environment_lighting);
        app.add_systems(Update, draw_axis_gizmo);
    }
}

/// Spawns ambient light and a directional light for the 3D scene.
fn setup_environment_lighting(mut commands: Commands) {
    // Ambient light for base visibility
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 200.0,
        ..default()
    });

    // Directional light for depth/shadow perception
    commands.spawn((
        DirectionalLight {
            illuminance: 5000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.5, 0.0)),
    ));
}

/// Axis colors matching Blender convention.
const AXIS_X_COLOR: Color = Color::srgb(0.9, 0.2, 0.2); // Red
const AXIS_Y_COLOR: Color = Color::srgb(0.2, 0.9, 0.2); // Green
const AXIS_Z_COLOR: Color = Color::srgb(0.3, 0.4, 0.9); // Blue

/// Draws an XYZ axis orientation gizmo.
///
/// The gizmo is positioned in world space near the camera to simulate
/// a fixed screen-corner position. It rotates to show the current
/// camera orientation relative to world axes.
fn draw_axis_gizmo(
    mut gizmos: Gizmos,
    camera_query: Query<(&Transform, &Projection), With<Camera3d>>,
) {
    let (camera_transform, _projection) = match camera_query.single() {
        Ok(result) => result,
        Err(_) => return,
    };

    // Place the gizmo in world space, offset from camera.
    // Position it slightly in front of the camera, offset to bottom-left.
    let forward = camera_transform.forward().as_vec3();
    let right = camera_transform.right().as_vec3();
    let up = camera_transform.up().as_vec3();

    // Gizmo center: in front of camera, offset to bottom-left
    let gizmo_center = camera_transform.translation + forward * 3.0 // in front of camera
        - right * 1.8  // shift left
        - up * 1.2; // shift down

    let axis_length = 0.25;

    // Draw axes: X (red), Y (green), Z (blue)
    gizmos.line(
        gizmo_center,
        gizmo_center + Vec3::X * axis_length,
        AXIS_X_COLOR,
    );
    gizmos.line(
        gizmo_center,
        gizmo_center + Vec3::Y * axis_length,
        AXIS_Y_COLOR,
    );
    gizmos.line(
        gizmo_center,
        gizmo_center + Vec3::Z * axis_length,
        AXIS_Z_COLOR,
    );

    // Draw small arrow tips (thicker end segments)
    let tip_len = 0.06;
    let tip_offset = axis_length - tip_len;

    // X axis tip
    let x_tip_start = gizmo_center + Vec3::X * tip_offset;
    let x_tip_end = gizmo_center + Vec3::X * axis_length;
    gizmos.line(x_tip_start + Vec3::Y * 0.02, x_tip_end, AXIS_X_COLOR);
    gizmos.line(x_tip_start - Vec3::Y * 0.02, x_tip_end, AXIS_X_COLOR);

    // Y axis tip
    let y_tip_start = gizmo_center + Vec3::Y * tip_offset;
    let y_tip_end = gizmo_center + Vec3::Y * axis_length;
    gizmos.line(y_tip_start + Vec3::X * 0.02, y_tip_end, AXIS_Y_COLOR);
    gizmos.line(y_tip_start - Vec3::X * 0.02, y_tip_end, AXIS_Y_COLOR);

    // Z axis tip
    let z_tip_start = gizmo_center + Vec3::Z * tip_offset;
    let z_tip_end = gizmo_center + Vec3::Z * axis_length;
    gizmos.line(z_tip_start + Vec3::Y * 0.02, z_tip_end, AXIS_Z_COLOR);
    gizmos.line(z_tip_start - Vec3::Y * 0.02, z_tip_end, AXIS_Z_COLOR);

    // Draw axis labels using small crosshairs at endpoints.
    // Gizmos don't support text, so we mark endpoints distinctly.
    let label_size = 0.03;

    // X label - small cross
    let x_end = gizmo_center + Vec3::X * (axis_length + 0.05);
    gizmos.line(
        x_end - Vec3::Y * label_size,
        x_end + Vec3::Y * label_size,
        AXIS_X_COLOR,
    );
    gizmos.line(
        x_end - Vec3::Z * label_size,
        x_end + Vec3::Z * label_size,
        AXIS_X_COLOR,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_colors_are_distinct() {
        // Ensure each axis has a unique color so they are visually distinguishable
        let colors = [AXIS_X_COLOR, AXIS_Y_COLOR, AXIS_Z_COLOR];
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(
                    colors[i], colors[j],
                    "Axis colors at index {} and {} must be distinct",
                    i, j
                );
            }
        }
    }

    #[test]
    fn clear_color_matches_spec() {
        let expected = Color::srgb(0.08, 0.08, 0.10);
        // Build a minimal app and check the inserted ClearColor resource
        // (don't call update — gizmo systems need DefaultPlugins resources)
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AxisGizmoPlugin);

        let clear_color = app.world().resource::<ClearColor>();
        assert_eq!(clear_color.0, expected);
    }
}
