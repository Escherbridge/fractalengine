//! Shared transform constraints; see AGENTS.md §transform-snapping.

use bevy::prelude::*;

use crate::gimbal::GimbalAxis;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TransformAxisLock {
    #[default]
    Any,
    X,
    Y,
    Z,
}

impl TransformAxisLock {
    pub fn allows(self, axis: GimbalAxis) -> bool {
        matches!(self, Self::Any)
            || matches!((self, axis), (Self::X, GimbalAxis::X) | (Self::Y, GimbalAxis::Y) | (Self::Z, GimbalAxis::Z))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolSnapSettings {
    pub axis: TransformAxisLock,
    pub grid_enabled: bool,
    pub grid_m: f32,
    pub angle_enabled: bool,
    pub angle_degrees: f32,
    pub scale_enabled: bool,
    pub scale_step: f32,
    pub uniform_scale: bool,
}

impl Default for ToolSnapSettings {
    fn default() -> Self {
        Self {
            axis: TransformAxisLock::Any,
            grid_enabled: false,
            grid_m: 1.0,
            angle_enabled: false,
            angle_degrees: 15.0,
            scale_enabled: false,
            scale_step: 0.1,
            uniform_scale: false,
        }
    }
}

pub(crate) fn snap_scalar(value: f32, step: f32) -> f32 {
    if !value.is_finite() || !step.is_finite() || step <= 0.0 {
        return value;
    }
    let snapped = (value / step).round() * step;
    if snapped.is_finite() { snapped } else { value }
}

impl ToolSnapSettings {
    pub(crate) fn move_position(self, start: Vec3, axis: GimbalAxis, distance: f32, world_scale: f64) -> Vec3 {
        let mut result = start;
        if !self.axis.allows(axis) || !distance.is_finite() {
            return result;
        }
        let index = axis_index(axis);
        let value = start[index] + distance;
        result[index] = if self.grid_enabled {
            snap_scalar(value, crate::geometry::meters_to_world(self.grid_m, world_scale))
        } else { value };
        result
    }

    pub(crate) fn rotation_angle(self, angle: f32) -> f32 {
        if self.angle_enabled { snap_scalar(angle, self.angle_degrees.to_radians()) } else { angle }
    }

    pub(crate) fn scale_value(self, start: Vec3, axis: GimbalAxis, factor: f32) -> Vec3 {
        if !self.axis.allows(axis) || !factor.is_finite() { return start; }
        let factor = factor.max(0.001);
        if self.uniform_scale {
            let factor = if self.scale_enabled { snap_scalar(factor, self.scale_step).max(0.001) } else { factor };
            return start * factor;
        }
        let index = axis_index(axis);
        let value = start[index] * factor;
        let value = if self.scale_enabled { snap_scalar(value, self.scale_step) } else { value };
        let mut result = start;
        result[index] = if value == 0.0 { 0.001_f32.copysign(start[index]) } else { value };
        result
    }
}

fn axis_index(axis: GimbalAxis) -> usize {
    match axis { GimbalAxis::X => 0, GimbalAxis::Y => 1, GimbalAxis::Z => 2 }
}
