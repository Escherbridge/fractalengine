//! Gimbal gizmo rendering — pure visual, no state.
//!
//! All interaction logic and selection state lives in [`crate::node_manager`].
//! This module only knows how to *draw* the coloured axis handles for a given
//! entity, active tool, and optionally highlighted axis.

use bevy::prelude::*;

use crate::panels::Tool;

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

const GIMBAL_LEN: f32 = 1.5;
const COLOR_X: Color = Color::srgb(0.9, 0.2, 0.2);
const COLOR_Y: Color = Color::srgb(0.2, 0.9, 0.2);
const COLOR_Z: Color = Color::srgb(0.3, 0.4, 0.9);
const COLOR_HOVER: Color = Color::srgb(1.0, 1.0, 0.2);

// ---------------------------------------------------------------------------
// Public drawing entry point (called by node_manager's draw system)
// ---------------------------------------------------------------------------

/// Draw the gimbal handles for `entity` given the current `tool` and which
/// axis (if any) is actively being dragged (`dragging`).
pub fn draw_gimbal_for_entity(
    entity: Entity,
    tool: Tool,
    dragging: Option<GimbalAxis>,
    g_transform_query: &Query<&GlobalTransform>,
    mut gizmos: Gizmos,
) {
    if tool == Tool::Select {
        return;
    }
    let Ok(g_tx) = g_transform_query.get(entity) else { return };
    let center = g_tx.translation();

    let cx = if dragging == Some(GimbalAxis::X) { COLOR_HOVER } else { COLOR_X };
    let cy = if dragging == Some(GimbalAxis::Y) { COLOR_HOVER } else { COLOR_Y };
    let cz = if dragging == Some(GimbalAxis::Z) { COLOR_HOVER } else { COLOR_Z };

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

fn draw_move_handle(gizmos: &mut Gizmos, center: Vec3, dir: Vec3, color: Color) {
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

fn draw_rotate_ring(gizmos: &mut Gizmos, center: Vec3, axis: Vec3, color: Color) {
    const SEGS: usize = 32;
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

fn draw_scale_handle(gizmos: &mut Gizmos, center: Vec3, dir: Vec3, color: Color) {
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
