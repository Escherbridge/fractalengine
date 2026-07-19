//! Gimbal axis interaction: hover detection, pick → drag → commit, and
//! delegating the visual draw to `crate::gimbal`.

use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority};
use super::selection::{is_path_selection, path_gimbal_target};
use super::{AxisDrag, NodeManager, SelectionKind, SelectionState};
use crate::gimbal::{
    draw_gimbal, gimbal_center, ring_points_buf, GimbalAxis, GimbalGizmoGroup, GIMBAL_LEN,
    RING_SEGMENTS,
};
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
    let Some(ref sel) = manager.selected else {
        return;
    };
    // Don't update hover while dragging — keep the dragged axis highlighted.
    if sel.drag.is_some() {
        return;
    }

    let entity = sel.entity;
    let Ok(window) = windows.single() else { return };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    if !viewport_rect
        .0
        .contains(bevy_egui::egui::pos2(cursor.x, cursor.y))
    {
        return;
    }
    let Ok((camera, cam_tx)) = cameras.single() else {
        return;
    };
    let Some(center_3d) = gimbal_center(entity, &g_transform_query, &aabb_query, &children_query)
    else {
        return;
    };

    manager.hovered_axis = pick_axis(tool.active_tool, cursor, center_3d, camera, cam_tx);
}

/// Shared axis picking logic for both hover and click. `pub(super)` so the FR-3
/// path-gimbal drag (`path_gimbal_drag.rs`) picks against a resolved path target
/// with the same math (see AGENTS.md §dispatch).
pub(super) fn pick_axis(
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

        let threshold = if tool == Tool::Rotate {
            RING_PICK_PX
        } else {
            PICK_PX
        };
        if dist < threshold && (best.is_none() || dist < best.unwrap().1) {
            best = Some((axis, dist));
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
    // Stack buffer — this runs every frame during hover; no heap allocation.
    let mut points = [Vec3::ZERO; RING_SEGMENTS];
    ring_points_buf(center_3d, axis, &mut points);
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
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&bevy::camera::primitives::Aabb>,
    children_query: Query<&Children>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Only the transform gizmo tools run the gimbal. Select has no gizmo; Pen
    // draws paths — running here would let `pick_axis` claim `Gimbal` (highest
    // priority) on a press near a projected axis and start a no-op drag that
    // steals a click meant for PathPlace (pen append). The Pen drag branch below
    // is already a no-op, so early-returning loses nothing. See AGENTS.md §pen-tool.
    if matches!(tool.active_tool, Tool::Select | Tool::Pen) {
        return;
    }
    let Some(sel) = manager.selected.as_mut() else {
        return;
    };
    let entity = sel.entity;

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else {
        return;
    };

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
                    let scale_factor = (g_tx.translation() - cam_tx.translation())
                        .length()
                        .max(0.5)
                        * 0.002;
                    transform.translation = drag.start_pos + axis_dir * movement * scale_factor;
                }
                Tool::Scale => {
                    let movement = (cursor_pos - drag.start_cursor).dot(drag.axis_screen_dir);
                    let f = 1.0 + movement * 0.005;
                    let b = drag.start_scale;
                    transform.scale = Vec3::new(
                        if drag.axis == GimbalAxis::X {
                            b.x * f
                        } else {
                            b.x
                        },
                        if drag.axis == GimbalAxis::Y {
                            b.y * f
                        } else {
                            b.y
                        },
                        if drag.axis == GimbalAxis::Z {
                            b.z * f
                        } else {
                            b.z
                        },
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

    // Pick axis on press — claim `Gimbal` only when an axis is actually grabbed,
    // so a press that misses the gimbal yields to lower-priority consumers.
    if arbiter.is_fresh_press() && arbiter.is_available() {
        let Some(cursor_pos) = cursor else { return };
        let Ok((t, g_tx)) = transform_query.get(entity) else {
            return;
        };
        let center_3d = gimbal_center(entity, &g_transform_query, &aabb_query, &children_query)
            .unwrap_or_else(|| g_tx.translation());
        let Ok(center_screen) = camera.world_to_viewport(cam_tx, center_3d) else {
            return;
        };

        if let Some(axis) = pick_axis(tool.active_tool, cursor_pos, center_3d, camera, cam_tx) {
            if !arbiter.claim(ClickPriority::Gimbal) {
                return;
            }
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

/// World-space unit vector of a gimbal axis. `pub(super)` — shared with the FR-3
/// path-gimbal drag (`path_gimbal_drag.rs`).
pub(super) fn axis_vec(axis: GimbalAxis) -> Vec3 {
    match axis {
        GimbalAxis::X => Vec3::X,
        GimbalAxis::Y => Vec3::Y,
        GimbalAxis::Z => Vec3::Z,
    }
}

/// Screen-space unit direction of `axis` projected from the gimbal `center_3d`,
/// or zero if the center fails to project. `pub(super)` — the FR-3 path-gimbal
/// drag captures this exactly as `handle_gimbal_interaction`'s press branch does,
/// so vertex/segment drags feel identical to entity drags (see AGENTS.md §dispatch).
pub(super) fn axis_screen_dir(
    axis: GimbalAxis,
    center_3d: Vec3,
    camera: &Camera,
    cam_tx: &GlobalTransform,
) -> Vec2 {
    let Ok(center_screen) = camera.world_to_viewport(cam_tx, center_3d) else {
        return Vec2::ZERO;
    };
    let tip_3d = center_3d + axis_vec(axis) * GIMBAL_LEN;
    let tip_screen = camera
        .world_to_viewport(cam_tx, tip_3d)
        .unwrap_or(center_screen);
    (tip_screen - center_screen).normalize_or_zero()
}

fn segment_dist_2d(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let ab_len_sq = ab.dot(ab);
    if ab_len_sq < 1e-4 {
        return (p - a).length(); // degenerate segment — distance to endpoint
    }
    let t = ((p - a).dot(ab) / ab_len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

// ---------------------------------------------------------------------------
// System: gimbal drawing (delegates to gimbal.rs pure-visual functions)
// ---------------------------------------------------------------------------

pub(super) fn draw_gimbal_system(
    manager: Res<NodeManager>,
    selection: Res<SelectionState>,
    tool: Res<ToolState>,
    path_state: Res<crate::gis::PathEditorState>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&bevy::camera::primitives::Aabb>,
    children_query: Query<&Children>,
    gizmos: Gizmos<GimbalGizmoGroup>,
) {
    // FR-3: path selections draw a gimbal even under Select/Pen (which have no
    // gizmo of their own → show Move arrows as a grabbable handle). Drawing
    // steals no clicks, so relaxing the tool gate for the *visual* is safe (the
    // axis-pick gate in `handle_gimbal_interaction` is untouched). Vertex/segment
    // resolve to a world point (they have no entity); a whole track keeps its
    // bridged ribbon-entity center so the drawn handle stays where the axis-pick
    // expects it (no regression to the existing whole-track drag).
    if is_path_selection(&selection.kind) {
        let path_center = match &selection.kind {
            SelectionKind::PathTrack { .. } => manager
                .selected
                .as_ref()
                .and_then(|s| {
                    gimbal_center(s.entity, &g_transform_query, &aabb_query, &children_query)
                })
                .or_else(|| {
                    // Track open but ribbon unspawned: fall back to the centroid.
                    let points: Vec<Vec3> =
                        path_state.points.iter().map(|p| Vec3::from(p.position)).collect();
                    path_gimbal_target(&selection.kind, &points)
                }),
            _ => {
                let points: Vec<Vec3> =
                    path_state.points.iter().map(|p| Vec3::from(p.position)).collect();
                path_gimbal_target(&selection.kind, &points)
            }
        };
        if let Some(center) = path_center {
            let effective_tool = match tool.active_tool {
                Tool::Select | Tool::Pen => Tool::Move,
                other => other,
            };
            draw_gimbal(center, effective_tool, manager.hovered_axis, gizmos);
        }
        return;
    }

    // Entity selections (node / bridged stamp) use the entity-based gimbal.
    let Some(sel) = &manager.selected else { return };
    let Some(center) = gimbal_center(sel.entity, &g_transform_query, &aabb_query, &children_query)
    else {
        return;
    };
    // Dragged axis takes priority over hover highlight.
    let highlight = sel.drag.as_ref().map(|d| d.axis).or(manager.hovered_axis);
    draw_gimbal(center, tool.active_tool, highlight, gizmos);
}
