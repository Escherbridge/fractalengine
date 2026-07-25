//! FR-5 (pen_curve_tool_20260722): viewport bezier-handle markers + drag —
//! one marker per stored handle, picked with the shared along-ray radius,
//! collinearity discipline enforced live, committed via
//! `PathSetAnchorHandles`/`PathSetAnchorCorner` (Authority B only).
//! See `fe-ui/src/node_manager/AGENTS.md` §pen-tool.

use bevy::prelude::*;

use super::dispatch::{resolve_operation, HandleSide, HitTarget, Operation};
use super::path_point_interaction::{ray_plane_y, MAX_POINT_MARKERS, PICK_RADIUS};
use super::router::{sweep_stranded_drag, ClickArbiter, ClickPriority, PointerPhase};
use super::selection::project_selection;
use crate::actions::{UiAction, UiManager};
use crate::gis::{
    min_neighbor_gap_m, smoothness_readback, CornerKind, PathEditorState, PathPointRow,
};
use crate::panels::toolbar::Tool;
use crate::plugin::{Billboard, ToolState};

/// Marker quad for anchor `index`'s `side` handle of the edited track.
#[derive(Component, Debug)]
pub struct PathHandleMarker {
    pub index: usize,
    pub side: HandleSide,
}

/// Handle-marker icon-quad edge (world units) — smaller than the anchor quad
/// so handles read as satellites of their anchor.
const HANDLE_QUAD_SIZE: f32 = 0.5;

/// Handle marker + stem tint (cyan) — distinct from the anchor yellow/orange.
const HANDLE_COLOR: Color = Color::srgb(0.25, 0.8, 1.0);

/// In-progress bezier-handle drag, `None` when idle. See AGENTS.md §pen-tool.
#[derive(Clone, Debug, PartialEq)]
pub struct PathHandleDragState {
    /// Anchor index in the edited track's point list.
    pub index: usize,
    /// Which handle of the anchor is being dragged.
    pub side: HandleSide,
    /// Anchor world position at drag-start (offsets are relative to it).
    pub anchor: Vec3,
    /// Height (Bevy Y) of the horizontal drag plane (the grabbed handle's Y).
    pub plane_y: f32,
    /// Anchor's corner kind at press — the collinearity-discipline source.
    pub start_kind: CornerKind,
    /// Dragged-side offset at press (skip the commit when unmoved).
    pub start: Vec3,
    /// Live dragged-side offset (relative meters).
    pub dragged: Vec3,
    /// Opposite-side discipline BASE: the stored offset at press, re-frozen at
    /// its live value on the first mid-drag Alt (FR-5 Alt-break).
    pub opposite: Option<Vec3>,
    /// Row smoothness at press — the commit fallback when no neighbor gap
    /// exists (otherwise the geometry readback is stored, see AGENTS.md).
    pub smoothness: f32,
    /// `true` once Alt has been observed (one-way break to Corner, FR-5).
    pub alt_broken: bool,
    /// Edited track at Press — Release commits only when it still matches.
    pub press_track_id: String,
}

/// The in-progress handle drag, `None` when idle — mirrors `PathPointDrag`.
#[derive(Resource, Default)]
pub struct PathHandleDrag {
    pub active: Option<PathHandleDragState>,
}

/// Corner kind in force during a handle drag: Alt (once seen) breaks to Corner.
pub(super) fn handle_drag_kind(start: CornerKind, alt_broken: bool) -> CornerKind {
    if alt_broken {
        CornerKind::Corner
    } else {
        start
    }
}

/// Opposite-handle offset while `dragged` moves, under `kind`'s collinearity
/// discipline (FR-5): Symmetric mirrors equal-and-opposite; Smooth keeps the
/// opposite collinear at its OWN length; Corner leaves it independent. A
/// missing opposite is never fabricated.
pub(super) fn disciplined_opposite(
    dragged: Vec3,
    opposite: Option<Vec3>,
    kind: CornerKind,
) -> Option<Vec3> {
    let opp = opposite?;
    match kind {
        CornerKind::Corner => Some(opp),
        CornerKind::Symmetric => Some(-dragged),
        CornerKind::Smooth => {
            let len = dragged.length();
            if len < 1e-6 {
                // Degenerate dragged direction — keep the opposite where it is.
                Some(opp)
            } else {
                Some(dragged * (-opp.length() / len))
            }
        }
    }
}

/// The opposite handle's live offset under the gesture's current discipline.
fn live_opposite(state: &PathHandleDragState) -> Option<Vec3> {
    disciplined_opposite(
        state.dragged,
        state.opposite,
        handle_drag_kind(state.start_kind, state.alt_broken),
    )
}

/// First Alt frame mid-drag: freeze the opposite at its live disciplined value
/// and demote the rest of the gesture to Corner (independent handles, FR-5).
fn handle_observe_alt(state: &mut PathHandleDragState, alt: bool) {
    if alt && !state.alt_broken {
        state.opposite = live_opposite(state);
        state.alt_broken = true;
    }
}

/// Smoothness to store at a handle commit: the geometry readback when a
/// neighbor gap exists, the press-time value otherwise — the stored scalar
/// must track handle truth so the Q5 slider never seeds from a stale value.
pub(super) fn commit_smoothness(
    handle_in: Option<[f32; 3]>,
    handle_out: Option<[f32; 3]>,
    min_gap_m: Option<f32>,
    fallback: f32,
) -> f32 {
    if min_gap_m.is_some() {
        smoothness_readback(handle_in, handle_out, min_gap_m)
    } else {
        fallback
    }
}

/// Final `(handle_in, handle_out)` for the release commit: the dragged side
/// takes the live offset, the opposite its disciplined value. Pure.
pub(super) fn handle_release_fields(
    side: HandleSide,
    dragged: Vec3,
    opposite: Option<Vec3>,
    kind: CornerKind,
) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
    let opp = disciplined_opposite(dragged, opposite, kind).map(|v| v.to_array());
    let drg = Some(dragged.to_array());
    match side {
        HandleSide::In => (drg, opp),
        HandleSide::Out => (opp, drg),
    }
}

/// Nearest (smallest along-ray) handle candidate within `radius` of the ray —
/// the pure core of `pick_handle_marker` (same test as `pick_marker`, Q7).
pub(super) fn pick_nearest_handle(
    origin: Vec3,
    dir: Vec3,
    radius: f32,
    candidates: impl IntoIterator<Item = (usize, HandleSide, Vec3)>,
) -> Option<(usize, HandleSide, f32)> {
    let mut best: Option<(usize, HandleSide, f32)> = None;
    for (idx, side, pos) in candidates {
        let along = (pos - origin).dot(dir);
        if along < 0.0 {
            continue;
        }
        let closest = origin + dir * along;
        if (pos - closest).length() < radius && best.is_none_or(|b| along < b.2) {
            best = Some((idx, side, along));
        }
    }
    best
}

/// Nearest handle marker under `ray` — the shared along-ray + `PICK_RADIUS`
/// test (ratified Q7), mirroring `pick_marker`.
fn pick_handle_marker(
    ray: &Ray3d,
    markers: &Query<(&GlobalTransform, &PathHandleMarker)>,
) -> Option<(usize, HandleSide, f32)> {
    pick_nearest_handle(
        ray.origin,
        *ray.direction,
        PICK_RADIUS,
        markers
            .iter()
            .map(|(g, m)| (m.index, m.side, g.translation())),
    )
}

/// Handle markers the edited track wants: one per `Some` handle side of each
/// (capped) anchor, at `anchor + offset` (world meters). Empty when not editing.
fn wanted_handle_markers(
    editing: bool,
    points: &[PathPointRow],
    cap: usize,
) -> Vec<(usize, HandleSide, Vec3)> {
    if !editing {
        return Vec::new();
    }
    let mut want = Vec::new();
    for (i, row) in points.iter().take(cap).enumerate() {
        let anchor = Vec3::from(row.position);
        if let Some(h) = row.handle_in {
            want.push((i, HandleSide::In, anchor + Vec3::from(h)));
        }
        if let Some(h) = row.handle_out {
            want.push((i, HandleSide::Out, anchor + Vec3::from(h)));
        }
    }
    want
}

/// Keeps one `PathHandleMarker` quad per stored handle of the edited track,
/// following the buffer while idle (a live drag owns the dragged anchor's
/// marker Transforms). Runs before the interaction system so picks see
/// current markers — mirrors `sync_path_point_markers`.
pub(super) fn sync_path_handle_markers(
    path_state: Res<PathEditorState>,
    drag: Res<PathHandleDrag>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut markers: Query<(Entity, &PathHandleMarker, &mut Transform)>,
    mut mesh_handle: Local<Option<Handle<Mesh>>>,
    mut mat_handle: Local<Option<Handle<StandardMaterial>>>,
) {
    let want = wanted_handle_markers(
        path_state.editing_track_id.is_some(),
        &path_state.points,
        MAX_POINT_MARKERS,
    );

    // Membership compare on (index, side) — positions are synced in place
    // below, so only a set change forces the despawn-all rebuild.
    let mut have: Vec<(usize, HandleSide)> =
        markers.iter().map(|(_, m, _)| (m.index, m.side)).collect();
    have.sort_unstable_by_key(|(i, s)| (*i, *s == HandleSide::Out));
    let want_keys: Vec<(usize, HandleSide)> = want.iter().map(|(i, s, _)| (*i, *s)).collect();

    if want_keys != have {
        let mesh = mesh_handle
            .get_or_insert_with(|| meshes.add(Rectangle::new(HANDLE_QUAD_SIZE, HANDLE_QUAD_SIZE)))
            .clone();
        let mat = mat_handle
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: HANDLE_COLOR,
                    unlit: true,
                    cull_mode: None,
                    ..default()
                })
            })
            .clone();
        for (entity, _, _) in markers.iter() {
            commands.entity(entity).despawn();
        }
        for (i, side, pos) in &want {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(*pos),
                Name::new(format!("PathHandle {i} {side:?}")),
                PathHandleMarker {
                    index: *i,
                    side: *side,
                },
                Billboard,
            ));
        }
        return;
    }

    // Position sync: follow the buffer while idle; during a drag the
    // interaction system owns the live marker Transforms.
    if drag.active.is_some() {
        return;
    }
    for (_, marker, mut tx) in markers.iter_mut() {
        if let Some((_, _, pos)) = want
            .iter()
            .find(|(i, s, _)| *i == marker.index && *s == marker.side)
        {
            tx.translation = *pos;
        }
    }
}

/// Whether either Alt key is held.
fn alt_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight)
}

/// Bezier-handle drag (FR-5): pick a handle marker, drag it on its horizontal
/// plane with the Symmetric/Smooth collinearity discipline (Alt breaks to
/// Corner), commit `PathSetAnchorHandles` (+ `PathSetAnchorCorner` on break)
/// on release. Runs FIRST in the click chain, so its `PathHandle` claim
/// outranks the gimbal arm and the vertex marker (ratified Q7).
pub(super) fn handle_path_handle_interaction(
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    path_state: Res<PathEditorState>,
    tool: Res<ToolState>,
    mut drag: ResMut<PathHandleDrag>,
    mut ui_mgr: ResMut<UiManager>,
    mut marker_tx: Query<(&mut Transform, &PathHandleMarker)>,
    marker_pick: Query<(&GlobalTransform, &PathHandleMarker)>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Stranded-gesture sweep: a Hover frame (button up) with a live drag means
    // its Release was missed — drop it before it can replay on a later click.
    sweep_stranded_drag(arbiter.phase(), &mut drag.active);

    // Handles are grabbable while editing in Select/Pen (mirrors the vertex
    // marker gate) — Move/Rotate/Scale yield to the gimbal.
    let editable = path_state.editing_track_id.is_some()
        && matches!(tool.active_tool, Tool::Select | Tool::Pen);
    if !editable && drag.active.is_none() {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else {
        return;
    };

    // Release → commit the drag: SetAnchorHandles when moved, plus the
    // Alt-break SetAnchorCorner (independent handles from here on). Commits
    // only while the press-time track is still the one being edited — a
    // stop/switch mid-hold drops the gesture.
    if arbiter.phase() == PointerPhase::Release {
        if let Some(state) = drag.active.take() {
            if path_state.editing_track_id.as_deref() == Some(state.press_track_id.as_str()) {
                let kind = handle_drag_kind(state.start_kind, state.alt_broken);
                if (state.dragged - state.start).length() >= 1e-4 {
                    let (handle_in, handle_out) =
                        handle_release_fields(state.side, state.dragged, state.opposite, kind);
                    // Stored smoothness tracks the committed geometry (Q5).
                    let prev = state
                        .index
                        .checked_sub(1)
                        .and_then(|i| path_state.points.get(i))
                        .map(|p| p.position);
                    let next = path_state.points.get(state.index + 1).map(|p| p.position);
                    let smoothness = commit_smoothness(
                        handle_in,
                        handle_out,
                        min_neighbor_gap_m(prev, state.anchor.to_array(), next),
                        state.smoothness,
                    );
                    ui_mgr.push_action(UiAction::PathSetAnchorHandles {
                        track_node_id: state.press_track_id.clone(),
                        index: state.index,
                        handle_in,
                        handle_out,
                        smoothness,
                    });
                }
                if state.alt_broken && state.start_kind != CornerKind::Corner {
                    ui_mgr.push_action(UiAction::PathSetAnchorCorner {
                        track_node_id: state.press_track_id,
                        index: state.index,
                        corner: CornerKind::Corner,
                    });
                }
            }
        }
        return;
    }

    // Hold → track the drag on the horizontal plane, observe Alt, and mirror
    // the opposite marker live under the collinearity discipline.
    if arbiter.phase() == PointerPhase::Hold {
        let Some(state) = drag.active.as_mut() else {
            return;
        };
        let Some(cursor_pos) = cursor else { return };
        let Ok(ray) = camera.viewport_to_world(cam_tx, cursor_pos) else {
            return;
        };
        if let Some(hit) = ray_plane_y(&ray, state.plane_y) {
            state.dragged = hit - state.anchor;
        }
        handle_observe_alt(state, alt_held(&keys));
        let opp_live = live_opposite(state);
        for (mut tx, marker) in marker_tx.iter_mut() {
            if marker.index != state.index {
                continue;
            }
            if marker.side == state.side {
                tx.translation = state.anchor + state.dragged;
            } else if let Some(opp) = opp_live {
                tx.translation = state.anchor + opp;
            }
        }
        return;
    }

    // Press → pick a handle marker and begin the drag. The arbiter has already
    // applied egui + viewport gating and computed the ray.
    if !arbiter.is_fresh_press() || !arbiter.is_available() {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };
    let Some((index, side, _)) = pick_handle_marker(&ray, &marker_pick) else {
        return;
    };
    // Route the claim through the FR-2 table (no-bypass): a handle-marker hit
    // resolves to `MoveHandle`, selection- and tool-independent — WHEN it's
    // pickable is this system's gate (the `editable` guard above). Projected
    // from Authority B (the edited track); node selection is immaterial here.
    let kind = project_selection(
        None,
        path_state.editing_track_id.as_deref(),
        path_state.selected_point,
        path_state.selected_segment,
    );
    if !matches!(
        resolve_operation(
            tool.active_tool,
            &kind,
            HitTarget::PathHandle { idx: index, side }
        ),
        Operation::MoveHandle { .. }
    ) {
        return;
    }
    // Q7: claim FIRST — this system runs before the gimbal and vertex
    // consumers, so an overlapping handle + vertex resolves to the handle.
    if !arbiter.claim(ClickPriority::PathHandle) {
        return;
    }
    let Some(row) = path_state.points.get(index) else {
        return; // stale marker (1-frame lag) — click consumed, nothing to drag
    };
    let Some(press_track_id) = path_state.editing_track_id.clone() else {
        return; // markers only exist while editing — same stale-lag guard
    };
    let anchor = Vec3::from(row.position);
    let (dragged, opposite) = match side {
        HandleSide::In => (row.handle_in, row.handle_out),
        HandleSide::Out => (row.handle_out, row.handle_in),
    };
    let Some(dragged) = dragged.map(Vec3::from) else {
        return; // marker without a stored handle — same stale-lag guard
    };
    drag.active = Some(PathHandleDragState {
        index,
        side,
        anchor,
        plane_y: anchor.y + dragged.y,
        start_kind: row.corner,
        start: dragged,
        dragged,
        opposite: opposite.map(Vec3::from),
        smoothness: row.smoothness,
        // Alt already down at press ⇒ independent handles from the start; the
        // opposite keeps its STORED value (no freeze).
        alt_broken: alt_held(&keys),
        press_track_id,
    });
}

/// Illustrator-style stems: a gizmo line from each anchor to its handle
/// marker's live Transform. Immediate-mode, draws only — the grab is the
/// marker pick in `handle_path_handle_interaction`.
pub(super) fn draw_handle_stems(
    path_state: Res<PathEditorState>,
    markers: Query<(&Transform, &PathHandleMarker)>,
    mut gizmos: Gizmos<crate::gimbal::GimbalGizmoGroup>,
) {
    if path_state.editing_track_id.is_none() {
        return;
    }
    for (tx, marker) in markers.iter() {
        let Some(row) = path_state.points.get(marker.index) else {
            continue;
        };
        gizmos.line(Vec3::from(row.position), tx.translation, HANDLE_COLOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- collinearity discipline (FR-5) --------------------------------

    #[test]
    fn symmetric_drag_mirrors_opposite_equal_and_opposite() {
        let opp = disciplined_opposite(
            Vec3::new(2.0, 0.5, 1.0),
            Some(Vec3::new(-0.3, 0.0, 0.2)),
            CornerKind::Symmetric,
        );
        assert_eq!(opp, Some(Vec3::new(-2.0, -0.5, -1.0)));
    }

    #[test]
    fn smooth_drag_keeps_opposite_length_frees_direction() {
        // Opposite keeps its own length (3) but flips collinear to −dragged̂.
        let opp = disciplined_opposite(
            Vec3::new(2.0, 0.0, 0.0),
            Some(Vec3::new(0.0, 0.0, 3.0)),
            CornerKind::Smooth,
        )
        .unwrap();
        assert!(
            (opp - Vec3::new(-3.0, 0.0, 0.0)).length() < 1e-5,
            "opp = {opp:?}"
        );
    }

    #[test]
    fn smooth_drag_degenerate_dragged_keeps_opposite() {
        // A zero-length dragged handle has no direction to enforce.
        let stored = Vec3::new(0.0, 0.0, 3.0);
        let opp = disciplined_opposite(Vec3::ZERO, Some(stored), CornerKind::Smooth);
        assert_eq!(opp, Some(stored));
    }

    #[test]
    fn corner_drag_leaves_opposite_untouched() {
        let stored = Vec3::new(1.0, 0.0, -1.0);
        let opp = disciplined_opposite(Vec3::new(5.0, 0.0, 5.0), Some(stored), CornerKind::Corner);
        assert_eq!(opp, Some(stored));
    }

    #[test]
    fn missing_opposite_is_never_fabricated() {
        for kind in [
            CornerKind::Corner,
            CornerKind::Smooth,
            CornerKind::Symmetric,
        ] {
            assert_eq!(disciplined_opposite(Vec3::X, None, kind), None, "{kind:?}");
        }
    }

    // ---- Alt-break transition (FR-5) ------------------------------------

    fn drag_state(kind: CornerKind) -> PathHandleDragState {
        PathHandleDragState {
            index: 1,
            side: HandleSide::Out,
            anchor: Vec3::ZERO,
            plane_y: 0.0,
            start_kind: kind,
            start: Vec3::X,
            dragged: Vec3::X,
            opposite: Some(-Vec3::X),
            smoothness: 0.0,
            alt_broken: false,
            press_track_id: "track:1".to_string(),
        }
    }

    #[test]
    fn alt_break_freezes_opposite_and_demotes_to_corner() {
        let mut s = drag_state(CornerKind::Symmetric);
        s.dragged = Vec3::new(2.0, 0.0, 1.0); // live mirror: (−2, 0, −1)
        handle_observe_alt(&mut s, true);
        assert!(s.alt_broken);
        assert_eq!(s.opposite, Some(Vec3::new(-2.0, 0.0, -1.0)));
        assert_eq!(
            handle_drag_kind(s.start_kind, s.alt_broken),
            CornerKind::Corner
        );
        // Further dragging no longer moves the frozen opposite.
        s.dragged = Vec3::new(9.0, 0.0, 0.0);
        assert_eq!(live_opposite(&s), Some(Vec3::new(-2.0, 0.0, -1.0)));
    }

    #[test]
    fn alt_break_is_one_way() {
        let mut s = drag_state(CornerKind::Smooth);
        handle_observe_alt(&mut s, true);
        handle_observe_alt(&mut s, false); // Alt released — the break persists
        assert!(s.alt_broken);
        assert_eq!(
            handle_drag_kind(s.start_kind, s.alt_broken),
            CornerKind::Corner
        );
    }

    #[test]
    fn no_alt_keeps_the_anchor_kind() {
        let mut s = drag_state(CornerKind::Symmetric);
        handle_observe_alt(&mut s, false);
        assert!(!s.alt_broken);
        assert_eq!(
            handle_drag_kind(s.start_kind, s.alt_broken),
            CornerKind::Symmetric
        );
    }

    // ---- release commit fields ------------------------------------------

    #[test]
    fn release_fields_map_dragged_side_out() {
        let (hin, hout) = handle_release_fields(
            HandleSide::Out,
            Vec3::new(2.0, 0.0, 0.0),
            Some(Vec3::new(0.0, 0.0, 1.0)),
            CornerKind::Symmetric,
        );
        assert_eq!(hout, Some([2.0, 0.0, 0.0]));
        assert_eq!(hin, Some([-2.0, 0.0, 0.0]), "symmetric mirror");
    }

    #[test]
    fn release_fields_map_dragged_side_in() {
        let (hin, hout) = handle_release_fields(
            HandleSide::In,
            Vec3::new(-1.0, 0.0, 0.0),
            Some(Vec3::new(0.5, 0.0, 0.0)),
            CornerKind::Corner,
        );
        assert_eq!(hin, Some([-1.0, 0.0, 0.0]));
        assert_eq!(hout, Some([0.5, 0.0, 0.0]), "corner keeps the opposite");
    }

    #[test]
    fn handle_commit_stores_the_geometry_readback_smoothness() {
        // A hand-dragged symmetric handle of length 1 between 3 m gaps reads
        // back as 1 / (k·3) = 1.0 — the press-time scalar is NOT reused.
        let s = commit_smoothness(
            Some([-1.0, 0.0, 0.0]),
            Some([1.0, 0.0, 0.0]),
            Some(3.0),
            0.0,
        );
        assert!((s - 1.0).abs() < 1e-5, "got {s}");
        let s = commit_smoothness(None, Some([0.5, 0.0, 0.0]), Some(3.0), 0.9);
        assert!((s - 0.5).abs() < 1e-5, "got {s}");
    }

    #[test]
    fn handle_commit_without_neighbor_gap_keeps_press_time_smoothness() {
        assert_eq!(
            commit_smoothness(Some([-1.0, 0.0, 0.0]), Some([1.0, 0.0, 0.0]), None, 0.7),
            0.7
        );
    }

    // ---- pick radius vs vertex body (ratified Q7) ------------------------

    #[test]
    fn handle_pick_radius_matches_the_vertex_body_radius() {
        // Same 0.7 m test on both, so an overlapping handle + vertex marker is
        // decided purely by chain order (the handle system runs first).
        assert_eq!(
            PICK_RADIUS,
            super::super::path_gimbal_drag::MARKER_BODY_RADIUS
        );
    }

    #[test]
    fn pick_hits_within_radius_and_misses_outside() {
        let origin = Vec3::new(0.0, 5.0, 0.0);
        let dir = Vec3::NEG_Y;
        let near = (0, HandleSide::Out, Vec3::new(PICK_RADIUS - 0.01, 0.0, 0.0));
        let far = (1, HandleSide::In, Vec3::new(PICK_RADIUS + 0.01, 0.0, 0.0));
        let hit = pick_nearest_handle(origin, dir, PICK_RADIUS, [near, far]);
        assert_eq!(hit.map(|(i, s, _)| (i, s)), Some((0, HandleSide::Out)));
    }

    #[test]
    fn pick_prefers_the_nearest_along_the_ray() {
        let origin = Vec3::ZERO;
        let dir = Vec3::Z;
        let behind = (0, HandleSide::In, Vec3::new(0.0, 0.0, -2.0)); // skipped
        let far = (1, HandleSide::Out, Vec3::new(0.0, 0.0, 9.0));
        let near = (2, HandleSide::In, Vec3::new(0.0, 0.0, 3.0));
        let hit = pick_nearest_handle(origin, dir, PICK_RADIUS, [behind, far, near]);
        assert_eq!(hit.map(|(i, s, _)| (i, s)), Some((2, HandleSide::In)));
    }

    // ---- marker want-set --------------------------------------------------

    #[test]
    fn wanted_handle_markers_empty_when_not_editing() {
        let rows = vec![PathPointRow {
            position: [0.0; 3],
            handle_out: Some([1.0, 0.0, 0.0]),
            ..Default::default()
        }];
        assert!(wanted_handle_markers(false, &rows, MAX_POINT_MARKERS).is_empty());
    }

    #[test]
    fn wanted_handle_markers_one_per_some_side_at_anchor_plus_offset() {
        let rows = vec![
            PathPointRow {
                position: [0.0, 0.0, 0.0],
                ..Default::default()
            }, // handle-less corner: no markers
            PathPointRow {
                position: [10.0, 1.0, 0.0],
                handle_in: Some([-1.0, 0.0, 0.0]),
                handle_out: Some([1.0, 0.0, 0.5]),
                ..Default::default()
            },
            PathPointRow {
                position: [20.0, 0.0, 0.0],
                handle_out: Some([0.0, 0.0, 2.0]),
                ..Default::default()
            },
        ];
        let want = wanted_handle_markers(true, &rows, MAX_POINT_MARKERS);
        assert_eq!(
            want,
            vec![
                (1, HandleSide::In, Vec3::new(9.0, 1.0, 0.0)),
                (1, HandleSide::Out, Vec3::new(11.0, 1.0, 0.5)),
                (2, HandleSide::Out, Vec3::new(20.0, 0.0, 2.0)),
            ]
        );
    }

    #[test]
    fn wanted_handle_markers_respects_the_cap() {
        let row = PathPointRow {
            position: [0.0; 3],
            handle_out: Some([1.0, 0.0, 0.0]),
            ..Default::default()
        };
        let rows = vec![row.clone(), row.clone(), row];
        assert_eq!(wanted_handle_markers(true, &rows, 2).len(), 2);
    }
}
