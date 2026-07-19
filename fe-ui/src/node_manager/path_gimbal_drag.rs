//! FR-3 (drag half): drag a selected path VERTEX or SEGMENT via the gimbal.
//!
//! A path vertex/segment has no ECS entity (it lives in `PathEditorState`, not
//! `NodeManager.selected`), so this can't reuse `handle_gimbal_interaction`'s
//! entity-`Transform` drag. Instead it computes the new world position from the
//! gimbal axis + cursor delta (same math as the entity gimbal) and, on release,
//! queues `UiAction::PathMovePoint` per affected index — a segment moves both its
//! endpoints by the same delta. See `fe-ui/src/node_manager/AGENTS.md` §dispatch
//! (FR-3 drag) + §selection-read-model.

use bevy::prelude::*;

use super::dispatch::{resolve_operation, HitTarget, Operation};
use super::gimbal_interaction::{axis_screen_dir, axis_vec, pick_axis};
use super::path_point_interaction::PathPointMarker;
use super::router::{ClickArbiter, ClickPriority};
use super::selection::{path_gimbal_target, project_selection, SelectionKind};
use super::NodeManager;
use crate::actions::{UiAction, UiManager};
use crate::gimbal::GimbalAxis;
use crate::gis::PathEditorState;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;

/// Camera-distance move sensitivity — mirrors `handle_gimbal_interaction`'s
/// `Tool::Move` branch so a vertex drag feels identical to an entity drag.
const MOVE_SENSITIVITY: f32 = 0.002;

/// In-progress path-gimbal drag, `None` when idle.
#[derive(Resource, Default)]
pub struct PathGimbalDrag {
    pub active: Option<PathGimbalDragState>,
}

/// A live vertex/segment gimbal drag. `affected` holds `(index, start world
/// position)` for each moved point (one for a vertex, two for a segment);
/// `current` is the same shape, updated to the live dragged position each hold
/// frame and committed on release.
#[derive(Clone, Debug)]
pub struct PathGimbalDragState {
    pub axis: GimbalAxis,
    pub axis_screen_dir: Vec2,
    pub start_cursor: Vec2,
    /// Gimbal target at drag-start (vertex point or segment midpoint) — the
    /// camera-distance frame for the move sensitivity.
    pub start_center: Vec3,
    pub track_id: String,
    pub affected: Vec<(usize, Vec3)>,
    pub current: Vec<Vec3>,
}

/// World-space translation delta for a gimbal-axis drag: project the cursor
/// travel onto the axis's screen direction, scale by camera distance. Pure.
pub(super) fn drag_axis_delta(
    axis_dir: Vec3,
    axis_screen_dir: Vec2,
    start_cursor: Vec2,
    cur_cursor: Vec2,
    scale_factor: f32,
) -> Vec3 {
    let movement = (cur_cursor - start_cursor).dot(axis_screen_dir);
    axis_dir * movement * scale_factor
}

/// New world position of a point dragged from `start_pos` along the gimbal axis.
/// Pure (Bevy-App-free) — the FR-3 math the unit tests pin down.
pub(super) fn drag_delta_to_position(
    start_pos: Vec3,
    axis_dir: Vec3,
    axis_screen_dir: Vec2,
    start_cursor: Vec2,
    cur_cursor: Vec2,
    scale_factor: f32,
) -> Vec3 {
    start_pos
        + drag_axis_delta(
            axis_dir,
            axis_screen_dir,
            start_cursor,
            cur_cursor,
            scale_factor,
        )
}

/// Camera-distance scale factor for the move drag (mirrors the entity gimbal's
/// `* 0.002` with the same `0.5` floor). Pure.
pub(super) fn move_scale_factor(center: Vec3, cam_pos: Vec3) -> f32 {
    (center - cam_pos).length().max(0.5) * MOVE_SENSITIVITY
}

/// FR-3 drag: press the gimbal axis over a selected vertex/segment → drag →
/// release commits `PathMovePoint`(s). Runs BEFORE `handle_gimbal_interaction`
/// in the chain and claims `ClickPriority::Gimbal` first, so the entity gimbal
/// yields (first-claim-wins) and never starts a competing entity drag. Gated to
/// `Tool::Move` only, so Pen appends and Select picks are untouched.
pub(super) fn handle_path_gimbal_drag(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    node_mgr: Res<NodeManager>,
    path_state: Res<PathEditorState>,
    tool: Res<ToolState>,
    mut drag: ResMut<PathGimbalDrag>,
    mut ui_mgr: ResMut<UiManager>,
    mut markers: Query<(&mut Transform, &PathPointMarker)>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Only the Move tool drags a path point via the gimbal. Any other tool
    // cancels an in-flight drag (keeps Pen/Select click paths safe).
    if tool.active_tool != Tool::Move {
        drag.active = None;
        return;
    }

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else {
        return;
    };

    // Release → commit one PathMovePoint per moved index (skip unmoved points so
    // a bare axis click doesn't queue a no-op round-trip).
    if mouse_button.just_released(MouseButton::Left) {
        if let Some(state) = drag.active.take() {
            for (i, (idx, start)) in state.affected.iter().enumerate() {
                let pos = state.current.get(i).copied().unwrap_or(*start);
                if (pos - *start).length() < 1e-4 {
                    continue;
                }
                ui_mgr.push_action(UiAction::PathMovePoint {
                    track_node_id: state.track_id.clone(),
                    index: *idx,
                    position: [pos.x, pos.y, pos.z],
                });
            }
        }
        return;
    }

    // Hold → recompute live positions and move the affected markers for feedback.
    if mouse_button.pressed(MouseButton::Left) {
        if let Some(state) = drag.active.as_mut() {
            let Some(cursor_pos) = cursor else { return };
            let scale = move_scale_factor(state.start_center, cam_tx.translation());
            let axis_dir = axis_vec(state.axis);
            let delta = drag_axis_delta(
                axis_dir,
                state.axis_screen_dir,
                state.start_cursor,
                cursor_pos,
                scale,
            );
            state.current.clear();
            for (_, start) in &state.affected {
                state.current.push(*start + delta);
            }
            for (mut tx, marker) in markers.iter_mut() {
                if let Some(pos_i) = state.affected.iter().position(|(idx, _)| *idx == marker.index) {
                    if let Some(p) = state.current.get(pos_i) {
                        tx.translation = *p;
                    }
                }
            }
            return;
        }
    }

    // Press → maybe begin a drag. The arbiter has already applied egui + viewport
    // gating and computed the ray/phase.
    if !arbiter.is_fresh_press() || !arbiter.is_available() {
        return;
    }
    let Some(cursor_pos) = cursor else { return };

    // Current typed selection (computed fresh here — `SelectionState` is projected
    // late in the chain, so reading it directly would be a frame stale).
    let node_selected = node_mgr
        .selected
        .as_ref()
        .map(|s| (s.entity, s.node_id.as_str()));
    let kind = project_selection(
        node_selected,
        path_state.editing_track_id.as_deref(),
        path_state.selected_point,
        path_state.selected_segment,
    );
    if !matches!(
        kind,
        SelectionKind::PathVertex { .. } | SelectionKind::PathSegment { .. }
    ) {
        return;
    }
    let Some(track_id) = path_state.editing_track_id.clone() else {
        return;
    };

    let points: Vec<Vec3> = path_state
        .points
        .iter()
        .map(|p| Vec3::from(p.position))
        .collect();
    let Some(center) = path_gimbal_target(&kind, &points) else {
        return;
    };
    // Same axis pick as the entity gimbal, but against the resolved path target.
    let Some(axis) = pick_axis(Tool::Move, cursor_pos, center, camera, cam_tx) else {
        return;
    };

    // FR-2 dispatch: what does a gimbal-axis press mean for this selection?
    let affected: Vec<(usize, Vec3)> =
        match resolve_operation(Tool::Move, &kind, HitTarget::GimbalAxis) {
            Operation::MoveVertex { idx } => {
                points.get(idx).map(|p| vec![(idx, *p)]).unwrap_or_default()
            }
            Operation::MoveSegment { idx } => match (points.get(idx), points.get(idx + 1)) {
                (Some(a), Some(b)) => vec![(idx, *a), (idx + 1, *b)],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
    if affected.is_empty() {
        return;
    }

    // Claim the gimbal FIRST so `handle_gimbal_interaction` (next in the chain)
    // can't also grab this press for an entity drag.
    if !arbiter.claim(ClickPriority::Gimbal) {
        return;
    }
    let screen_dir = axis_screen_dir(axis, center, camera, cam_tx);
    let current: Vec<Vec3> = affected.iter().map(|(_, p)| *p).collect();
    drag.active = Some(PathGimbalDragState {
        axis,
        axis_screen_dir: screen_dir,
        start_cursor: cursor_pos,
        start_center: center,
        track_id,
        affected,
        current,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_delta_projects_cursor_onto_screen_dir() {
        // Move 100px along +X screen with the axis pointing +X on screen: full
        // projection → 100 * scale along the world X axis.
        let d = drag_axis_delta(
            Vec3::X,
            Vec2::X,
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            0.002,
        );
        assert!((d - Vec3::new(0.2, 0.0, 0.0)).length() < 1e-6, "d = {d:?}");
    }

    #[test]
    fn axis_delta_ignores_perpendicular_cursor_motion() {
        // Cursor moves straight down; axis screen dir is horizontal → zero dot.
        let d = drag_axis_delta(
            Vec3::X,
            Vec2::X,
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 100.0),
            0.002,
        );
        assert!(d.length() < 1e-6, "expected ~0 delta, got {d:?}");
    }

    #[test]
    fn delta_to_position_adds_delta_to_start() {
        let p = drag_delta_to_position(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::Z,
            Vec2::X,
            Vec2::new(10.0, 0.0),
            Vec2::new(60.0, 0.0),
            0.002,
        );
        // 50px * 0.002 = 0.1 along +Z.
        assert!((p - Vec3::new(1.0, 2.0, 3.1)).length() < 1e-6, "p = {p:?}");
    }

    #[test]
    fn segment_endpoints_move_by_the_same_delta() {
        // A segment drag applies one delta to both endpoints (parallel translate).
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(10.0, 0.0, 0.0);
        let new_a = drag_delta_to_position(a, Vec3::X, Vec2::X, Vec2::ZERO, Vec2::new(50.0, 0.0), 0.002);
        let new_b = drag_delta_to_position(b, Vec3::X, Vec2::X, Vec2::ZERO, Vec2::new(50.0, 0.0), 0.002);
        assert!((new_a - Vec3::new(0.1, 0.0, 0.0)).length() < 1e-6, "a = {new_a:?}");
        assert!((new_b - Vec3::new(10.1, 0.0, 0.0)).length() < 1e-6, "b = {new_b:?}");
        // The segment's length/orientation is preserved by an equal delta.
        assert!(((new_b - new_a) - (b - a)).length() < 1e-6);
    }

    #[test]
    fn move_scale_factor_scales_with_camera_distance() {
        // 10 world units away → 10 * 0.002.
        let s = move_scale_factor(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0));
        assert!((s - 0.02).abs() < 1e-6, "s = {s}");
    }

    #[test]
    fn move_scale_factor_floors_at_half_unit() {
        // Camera on top of the target → distance 0 clamps to the 0.5 floor.
        let s = move_scale_factor(Vec3::new(3.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0));
        assert!((s - 0.001).abs() < 1e-6, "s = {s}");
    }
}
