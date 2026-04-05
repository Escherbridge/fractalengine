use bevy::gizmos::config::{DefaultGizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;

/// Plugin that renders an infinite-style ground grid at Y=0.
///
/// Uses Bevy's gizmo system to draw minor lines (1-unit spacing) and
/// major lines (10-unit spacing) centered on the camera's XZ position,
/// creating the illusion of an infinite grid.
pub struct GridPlugin;

impl Plugin for GridPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GridConfig>()
            .add_systems(Startup, configure_grid_gizmos)
            .add_systems(Update, draw_grid);
    }
}

/// Depth bias applied to the default gizmo config for grid / axis gizmos.
/// Set to 0.0 for default depth-testing behaviour — the grid renders behind
/// opaque geometry correctly. A value of -1.0 was too aggressive and caused
/// grid lines to paint over imported models (duck artefact).
pub const GRID_GIZMO_DEPTH_BIAS: f32 = 0.0;

/// Startup system: configure the default gizmo group so grid lines render
/// in front of opaque meshes. Runs after the Bevy gizmo plugin has
/// populated `GizmoConfigStore`.
fn configure_grid_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    cfg.depth_bias = GRID_GIZMO_DEPTH_BIAS;
}

/// Grid configuration resource for customization.
///
/// Controls the density, extent, coloring, and fade behavior of the
/// ground-plane grid lines drawn each frame via gizmos.
#[derive(Resource)]
pub struct GridConfig {
    /// Number of minor grid cells in each direction from center.
    pub half_extent: i32,
    /// Spacing between minor grid lines (world units).
    pub minor_spacing: f32,
    /// Every Nth minor line is drawn as a major line.
    pub major_every: i32,
    /// Minor line color (including base alpha).
    pub minor_color: Color,
    /// Major line color (including base alpha).
    pub major_color: Color,
    /// Maximum render distance — grid lines fade to invisible beyond this.
    pub fade_distance: f32,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            half_extent: 100,
            minor_spacing: 1.0,
            major_every: 10,
            minor_color: Color::srgba(1.0, 1.0, 1.0, 0.05),
            major_color: Color::srgba(1.0, 1.0, 1.0, 0.15),
            fade_distance: 80.0,
        }
    }
}

/// Snaps a value to the nearest multiple of `spacing`.
fn snap_to_grid(value: f32, spacing: f32) -> f32 {
    (value / spacing).round() * spacing
}

/// Computes a fade-out multiplier based on distance from the camera.
///
/// Returns a value in `[0.0, 1.0]` where 0 means fully faded (invisible)
/// and 1 means fully visible.
fn fade_factor(distance: f32, fade_distance: f32) -> f32 {
    1.0 - (distance / fade_distance).clamp(0.0, 1.0)
}

/// System that draws the ground grid each frame using gizmos.
///
/// The grid is centered on the camera's XZ position (snapped to the
/// minor grid spacing) so it appears infinite as the camera moves.
fn draw_grid(
    mut gizmos: Gizmos,
    config: Res<GridConfig>,
    camera_query: Query<&Transform, With<Camera3d>>,
) {
    let camera_transform = match camera_query.single() {
        Ok(t) => t,
        Err(_) => return,
    };

    let cam_x = camera_transform.translation.x;
    let cam_z = camera_transform.translation.z;
    let snap_x = snap_to_grid(cam_x, config.minor_spacing);
    let snap_z = snap_to_grid(cam_z, config.minor_spacing);

    let extent = config.half_extent as f32 * config.minor_spacing;

    // Lines parallel to Z axis (varying X position)
    for i in -config.half_extent..=config.half_extent {
        let x = snap_x + i as f32 * config.minor_spacing;
        let is_major = (i % config.major_every) == 0;
        let base_color = if is_major {
            config.major_color
        } else {
            config.minor_color
        };

        let alpha = fade_factor((x - cam_x).abs(), config.fade_distance);
        if alpha <= 0.0 {
            continue;
        }

        let color = base_color.with_alpha(base_color.alpha() * alpha);
        gizmos.line(
            Vec3::new(x, 0.0, snap_z - extent),
            Vec3::new(x, 0.0, snap_z + extent),
            color,
        );
    }

    // Lines parallel to X axis (varying Z position)
    for i in -config.half_extent..=config.half_extent {
        let z = snap_z + i as f32 * config.minor_spacing;
        let is_major = (i % config.major_every) == 0;
        let base_color = if is_major {
            config.major_color
        } else {
            config.minor_color
        };

        let alpha = fade_factor((z - cam_z).abs(), config.fade_distance);
        if alpha <= 0.0 {
            continue;
        }

        let color = base_color.with_alpha(base_color.alpha() * alpha);
        gizmos.line(
            Vec3::new(snap_x - extent, 0.0, z),
            Vec3::new(snap_x + extent, 0.0, z),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = GridConfig::default();
        assert_eq!(cfg.half_extent, 100);
        assert!((cfg.minor_spacing - 1.0).abs() < f32::EPSILON);
        assert_eq!(cfg.major_every, 10);
        assert!((cfg.fade_distance - 80.0).abs() < f32::EPSILON);
        assert!((cfg.minor_color.alpha() - 0.05).abs() < 0.001);
        assert!((cfg.major_color.alpha() - 0.15).abs() < 0.001);
    }

    #[test]
    fn snap_to_grid_rounds_correctly() {
        // Exact multiples stay unchanged
        assert!((snap_to_grid(5.0, 1.0) - 5.0).abs() < f32::EPSILON);
        assert!((snap_to_grid(10.0, 2.0) - 10.0).abs() < f32::EPSILON);

        // Values snap to nearest multiple
        assert!((snap_to_grid(5.3, 1.0) - 5.0).abs() < f32::EPSILON);
        assert!((snap_to_grid(5.7, 1.0) - 6.0).abs() < f32::EPSILON);
        assert!((snap_to_grid(5.5, 1.0) - 6.0).abs() < f32::EPSILON);

        // Negative values
        assert!((snap_to_grid(-3.2, 1.0) - (-3.0)).abs() < f32::EPSILON);
        assert!((snap_to_grid(-3.8, 1.0) - (-4.0)).abs() < f32::EPSILON);

        // Non-unit spacing
        assert!((snap_to_grid(3.3, 2.5) - 2.5).abs() < f32::EPSILON);
        assert!((snap_to_grid(4.0, 2.5) - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn grid_gizmo_depth_bias_does_not_overdraw_models() {
        // Bias must be 0.0 (default depth testing). A negative value caused
        // grid lines to render on top of (overdraw) imported GLB models.
        assert!(
            GRID_GIZMO_DEPTH_BIAS.abs() < 1.5,
            "GRID_GIZMO_DEPTH_BIAS must not be aggressively negative, got {GRID_GIZMO_DEPTH_BIAS}"
        );
        assert_eq!(
            GRID_GIZMO_DEPTH_BIAS, 0.0,
            "GRID_GIZMO_DEPTH_BIAS should be exactly 0.0, got {GRID_GIZMO_DEPTH_BIAS}"
        );
    }

    #[test]
    fn fade_factor_boundaries() {
        // At camera position: full visibility
        assert!((fade_factor(0.0, 80.0) - 1.0).abs() < f32::EPSILON);

        // At fade distance: invisible
        assert!((fade_factor(80.0, 80.0) - 0.0).abs() < f32::EPSILON);

        // Beyond fade distance: clamped to 0
        assert!((fade_factor(200.0, 80.0) - 0.0).abs() < f32::EPSILON);

        // Halfway: 0.5
        assert!((fade_factor(40.0, 80.0) - 0.5).abs() < f32::EPSILON);
    }
}
