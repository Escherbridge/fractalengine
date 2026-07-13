//! Gimbal axis interaction: hover detection, pick → drag → commit, and
//! delegating the visual draw to `crate::gimbal`.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::{AxisDrag, NodeManager};
use crate::gimbal::{draw_gimbal, gimbal_center, ring_points, GimbalAxis, GimbalGizmoGroup, GIMBAL_LEN};
use crate::panels::toolbar::Tool;
use crate::plugin::{ToolState, ViewportRect};

const PICK_PX: f32 = 20.0;
const RING_PICK_PX: f32 = 14.0;

// ---------------------------------------------------------------------------
// System: hover detection (updates hovered_axis for visual feedback)
// ---------------------------------------------------------------------------

pub(super) fn update_hovered_axis(
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut manager: ResMut<NodeManager>,
    tool: Res<ToolState>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&bevy::camera::primitives::Aabb>,
    children_query: Query<&Children>,
    viewport_rect: Res<ViewportRect>,
) {
    manager.hovered_axis = None;

    if tool.active_tool == Tool::Select {
        return;
    }
    let Some(ref sel) = manager.selected else { return };
    // Don't update hover while dragging — keep the dragged axis highlighted.
    if sel.drag.is_some() {
        return;
    }

    let entity = sel.entity;
    let Ok(window) = windows.single() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    if !viewport_rect.0.contains(bevy_egui::egui::pos2(cursor.x, cursor.y)) {
        return;
    }
    let Ok((camera, cam_tx)) = cameras.single() else { return };
    let Some(center_3d) = gimbal_center(entity, &g_transform_query, &aabb_query, &children_query)
    else {
        return;
    };

    manager.hovered_axis = pick_axis(tool.active_tool, cursor, center_3d, camera, cam_tx);
}

/// Shared axis picking logic for both hover and click.
fn pick_axis(
    tool: Tool,
    cursor: Vec2,
    center_3d: Vec3,
    camera: &Camera,
    cam_tx: &GlobalTransform,
) -> Option<GimbalAxis> {
    let Ok(center_screen) = camera.world_to_viewport(cam_tx, center_3d) else {
        return None;
    };

    let mut best: Option<(GimbalAxis, f32)> = None;

    for axis in [GimbalAxis::X, GimbalAxis::Y, GimbalAxis::Z] {
        let dist = if tool == Tool::Rotate {
            // For rotation: check distance to the projected ring
            ring_screen_distance(cursor, center_3d, axis_vec(axis), camera, cam_tx)
        } else {
            // For move/scale: check distance to the axis line segment
            let tip_3d = center_3d + axis_vec(axis) * GIMBAL_LEN;
            let Ok(tip_screen) = camera.world_to_viewport(cam_tx, tip_3d) else {
                continue;
            };
            segment_dist_2d(cursor, center_screen, tip_screen)
        };

        let threshold = if tool == Tool::Rotate { RING_PICK_PX } else { PICK_PX };
        if dist < threshold {
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((axis, dist));
            }
        }
    }

    best.map(|(axis, _)| axis)
}

/// Minimum screen-space distance from `cursor` to a rotation ring.
fn ring_screen_distance(
    cursor: Vec2,
    center_3d: Vec3,
    axis: Vec3,
    camera: &Camera,
    cam_tx: &GlobalTransform,
) -> f32 {
    let points = ring_points(center_3d, axis, 48);
    let mut min_dist = f32::MAX;
    let mut prev_screen: Option<Vec2> = None;
    for pt in &points {
        let Ok(screen) = camera.world_to_viewport(cam_tx, *pt) else {
            prev_screen = None;
            continue;
        };
        if let Some(prev) = prev_screen {
            let d = segment_dist_2d(cursor, prev, screen);
            if d < min_dist {
                min_dist = d;
            }
        }
        prev_screen = Some(screen);
    }
    // Close the ring: last → first
    if let (Some(last), Some(Ok(first))) = (
        prev_screen,
        points.first().map(|p| camera.world_to_viewport(cam_tx, *p)),
    ) {
        let d = segment_dist_2d(cursor, last, first);
        if d < min_dist {
            min_dist = d;
        }
    }
    min_dist
}

// ---------------------------------------------------------------------------
// System: gimbal axis interaction (pick → drag → commit)
// ---------------------------------------------------------------------------

pub(super) fn handle_gimbal_interaction(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut manager: ResMut<NodeManager>,
    tool: Res<ToolState>,
    mut transform_query: Query<(&mut Transform, &GlobalTransform)>,
    aabb_query: Query<&bevy::camera::primitives::Aabb>,
    children_query: Query<&Children>,
    mut egui_ctx: EguiContexts,
    viewport_rect: Res<ViewportRect>,
) {
    if tool.active_tool == Tool::Select {
        return;
    }
    let Some(sel) = manager.selected.as_mut() else {
        return;
    };
    let entity = sel.entity;

    let egui_using = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.is_using_pointer())
        .unwrap_or(false);

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else { return };

    // Release → commit
    if mouse_button.just_released(MouseButton::Left) && sel.drag.is_some() {
        sel.drag = None;
        sel.drag_committed = true;
        return;
    }

    // Apply active drag
    if mouse_button.pressed(MouseButton::Left) {
        if let Some(ref drag) = sel.drag {
            let Some(cursor_pos) = cursor else { return };
            let Ok((mut transform, g_tx)) = transform_query.get_mut(entity) else {
                return;
            };
            let axis_dir = axis_vec(drag.axis);
            match tool.active_tool {
                Tool::Move => {
                    let movement = (cursor_pos - drag.start_cursor).dot(drag.axis_screen_dir);
                    let scale_factor = (g_tx.translation() - cam_tx.translation()).length().max(0.5) * 0.002;
                    transform.translation = drag.start_pos + axis_dir * movement * scale_factor;
                }
                Tool::Scale => {
                    let movement = (cursor_pos - drag.start_cursor).dot(drag.axis_screen_dir);
                    let f = 1.0 + movement * 0.005;
                    let b = drag.start_scale;
                    transform.scale = Vec3::new(
                        if drag.axis == GimbalAxis::X { b.x * f } else { b.x },
                        if drag.axis == GimbalAxis::Y { b.y * f } else { b.y },
                        if drag.axis == GimbalAxis::Z { b.z * f } else { b.z },
                    );
                }
                Tool::Rotate => {
                    // Use perpendicular screen direction for intuitive rotation:
                    // dragging "around" the ring, not along the axis.
                    let tangent = Vec2::new(-drag.axis_screen_dir.y, drag.axis_screen_dir.x);
                    let movement = (cursor_pos - drag.start_cursor).dot(tangent);
                    transform.rotation =
                        Quat::from_axis_angle(axis_dir, movement * 0.01) * drag.start_rot;
                }
                // Pen is a path-drawing mode, not a transform gizmo — no drag.
                Tool::Select | Tool::Pen => {}
            }
            return;
        }
    }

    // Pick axis on press
    let in_viewport = cursor
        .map(|c| viewport_rect.0.contains(bevy_egui::egui::pos2(c.x, c.y)))
        .unwrap_or(false);

    if mouse_button.just_pressed(MouseButton::Left) && !egui_using && in_viewport {
        let Some(cursor_pos) = cursor else { return };
        let Ok((t, g_tx)) = transform_query.get(entity) else { return };
        let center_3d = compute_gimbal_center_inline(entity, g_tx, &aabb_query, &children_query, &transform_query);
        let Ok(center_screen) = camera.world_to_viewport(cam_tx, center_3d) else { return };

        if let Some(axis) = pick_axis(tool.active_tool, cursor_pos, center_3d, camera, cam_tx) {
            let tip_3d = center_3d + axis_vec(axis) * GIMBAL_LEN;
            let tip_screen = camera
                .world_to_viewport(cam_tx, tip_3d)
                .unwrap_or(center_screen);
            let screen_dir = (tip_screen - center_screen).normalize_or_zero();

            sel.drag = Some(AxisDrag {
                axis,
                start_cursor: cursor_pos,
                axis_screen_dir: screen_dir,
                start_pos: t.translation,
                start_rot: t.rotation,
                start_scale: t.scale,
            });
            sel.drag_committed = false;
        }
    }
}

fn axis_vec(axis: GimbalAxis) -> Vec3 {
    match axis {
        GimbalAxis::X => Vec3::X,
        GimbalAxis::Y => Vec3::Y,
        GimbalAxis::Z => Vec3::Z,
    }
}

/// Compute AABB-based gimbal center without needing a separate `Query<&GlobalTransform>`.
/// Used in systems that already hold a mutable transform query.
fn compute_gimbal_center_inline(
    entity: Entity,
    g_tx: &GlobalTransform,
    aabb_query: &Query<&bevy::camera::primitives::Aabb>,
    children_query: &Query<&Children>,
    transform_query: &Query<(&mut Transform, &GlobalTransform)>,
) -> Vec3 {
    if let Ok(aabb) = aabb_query.get(entity) {
        return g_tx.transform_point(aabb.center.into());
    }
    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            if let (Ok((_, child_gtx)), Ok(aabb)) = (transform_query.get(child), aabb_query.get(child)) {
                return child_gtx.transform_point(aabb.center.into());
            }
        }
    }
    g_tx.translation()
}

fn segment_dist_2d(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.dot(ab).max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

// ---------------------------------------------------------------------------
// System: gimbal drawing (delegates to gimbal.rs pure-visual functions)
// ---------------------------------------------------------------------------

pub(super) fn draw_gimbal_system(
    manager: Res<NodeManager>,
    tool: Res<ToolState>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&bevy::camera::primitives::Aabb>,
    children_query: Query<&Children>,
    gizmos: Gizmos<GimbalGizmoGroup>,
) {
    let Some(sel) = &manager.selected else { return };
    let Some(center) = gimbal_center(sel.entity, &g_transform_query, &aabb_query, &children_query) else {
        return;
    };
    // Dragged axis takes priority over hover highlight.
    let highlight = sel.drag.as_ref().map(|d| d.axis).or(manager.hovered_axis);
    draw_gimbal(center, tool.active_tool, highlight, gizmos);
}
