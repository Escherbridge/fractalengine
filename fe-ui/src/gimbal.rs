//! Gimbal gizmo rendering — pure visual, no state.
//!
//! All interaction logic and selection state lives in [`crate::node_manager`].
//! This module only knows how to *draw* the coloured axis handles for a given
//! entity, active tool, and optionally highlighted axis.

use bevy::gizmos::config::{GizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;

use crate::panels::Tool;

// ---------------------------------------------------------------------------
// Custom gizmo group — renders on top of all 3D geometry
// ---------------------------------------------------------------------------

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct GimbalGizmoGroup;

/// Call once at Startup to configure the gimbal gizmo group to draw on top.
pub fn configure_gimbal_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = config_store.config_mut::<GimbalGizmoGroup>();
    cfg.depth_bias = -1.0; // always render in front of meshes
    cfg.line.width = 2.5;
}

// ---------------------------------------------------------------------------
// Public types (used by node_manager for drag state)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GimbalAxis {
    X,
    Y,
    Z,
}

// ---------------------------------------------------------------------------
// Visual constants
// ---------------------------------------------------------------------------

pub const GIMBAL_LEN: f32 = 1.5;
const COLOR_X: Color = Color::srgb(0.9, 0.2, 0.2);
const COLOR_Y: Color = Color::srgb(0.2, 0.9, 0.2);
const COLOR_Z: Color = Color::srgb(0.3, 0.4, 0.9);
const COLOR_HOVER: Color = Color::srgb(1.0, 1.0, 0.2);

// ---------------------------------------------------------------------------
// Public drawing entry point (called by node_manager's draw system)
// ---------------------------------------------------------------------------

/// Compute the world-space center of the gimbal for an entity.
///
/// If the entity (or any child) has a Bevy `Aabb`, the gimbal is placed at
/// the AABB center transformed to world space.  Otherwise it falls back to
/// the entity's `GlobalTransform` translation.
pub fn gimbal_center(
    entity: Entity,
    g_transform_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&bevy::camera::primitives::Aabb>,
    children_query: &Query<&Children>,
) -> Option<Vec3> {
    let g_tx = g_transform_query.get(entity).ok()?;
    // Try the entity itself first, then scan immediate children (glTF scenes
    // often place the Aabb on a child mesh entity, not the root).
    if let Ok(aabb) = aabb_query.get(entity) {
        return Some(g_tx.transform_point(aabb.center.into()));
    }
    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            if let (Ok(child_gtx), Ok(aabb)) =
                (g_transform_query.get(child), aabb_query.get(child))
            {
                return Some(child_gtx.transform_point(aabb.center.into()));
            }
        }
    }
    Some(g_tx.translation())
}

/// Draw the gimbal handles at `center` given the current `tool` and which
/// axis (if any) is highlighted (`highlight` covers both hover and drag).
pub fn draw_gimbal(
    center: Vec3,
    tool: Tool,
    highlight: Option<GimbalAxis>,
    mut gizmos: Gizmos<GimbalGizmoGroup>,
) {
    if tool == Tool::Select {
        return;
    }

    let cx = if highlight == Some(GimbalAxis::X) { COLOR_HOVER } else { COLOR_X };
    let cy = if highlight == Some(GimbalAxis::Y) { COLOR_HOVER } else { COLOR_Y };
    let cz = if highlight == Some(GimbalAxis::Z) { COLOR_HOVER } else { COLOR_Z };

    match tool {
        Tool::Select => {}
        Tool::Move => {
            draw_move_handle(&mut gizmos, center, Vec3::X, cx);
            draw_move_handle(&mut gizmos, center, Vec3::Y, cy);
            draw_move_handle(&mut gizmos, center, Vec3::Z, cz);
        }
        Tool::Rotate => {
            draw_rotate_ring(&mut gizmos, center, Vec3::X, cx);
            draw_rotate_ring(&mut gizmos, center, Vec3::Y, cy);
            draw_rotate_ring(&mut gizmos, center, Vec3::Z, cz);
        }
        Tool::Scale => {
            draw_scale_handle(&mut gizmos, center, Vec3::X, cx);
            draw_scale_handle(&mut gizmos, center, Vec3::Y, cy);
            draw_scale_handle(&mut gizmos, center, Vec3::Z, cz);
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

fn draw_move_handle(gizmos: &mut Gizmos<GimbalGizmoGroup>, center: Vec3, dir: Vec3, color: Color) {
    let tip = center + dir * GIMBAL_LEN;
    gizmos.line(center, tip, color);
    let perp = if dir.abs().dot(Vec3::Y) < 0.9 {
        dir.cross(Vec3::Y).normalize()
    } else {
        dir.cross(Vec3::X).normalize()
    };
    let back = tip - dir * 0.2;
    gizmos.line(tip, back + perp * 0.12, color);
    gizmos.line(tip, back - perp * 0.12, color);
}

fn draw_rotate_ring(gizmos: &mut Gizmos<GimbalGizmoGroup>, center: Vec3, axis: Vec3, color: Color) {
    const SEGS: usize = 48;
    let perp1 = if axis.abs().dot(Vec3::Y) < 0.9 {
        axis.cross(Vec3::Y).normalize()
    } else {
        axis.cross(Vec3::X).normalize()
    };
    let perp2 = axis.cross(perp1).normalize();
    let mut prev = center + perp1 * GIMBAL_LEN;
    for i in 1..=SEGS {
        let angle = i as f32 * std::f32::consts::TAU / SEGS as f32;
        let next = center + (perp1 * angle.cos() + perp2 * angle.sin()) * GIMBAL_LEN;
        gizmos.line(prev, next, color);
        prev = next;
    }
}

fn draw_scale_handle(gizmos: &mut Gizmos<GimbalGizmoGroup>, center: Vec3, dir: Vec3, color: Color) {
    let tip = center + dir * GIMBAL_LEN;
    gizmos.line(center, tip, color);
    let perp1 = if dir.abs().dot(Vec3::Y) < 0.9 {
        dir.cross(Vec3::Y).normalize()
    } else {
        dir.cross(Vec3::X).normalize()
    };
    let perp2 = dir.cross(perp1).normalize();
    let s = 0.1;
    let c1 = tip + (perp1 + perp2) * s;
    let c2 = tip + (perp1 - perp2) * s;
    let c3 = tip + (-perp1 - perp2) * s;
    let c4 = tip + (-perp1 + perp2) * s;
    gizmos.line(c1, c2, color);
    gizmos.line(c2, c3, color);
    gizmos.line(c3, c4, color);
    gizmos.line(c4, c1, color);
}

// ---------------------------------------------------------------------------
// Ring geometry helpers (used by node_manager for hit-testing)
// ---------------------------------------------------------------------------

/// Returns the two perpendicular basis vectors for a rotation ring around `axis`.
pub fn ring_basis(axis: Vec3) -> (Vec3, Vec3) {
    let perp1 = if axis.abs().dot(Vec3::Y) < 0.9 {
        axis.cross(Vec3::Y).normalize()
    } else {
        axis.cross(Vec3::X).normalize()
    };
    let perp2 = axis.cross(perp1).normalize();
    (perp1, perp2)
}

/// Sample points along a rotation ring for screen-space hit testing.
pub fn ring_points(center: Vec3, axis: Vec3, count: usize) -> Vec<Vec3> {
    let (perp1, perp2) = ring_basis(axis);
    (0..count)
        .map(|i| {
            let angle = i as f32 * std::f32::consts::TAU / count as f32;
            center + (perp1 * angle.cos() + perp2 * angle.sin()) * GIMBAL_LEN
        })
        .collect()
}
