use bevy::prelude::*;

use crate::axis_gizmo::AxisGizmoPlugin;
use crate::camera::CameraControllerPlugin;
use crate::grid::GridPlugin;

/// Composite plugin that assembles the full 3D viewport foundation.
///
/// Adds the orbit camera controller, infinite ground grid, environment
/// lighting, and XYZ axis orientation indicator.
pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((CameraControllerPlugin, GridPlugin, AxisGizmoPlugin));
    }
}
