//! Path-point viewport interaction: place / drag / annotate the edited track's
//! points. See `fe-ui/src/node_manager/AGENTS.md` §path-points.

use bevy::prelude::*;

use super::curve;
use super::dispatch::{resolve_operation, HitTarget, Operation};
use super::router::{sweep_stranded_drag, ClickArbiter, ClickPriority, PointerPhase};
use super::selection::project_selection;
use crate::actions::{UiAction, UiManager};
use crate::gis::{min_neighbor_gap_m, smoothness_readback, CornerKind, PathEditorState};
use crate::navigation_manager::NavigationManager;
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::toolbar::Tool;
use crate::plugin::{Billboard, ToolState};

/// Default name for a track auto-created by the first Pen click when none is
/// being edited (`pen_autocreate_track_20260713`). Renameable in the Paths tab.
const AUTO_TRACK_NAME: &str = "New Path";

/// Marker sphere for point `index` in the currently-edited track's point list.
#[derive(Component, Debug)]
pub struct PathPointMarker {
    pub index: usize,
}

/// Marker icon-quad edge length (world units). Billboarded flat quad (FR-2,
/// data_icons_20260713) reads as an icon vs. the old solid sphere; sized to
/// span roughly the old `Sphere(0.35)` diameter so pick feel is unchanged.
const MARKER_QUAD_SIZE: f32 = 0.7;
/// Manual ray/marker hit radius (world units) — shared with the FR-5 handle
/// pick (ratified Q7, same radius). See AGENTS.md §path-points.
pub(super) const PICK_RADIUS: f32 = 0.7;

/// Hard cap on point-marker entities (mirrors `MAX_STAMPS`): a huge imported
/// GPX track shows markers for its first N points only.
pub(super) const MAX_POINT_MARKERS: usize = 4096;

/// Marker count to keep spawned: the edited track's point count, capped.
fn marker_want(editing: bool, points: usize, cap: usize) -> usize {
    if editing {
        points.min(cap)
    } else {
        0
    }
}

/// In-progress drag of a path-point marker, `None` when idle. See
/// `node_manager/AGENTS.md` §path-points for the two drag modes (horizontal
/// ray-plane vs. Ctrl-held vertical height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPointDragState {
    /// Index of the dragged marker in the edited track's point list.
    pub index: usize,
    /// Height (Bevy Y) of the horizontal drag plane, captured at drag-start.
    pub plane_y: f32,
    /// Live marker height (Bevy Y): equals `plane_y` for a horizontal drag,
    /// raised/lowered by the Ctrl-held vertical mode (FR-1a).
    pub height_y: f32,
    /// Cursor screen-Y last frame, for the Ctrl-held vertical delta; `None`
    /// until the first Ctrl-hold frame so no jump is applied on modifier press.
    pub last_cursor_y: Option<f32>,
}

/// World-units of height (Bevy Y) per pixel of vertical cursor motion in the
/// Ctrl-held mode — matches `gimbal_interaction.rs`'s `* 0.01` feel.
const HEIGHT_DRAG_SENSITIVITY: f32 = 0.01;

/// Height (Bevy Y) delta for a Ctrl-held vertical drag: screen-Y grows
/// downward, so upward cursor motion (`cur < prev`) raises the point (+Y).
fn height_delta_from_cursor(prev_cursor_y: f32, cur_cursor_y: f32, sensitivity: f32) -> f32 {
    (prev_cursor_y - cur_cursor_y) * sensitivity
}

/// The in-progress path-point drag, `None` when idle.
#[derive(Resource, Default)]
pub struct PathPointDrag {
    pub active: Option<PathPointDragState>,
}

/// pen_curve_tool_20260722 (FR-4): drag distance below which a Pen release is
/// a plain click (petal-local meters).
pub(super) const PEN_DRAG_THRESHOLD_M: f32 = 0.15;

/// In-progress Pen press-drag placing a NEW anchor; resolved at Release by
/// `pen_release_decision`. See AGENTS.md §pen-tool.
#[derive(Clone, Debug, PartialEq)]
pub struct PenHandleDragState {
    /// Anchor position captured at press (y=0 plane hit, petal-local meters).
    pub anchor: [f32; 3],
    /// Live drag vector (current y=0 plane hit − anchor).
    pub drag_vec: [f32; 3],
    /// `true` once Alt has been observed during the gesture (press or hold).
    pub alt_seen: bool,
    /// Symmetric in-handle frozen at the FIRST mid-drag Alt observation
    /// (ratified Q8); stays `None` when Alt was held from the initial press.
    pub frozen_in: Option<[f32; 3]>,
    /// Edited track at Press; `None` = the auto-create was intended. Release
    /// drops the gesture when this no longer matches (see AGENTS.md §pen-tool).
    pub press_track_id: Option<String>,
    /// Active petal at Press — auto-create fires only while it still matches.
    pub press_petal_id: Option<String>,
}

/// The in-progress pen press-drag, `None` when idle — mirrors `PathPointDrag`.
#[derive(Resource, Default)]
pub struct PenHandleDrag {
    pub active: Option<PenHandleDragState>,
}

/// Record an Alt observation on a hold frame: the FIRST one freezes the
/// symmetric trailing in-handle at the current drag vector (ratified Q8).
fn pen_observe_alt(state: &mut PenHandleDragState, alt_held: bool) {
    if alt_held && !state.alt_seen {
        state.alt_seen = true;
        state.frozen_in = Some([-state.drag_vec[0], -state.drag_vec[1], -state.drag_vec[2]]);
    }
}

/// What a Pen release places — the pure FR-4 threshold/symmetry/Alt decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum PenReleaseDecision {
    /// Below-threshold click, Corner default: today's handle-less append.
    Corner,
    /// Below-threshold click, Smooth/Symmetric default: auto-derived handles.
    SmoothClick { kind: CornerKind },
    /// Press-drag without Alt: symmetric smooth anchor.
    SymmetricDrag {
        handle_out: [f32; 3],
        handle_in: [f32; 3],
    },
    /// Alt-drag (ratified Q8): combination anchor — independent out-handle,
    /// `handle_in` frozen at first mid-drag Alt (`None` when Alt from press).
    CombinationDrag {
        handle_out: [f32; 3],
        handle_in: Option<[f32; 3]>,
    },
}

/// Pure release decision: drag length vs `threshold`, Alt state, tool default.
fn pen_release_decision(
    drag_vec: [f32; 3],
    alt_seen: bool,
    frozen_in: Option<[f32; 3]>,
    default_kind: CornerKind,
    threshold: f32,
) -> PenReleaseDecision {
    let len =
        (drag_vec[0] * drag_vec[0] + drag_vec[1] * drag_vec[1] + drag_vec[2] * drag_vec[2]).sqrt();
    if len < threshold {
        return match default_kind {
            CornerKind::Corner => PenReleaseDecision::Corner,
            kind => PenReleaseDecision::SmoothClick { kind },
        };
    }
    if alt_seen {
        PenReleaseDecision::CombinationDrag {
            handle_out: drag_vec,
            handle_in: frozen_in,
        }
    } else {
        PenReleaseDecision::SymmetricDrag {
            handle_out: drag_vec,
            handle_in: [-drag_vec[0], -drag_vec[1], -drag_vec[2]],
        }
    }
}

/// Resolve a release decision into the anchor's bezier fields
/// `(handle_in, handle_out, corner, smoothness)`; `prev` is the previous
/// anchor's position for the SmoothClick tangent derivation. `None` = plain
/// legacy corner append (also the SmoothClick fallback with no tangent).
#[allow(clippy::type_complexity)]
fn pen_anchor_fields(
    decision: PenReleaseDecision,
    prev: Option<[f32; 3]>,
    anchor: [f32; 3],
) -> Option<(Option<[f32; 3]>, Option<[f32; 3]>, CornerKind, f32)> {
    match decision {
        PenReleaseDecision::Corner => None,
        PenReleaseDecision::SmoothClick { kind } => {
            // Appending at the end: no next neighbor yet; full smoothness so
            // the derived length matches the classic 1/3-gap tangent.
            let (hin, hout) = curve::derive_symmetric_handles(prev, anchor, None, 1.0)?;
            Some((Some(hin), Some(hout), kind, 1.0))
        }
        // Drag anchors store the geometry-readback smoothness (0.0 only when
        // no neighbor gap exists yet, e.g. the first anchor of a new track) so
        // the stored scalar tracks handle truth — see AGENTS.md §pen-tool.
        PenReleaseDecision::SymmetricDrag {
            handle_out,
            handle_in,
        } => {
            let s = smoothness_readback(
                Some(handle_in),
                Some(handle_out),
                min_neighbor_gap_m(prev, anchor, None),
            );
            Some((Some(handle_in), Some(handle_out), CornerKind::Symmetric, s))
        }
        PenReleaseDecision::CombinationDrag {
            handle_out,
            handle_in,
        } => {
            let s = smoothness_readback(
                handle_in,
                Some(handle_out),
                min_neighbor_gap_m(prev, anchor, None),
            );
            Some((handle_in, Some(handle_out), CornerKind::Corner, s))
        }
    }
}

/// What a completed pen gesture may do at Release, from the press-time vs
/// release-time context. A mid-hold stop/switch/petal-change drops the
/// gesture — a canceled edit session must never auto-create a track or append
/// to a context the press never saw. Pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PenGestureFate {
    /// Append onto the (unchanged) edited track.
    Append,
    /// Auto-create a track in the (unchanged) active petal.
    AutoCreate,
    /// Context changed between Press and Release — drop silently.
    Drop,
}

/// Pure Release gate: the press-time editing track must still be the current
/// one; auto-create (press-time `None`) additionally requires the same petal.
fn pen_gesture_fate(
    press_track: Option<&str>,
    cur_track: Option<&str>,
    press_petal: Option<&str>,
    cur_petal: Option<&str>,
) -> PenGestureFate {
    if press_track != cur_track {
        return PenGestureFate::Drop;
    }
    match cur_track {
        Some(_) => PenGestureFate::Append,
        None if press_petal.is_some() && press_petal == cur_petal => PenGestureFate::AutoCreate,
        None => PenGestureFate::Drop,
    }
}

/// Resolve a completed pen gesture: append onto the edited track, or stash the
/// FULL first-anchor payload (handles included — the FR-4 must-fix) under a
/// correlation id for the deferred auto-create echo. See AGENTS.md §pen-tool.
fn release_pen_gesture(
    pen: PenHandleDragState,
    default_kind: CornerKind,
    path_state: &mut PathEditorState,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
) {
    // Press-time context gate: a staged-Escape stop_editing, track delete,
    // petal change, or track switch mid-hold cancels the gesture outright.
    let fate = pen_gesture_fate(
        pen.press_track_id.as_deref(),
        path_state.editing_track_id.as_deref(),
        pen.press_petal_id.as_deref(),
        nav.active_petal_id.as_deref(),
    );
    if fate == PenGestureFate::Drop {
        return;
    }
    let decision = pen_release_decision(
        pen.drag_vec,
        pen.alt_seen,
        pen.frozen_in,
        default_kind,
        PEN_DRAG_THRESHOLD_M,
    );
    let prev = path_state.points.last().map(|p| p.position);
    if fate == PenGestureFate::Append {
        let Some(track_id) = pen.press_track_id else {
            return; // unreachable: Append implies a press-time track
        };
        match pen_anchor_fields(decision, prev, pen.anchor) {
            Some((handle_in, handle_out, corner, smoothness)) => {
                ui_mgr.push_action(UiAction::PathAppendSmoothPoint {
                    track_node_id: track_id,
                    position: pen.anchor,
                    handle_in,
                    handle_out,
                    corner,
                    smoothness,
                });
            }
            None => {
                ui_mgr.push_action(UiAction::PathAppendPoint {
                    track_node_id: track_id,
                    position: pen.anchor,
                });
            }
        }
        return;
    }
    // AutoCreate: no track at Press AND still none (same petal) → auto-create
    // one and stash this gesture's anchor (+ handles) under a fe-ui-generated
    // correlation id; the append is deferred until the new track's
    // `NodeCreated` echoes that id (`pen_autocreate_track_20260713` + FR-4).
    if path_state.has_pending_pen_create() {
        return;
    }
    let Some(petal_id) = nav.active_petal_id.clone() else {
        return; // unreachable: AutoCreate requires a (matching) active petal
    };
    let (handle_in, handle_out, corner, smoothness) = pen_anchor_fields(decision, None, pen.anchor)
        .unwrap_or((None, None, CornerKind::Corner, 0.0));
    let correlation_id = crate::gis::next_pen_correlation_id();
    path_state.pending_pen_create = Some(crate::gis::PendingPenCreate {
        correlation_id: correlation_id.clone(),
        first_point: pen.anchor,
        handle_in,
        handle_out,
        corner,
        smoothness,
    });
    ui_mgr.push_action(UiAction::PathCreateTrack {
        petal_id,
        name: AUTO_TRACK_NAME.to_string(),
        correlation_id: Some(correlation_id),
    });
}

/// Keeps one `PathPointMarker` sphere per edited-track point; despawns all when
/// not editing. Runs before the interaction system so picks see current markers.
pub(super) fn sync_path_point_markers(
    mut path_state: ResMut<PathEditorState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut markers: Query<(
        Entity,
        &PathPointMarker,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    mut mesh_handle: Local<Option<Handle<Mesh>>>,
    mut mat_handle: Local<Option<Handle<StandardMaterial>>>,
    mut hl_handle: Local<Option<Handle<StandardMaterial>>>,
) {
    let editing = path_state.editing_track_id.is_some();
    let want = marker_want(editing, path_state.points.len(), MAX_POINT_MARKERS);
    let have = markers.iter().count();

    // Shared handles (allocated once): the icon-quad mesh + the normal (yellow)
    // and FR-2/FR-3 highlight (orange) materials. `Rectangle` lies in local XY
    // (+Z normal); the `Billboard` tag keeps it camera-facing (data_icons).
    let mesh = mesh_handle
        .get_or_insert_with(|| meshes.add(Rectangle::new(MARKER_QUAD_SIZE, MARKER_QUAD_SIZE)))
        .clone();
    let normal_mat = mat_handle
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.85, 0.2),
                unlit: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    let highlight_mat = hl_handle
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                // FR-2/FR-3: bright orange for the selected vertex, or the two
                // ends of the selected segment.
                base_color: Color::srgb(1.0, 0.5, 0.05),
                unlit: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();

    if want != have {
        // Saturation warn only on rebuild — not every frame while editing.
        if editing && path_state.points.len() > MAX_POINT_MARKERS {
            bevy::log::warn!(
                "path-point markers saturated: showing {} of {} points (cap {})",
                want,
                path_state.points.len(),
                MAX_POINT_MARKERS
            );
        }
        // Count changed → a cached selection index may be stale; drop it.
        path_state.clear_path_selection();
        // Despawn-all + respawn: point counts are small, so a per-change
        // rebuild is cheaper than index bookkeeping when a mid-list point goes.
        for (entity, _, _) in markers.iter() {
            commands.entity(entity).despawn();
        }
        if want == 0 {
            return;
        }
        for (i, point) in path_state.points.iter().take(want).enumerate() {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(normal_mat.clone()),
                Transform::from_xyz(point.position[0], point.position[1], point.position[2]),
                Name::new(format!("PathPoint {i}")),
                PathPointMarker { index: i },
                Billboard,
            ));
        }
        return;
    }

    // FR-2/FR-3 highlight pass: recolor markers in place (no respawn) so the
    // selected vertex — or the two ends of the selected segment — stands out.
    let sel_point = path_state.selected_point;
    let sel_seg = path_state.selected_segment;
    for (_, marker, mut mat) in markers.iter_mut() {
        let highlighted = Some(marker.index) == sel_point
            || sel_seg.is_some_and(|s| marker.index == s || marker.index == s + 1);
        let want_mat = if highlighted {
            &highlight_mat
        } else {
            &normal_mat
        };
        if mat.0.id() != want_mat.id() {
            mat.0 = want_mat.clone();
        }
    }
}

/// Nearest marker index under `ray`, if any (along-ray + radius test, mirrors
/// `viewport_pick.rs`; no Bevy picking).
fn pick_marker(
    ray: &Ray3d,
    markers: &Query<(&GlobalTransform, &PathPointMarker)>,
) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (g_tx, marker) in markers.iter() {
        let pos = g_tx.translation();
        let along = (pos - ray.origin).dot(*ray.direction);
        if along < 0.0 {
            continue;
        }
        let closest = ray.origin + *ray.direction * along;
        if (pos - closest).length() < PICK_RADIUS && best.is_none_or(|b| along < b.1) {
            best = Some((marker.index, along));
        }
    }
    best
}

/// Intersect `ray` with the horizontal plane `y = plane_y`; `None` if parallel
/// or behind the origin. Shared with the FR-5 handle drag.
pub(super) fn ray_plane_y(ray: &Ray3d, plane_y: f32) -> Option<Vec3> {
    let dir_y = ray.direction.y;
    if dir_y.abs() < 1e-6 {
        return None;
    }
    let t = (plane_y - ray.origin.y) / dir_y;
    if t < 0.0 {
        return None;
    }
    Some(ray.origin + *ray.direction * t)
}

/// Path-point interaction: Pen-tool Press/Hold/Release gesture (click = corner
/// anchor, press-drag = smooth anchor, Alt-drag = Q8 combination —
/// pen_curve_tool_20260722 FR-4), drag-to-move (Ctrl-held → vertical height,
/// FR-1a), modifier-click-to-annotate. Claims `PathMarker` on marker pick and
/// `PathPlace` on the pen press. See `node_manager/AGENTS.md` §pen-tool.
pub(super) fn handle_path_point_interaction(
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut path_state: ResMut<PathEditorState>,
    mut drag: ResMut<PathPointDrag>,
    mut pen_drag: ResMut<PenHandleDrag>,
    tool: Res<ToolState>,
    tool_panel: Res<ToolPanelState>,
    nav: Res<NavigationManager>,
    mut ui_mgr: ResMut<UiManager>,
    mut marker_tx: Query<(&mut Transform, &PathPointMarker)>,
    marker_pick: Query<(&GlobalTransform, &PathPointMarker)>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Stranded-gesture sweep: a Hover frame (button up) with a live drag means
    // its Release was missed — drop it before it can replay on a later click.
    let phase = arbiter.phase();
    sweep_stranded_drag(phase, &mut drag.active);
    sweep_stranded_drag(phase, &mut pen_drag.active);

    // Act while the Pen tool is active (new-point placement, incl. the no-track
    // auto-create case), OR — path_interaction_20260716 (FR-2) — while a track
    // is open for editing in the Select tool so its vertex markers are
    // pick/draggable like Illustrator's pen; OR while a marker/pen drag is in
    // flight. Markers only exist while editing, so `markers_editable` gates the
    // Select case. Pen-append stays Pen-only (guarded at the append branch).
    // Move/Rotate/Scale still yield to the gimbal (their handler runs first).
    let pen_active = tool.active_tool == Tool::Pen;
    let markers_editable = path_state.editing_track_id.is_some()
        && matches!(tool.active_tool, Tool::Select | Tool::Pen);
    if !pen_active && !markers_editable && drag.active.is_none() && pen_drag.active.is_none() {
        drag.active = None;
        return;
    }
    // `None` while no track is being edited — the Pen no-track branch below
    // auto-creates one (`pen_autocreate_track_20260713`); marker drag/annotate
    // and the append branch use `track_id` only when it's `Some`.
    let editing_track_id = path_state.editing_track_id.clone();

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else {
        return;
    };

    // Release → commit gestures. A marker drag commits as a MovePoint (no
    // index churn; the committed y is read from the marker `Transform`, so a
    // Ctrl-raised height flows through automatically, FR-1a). A pen press-drag
    // resolves via the pure threshold decision (pen_curve_tool_20260722 FR-4).
    if arbiter.phase() == PointerPhase::Release {
        if let Some(state) = drag.active.take() {
            // A drag can only start while editing a track, so `editing_track_id`
            // is `Some` here.
            if let (Some(track_id), Some((tx, _))) = (
                editing_track_id.clone(),
                marker_tx.iter().find(|(_, m)| m.index == state.index),
            ) {
                let p = tx.translation;
                ui_mgr.push_action(UiAction::PathMovePoint {
                    track_node_id: track_id,
                    index: state.index,
                    position: [p.x, p.y, p.z],
                });
            }
        }
        if let Some(pen) = pen_drag.active.take() {
            release_pen_gesture(
                pen,
                tool_panel.pen_new_anchor_kind,
                &mut path_state,
                &nav,
                &mut ui_mgr,
            );
        }
        return;
    }

    // Hold → update the dragged marker (Ctrl-held: raise/lower height, Bevy Y,
    // by vertical cursor delta, decoupled from the ray-plane hit, FR-1a;
    // otherwise reproject through the horizontal `plane_y`) — or update the
    // in-flight pen gesture's drag vector + Alt freeze (FR-4/Q8).
    if arbiter.phase() == PointerPhase::Hold {
        if let Some(mut state) = drag.active {
            let Some(cursor_pos) = cursor else { return };
            let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
            if ctrl {
                if let Some(prev_y) = state.last_cursor_y {
                    state.height_y +=
                        height_delta_from_cursor(prev_y, cursor_pos.y, HEIGHT_DRAG_SENSITIVITY);
                }
                state.last_cursor_y = Some(cursor_pos.y);
                for (mut tx, marker) in marker_tx.iter_mut() {
                    if marker.index == state.index {
                        tx.translation.y = state.height_y;
                    }
                }
            } else {
                // Reset the vertical anchor so re-pressing Ctrl doesn't apply a
                // stale delta accumulated across the gap.
                state.last_cursor_y = None;
                let Ok(ray) = camera.viewport_to_world(cam_tx, cursor_pos) else {
                    return;
                };
                if let Some(hit) = ray_plane_y(&ray, state.plane_y) {
                    for (mut tx, marker) in marker_tx.iter_mut() {
                        if marker.index == state.index {
                            // Keep any Ctrl-raised height; only x/z track the ray.
                            tx.translation.x = hit.x;
                            tx.translation.z = hit.z;
                            tx.translation.y = state.height_y;
                        }
                    }
                }
            }
            drag.active = Some(state);
            return;
        }
        if let Some(pen) = pen_drag.active.as_mut() {
            let Some(cursor_pos) = cursor else { return };
            let Ok(ray) = camera.viewport_to_world(cam_tx, cursor_pos) else {
                return;
            };
            if let Some(hit) = ray_plane_y(&ray, 0.0) {
                pen.drag_vec = [
                    hit.x - pen.anchor[0],
                    hit.y - pen.anchor[1],
                    hit.z - pen.anchor[2],
                ];
            }
            pen_observe_alt(
                pen,
                keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
            );
            return;
        }
    }

    // Press → pick a marker (drag / annotate) or place a new point. The arbiter
    // has already applied egui + viewport gating and computed the ray.
    if !arbiter.is_fresh_press() || !arbiter.is_available() {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };

    // Object-aware routing (no-bypass): both claim branches below confirm their
    // verb against the FR-2 table before claiming. Projected from Authority B
    // (the edited track); node selection is immaterial to marker/place resolution.
    let kind = project_selection(
        None,
        editing_track_id.as_deref(),
        path_state.selected_point,
        path_state.selected_segment,
    );

    if let Some((index, _)) = pick_marker(&ray, &marker_pick) {
        // A marker hit resolves to `SelectVertex` (dispatch.rs), tool- and
        // selection-independent — confirm the verb before claiming.
        if !matches!(
            resolve_operation(
                tool.active_tool,
                &kind,
                HitTarget::PathVertex { idx: index }
            ),
            Operation::SelectVertex { .. }
        ) {
            return;
        }
        // A marker pick claims `PathMarker` (Pen mode, Select-while-editing per
        // FR-2, or while a drag is already active) — the guard above returns in
        // Move/Rotate/Scale so the gimbal keeps the click there.
        if !arbiter.claim(ClickPriority::PathMarker) {
            return;
        }
        // FR-2: selecting a vertex highlights it and clears any segment
        // selection (the two are mutually exclusive).
        path_state.selected_point = Some(index);
        path_state.selected_segment = None;
        let modifier = keys.pressed(KeyCode::ShiftLeft)
            || keys.pressed(KeyCode::ShiftRight)
            || keys.pressed(KeyCode::AltLeft)
            || keys.pressed(KeyCode::AltRight);
        if modifier {
            // Modifier-click → open the inline annotation form for this point.
            path_state.open_annotate_form(index);
        } else {
            // Plain click → begin dragging this point on its current y-plane.
            // Hold Ctrl during the drag to raise/lower height instead (FR-1a).
            let plane_y = marker_pick
                .iter()
                .find(|(_, m)| m.index == index)
                .map(|(g, _)| g.translation().y)
                .unwrap_or(0.0);
            drag.active = Some(PathPointDragState {
                index,
                plane_y,
                height_y: plane_y,
                last_cursor_y: None,
            });
        }
        return;
    }

    // Empty press on terrain while the Pen tool is active → begin a pen
    // press-drag gesture at the Y=0 plane; the anchor kind (corner vs smooth
    // vs Q8 combination) is decided at Release (pen_curve_tool_20260722 FR-4).
    // Claims `PathPlace` only in Pen mode, so Select-mode empty clicks yield
    // to node selection — see AGENTS.md §pen-tool. Routed (no-bypass): an
    // empty-terrain press resolves to `PlacePathPoint` only in Pen (dispatch.rs);
    // every other tool resolves to `Deselect` and yields the frame to NodePick.
    if !matches!(
        resolve_operation(tool.active_tool, &kind, HitTarget::Empty),
        Operation::PlacePathPoint
    ) {
        return;
    }
    // Claim `PathPlace` first (even in the no-track auto-create case) so
    // `viewport_pick`'s `NodePick` doesn't also fire on this frame's click.
    // The claim must stay on the PRESS frame — the arbiter resets its owner
    // each frame, and the Release frame is never contested (other consumers
    // act on `is_fresh_press` only).
    if !arbiter.claim(ClickPriority::PathPlace) {
        return;
    }
    let Some(hit) = ray_plane_y(&ray, 0.0) else {
        return;
    };
    // The no-track gates stay press-time (mirroring the old append-time
    // behavior) so a doomed gesture never starts: a rapid press while the
    // auto-create round-trips is dropped (`pen_autocreate_track_20260713`),
    // and no active petal means nowhere to put a track.
    if editing_track_id.is_none() {
        if path_state.has_pending_pen_create() {
            return;
        }
        if nav.active_petal_id.is_none() {
            bevy::log::info!("Pen: no active petal — select a petal before drawing a path");
            return;
        }
    }
    pen_drag.active = Some(PenHandleDragState {
        anchor: [hit.x, hit.y, hit.z],
        drag_vec: [0.0; 3],
        // Alt already down at press ⇒ Q8's "held from the initial press" case:
        // `alt_seen` without a frozen in-handle.
        alt_seen: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        frozen_in: None,
        // Press-time context: Release commits only when it still matches.
        press_track_id: editing_track_id.clone(),
        press_petal_id: nav.active_petal_id.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_want_zero_when_not_editing() {
        assert_eq!(marker_want(false, 500, MAX_POINT_MARKERS), 0);
        assert_eq!(marker_want(false, 0, MAX_POINT_MARKERS), 0);
    }

    #[test]
    fn marker_want_passes_through_under_cap() {
        assert_eq!(marker_want(true, 12, MAX_POINT_MARKERS), 12);
        assert_eq!(
            marker_want(true, MAX_POINT_MARKERS, MAX_POINT_MARKERS),
            MAX_POINT_MARKERS
        );
    }

    #[test]
    fn marker_want_saturates_at_cap() {
        assert_eq!(
            marker_want(true, 500_000, MAX_POINT_MARKERS),
            MAX_POINT_MARKERS
        );
        assert_eq!(
            marker_want(true, usize::MAX, MAX_POINT_MARKERS),
            MAX_POINT_MARKERS
        );
    }

    #[test]
    fn upward_cursor_motion_raises_height() {
        // Screen-Y grows downward, so a smaller cur than prev is upward → +Y.
        let d = height_delta_from_cursor(200.0, 150.0, HEIGHT_DRAG_SENSITIVITY);
        assert!(d > 0.0, "upward drag should raise height, got {d}");
        assert!((d - 0.5).abs() < 1e-6, "50px * 0.01 = 0.5, got {d}");
    }

    #[test]
    fn downward_cursor_motion_lowers_height() {
        let d = height_delta_from_cursor(150.0, 200.0, HEIGHT_DRAG_SENSITIVITY);
        assert!(d < 0.0, "downward drag should lower height, got {d}");
        assert!((d + 0.5).abs() < 1e-6, "-50px * 0.01 = -0.5, got {d}");
    }

    #[test]
    fn zero_cursor_motion_is_no_height_change() {
        assert_eq!(
            height_delta_from_cursor(120.0, 120.0, HEIGHT_DRAG_SENSITIVITY),
            0.0
        );
    }

    #[test]
    fn accumulated_height_matches_summed_deltas() {
        // Two upward steps accumulate onto the starting plane_y.
        let mut state = PathPointDragState {
            index: 0,
            plane_y: 2.0,
            height_y: 2.0,
            last_cursor_y: Some(300.0),
        };
        state.height_y += height_delta_from_cursor(300.0, 250.0, HEIGHT_DRAG_SENSITIVITY);
        state.height_y += height_delta_from_cursor(250.0, 200.0, HEIGHT_DRAG_SENSITIVITY);
        // 2.0 + 0.5 + 0.5 = 3.0.
        assert!(
            (state.height_y - 3.0).abs() < 1e-6,
            "got {}",
            state.height_y
        );
    }

    // ---- FR-4 pure release-decision tests (pen_curve_tool_20260722) -----

    #[test]
    fn release_below_threshold_defaults_to_corner_click() {
        let d = pen_release_decision(
            [0.1, 0.0, 0.05],
            false,
            None,
            CornerKind::Corner,
            PEN_DRAG_THRESHOLD_M,
        );
        assert_eq!(d, PenReleaseDecision::Corner);
    }

    #[test]
    fn release_below_threshold_with_smooth_default_is_smooth_click() {
        // The tool-level default kind (ToolPanelState.pen_new_anchor_kind)
        // upgrades a plain click to an auto-derived smooth anchor.
        let d = pen_release_decision(
            [0.0; 3],
            false,
            None,
            CornerKind::Smooth,
            PEN_DRAG_THRESHOLD_M,
        );
        assert_eq!(
            d,
            PenReleaseDecision::SmoothClick {
                kind: CornerKind::Smooth
            }
        );
        let d = pen_release_decision(
            [0.0; 3],
            false,
            None,
            CornerKind::Symmetric,
            PEN_DRAG_THRESHOLD_M,
        );
        assert_eq!(
            d,
            PenReleaseDecision::SmoothClick {
                kind: CornerKind::Symmetric
            }
        );
    }

    #[test]
    fn release_at_threshold_is_symmetric_drag() {
        // Exactly ON the threshold counts as a drag (below ⇒ click, at/above
        // ⇒ smooth) — the FR-4 boundary case.
        let d = pen_release_decision(
            [PEN_DRAG_THRESHOLD_M, 0.0, 0.0],
            false,
            None,
            CornerKind::Corner,
            PEN_DRAG_THRESHOLD_M,
        );
        assert_eq!(
            d,
            PenReleaseDecision::SymmetricDrag {
                handle_out: [PEN_DRAG_THRESHOLD_M, 0.0, 0.0],
                handle_in: [-PEN_DRAG_THRESHOLD_M, 0.0, 0.0],
            }
        );
    }

    #[test]
    fn drag_without_alt_mirrors_the_in_handle() {
        let d = pen_release_decision(
            [2.0, 0.0, 1.0],
            false,
            None,
            CornerKind::Corner,
            PEN_DRAG_THRESHOLD_M,
        );
        match d {
            PenReleaseDecision::SymmetricDrag {
                handle_out,
                handle_in,
            } => {
                assert_eq!(handle_out, [2.0, 0.0, 1.0]);
                assert_eq!(handle_in, [-2.0, 0.0, -1.0]);
            }
            other => panic!("expected SymmetricDrag, got {other:?}"),
        }
    }

    /// Pen drag state with no press-time context, for the pure decision math.
    fn pen_state(drag_vec: [f32; 3], alt_seen: bool) -> PenHandleDragState {
        PenHandleDragState {
            anchor: [0.0; 3],
            drag_vec,
            alt_seen,
            frozen_in: None,
            press_track_id: None,
            press_petal_id: None,
        }
    }

    #[test]
    fn alt_frozen_mid_drag_yields_combination_with_frozen_in() {
        // Q8: Alt first observed mid-drag freezes −drag at that moment; the
        // out-handle keeps tracking the cursor afterwards.
        let mut pen = pen_state([1.0, 0.0, 0.0], false);
        pen_observe_alt(&mut pen, true); // Alt first seen at drag (1,0,0)
        pen.drag_vec = [3.0, 0.0, 1.0]; // keeps dragging after the freeze
        pen_observe_alt(&mut pen, true); // later Alt frames don't re-freeze
        let d = pen_release_decision(
            pen.drag_vec,
            pen.alt_seen,
            pen.frozen_in,
            CornerKind::Corner,
            PEN_DRAG_THRESHOLD_M,
        );
        assert_eq!(
            d,
            PenReleaseDecision::CombinationDrag {
                handle_out: [3.0, 0.0, 1.0],
                handle_in: Some([-1.0, 0.0, 0.0]),
            }
        );
    }

    #[test]
    fn alt_from_press_yields_combination_with_no_in_handle() {
        // Q8: Alt held from the initial press ⇒ handle_in None (the press
        // branch seeds alt_seen without a frozen value).
        let mut pen = pen_state([0.0; 3], true);
        pen.drag_vec = [1.0, 0.0, 0.0];
        pen_observe_alt(&mut pen, true); // never re-freezes once alt_seen
        let d = pen_release_decision(
            pen.drag_vec,
            pen.alt_seen,
            pen.frozen_in,
            CornerKind::Corner,
            PEN_DRAG_THRESHOLD_M,
        );
        assert_eq!(
            d,
            PenReleaseDecision::CombinationDrag {
                handle_out: [1.0, 0.0, 0.0],
                handle_in: None,
            }
        );
    }

    #[test]
    fn alt_release_mid_drag_does_not_unfreeze() {
        let mut pen = pen_state([1.0, 0.0, 0.0], false);
        pen_observe_alt(&mut pen, true);
        pen_observe_alt(&mut pen, false); // Alt released — the freeze persists
        assert!(pen.alt_seen);
        assert_eq!(pen.frozen_in, Some([-1.0, 0.0, 0.0]));
    }

    #[test]
    fn pen_anchor_fields_smooth_click_derives_from_prev_tangent() {
        // A Smooth click with a previous anchor derives collinear handles
        // (3 m gap · 1/3 = 1 m out-handle along the trailing tangent).
        let fields = pen_anchor_fields(
            PenReleaseDecision::SmoothClick {
                kind: CornerKind::Smooth,
            },
            Some([0.0, 0.0, 0.0]),
            [3.0, 0.0, 0.0],
        );
        let (hin, hout, corner, smoothness) = fields.expect("neighbor tangent derivable");
        assert_eq!(corner, CornerKind::Smooth);
        assert_eq!(smoothness, 1.0);
        let hout = hout.unwrap();
        assert!((hout[0] - 1.0).abs() < 1e-5, "got {}", hout[0]);
        assert_eq!(hin.unwrap()[0], -hout[0]);
    }

    #[test]
    fn pen_anchor_fields_first_anchor_smooth_click_falls_back_to_corner() {
        // No neighbors at all (first anchor of a new track) ⇒ no tangent ⇒
        // the plain legacy corner append.
        assert!(pen_anchor_fields(
            PenReleaseDecision::SmoothClick {
                kind: CornerKind::Smooth
            },
            None,
            [1.0, 0.0, 1.0],
        )
        .is_none());
    }

    #[test]
    fn pen_anchor_fields_corner_is_legacy_append() {
        assert!(
            pen_anchor_fields(PenReleaseDecision::Corner, Some([0.0; 3]), [1.0, 0.0, 0.0])
                .is_none()
        );
    }

    #[test]
    fn pen_anchor_fields_combination_keeps_optional_in_handle() {
        // Q8 combination: independent out-handle, classification Corner.
        let (hin, hout, corner, _) = pen_anchor_fields(
            PenReleaseDecision::CombinationDrag {
                handle_out: [2.0, 0.0, 0.0],
                handle_in: Some([-0.5, 0.0, 0.0]),
            },
            Some([0.0; 3]),
            [1.0, 0.0, 0.0],
        )
        .unwrap();
        assert_eq!(hin, Some([-0.5, 0.0, 0.0]));
        assert_eq!(hout, Some([2.0, 0.0, 0.0]));
        assert_eq!(corner, CornerKind::Corner);
    }

    #[test]
    fn pen_anchor_fields_drag_commit_stores_readback_smoothness() {
        // A drag-created anchor must not store 0.0 while carrying real
        // handles: smoothness = |handle_out| / (k · gap-to-prev), clamped.
        let (_, _, _, s) = pen_anchor_fields(
            PenReleaseDecision::SymmetricDrag {
                handle_out: [1.0, 0.0, 0.0],
                handle_in: [-1.0, 0.0, 0.0],
            },
            Some([0.0; 3]),
            [3.0, 0.0, 0.0], // gap 3 → k·gap = 1 → s = 1.0
        )
        .unwrap();
        assert!((s - 1.0).abs() < 1e-5, "got {s}");
        let (_, _, _, s) = pen_anchor_fields(
            PenReleaseDecision::CombinationDrag {
                handle_out: [0.5, 0.0, 0.0],
                handle_in: None,
            },
            Some([0.0; 3]),
            [3.0, 0.0, 0.0],
        )
        .unwrap();
        assert!((s - 0.5).abs() < 1e-5, "got {s}");
    }

    #[test]
    fn pen_anchor_fields_first_drag_anchor_falls_back_to_zero_smoothness() {
        // First anchor of a new track: no neighbor gap yet — keep the 0.0
        // default (the readback becomes computable once a neighbor exists).
        let (_, _, _, s) = pen_anchor_fields(
            PenReleaseDecision::SymmetricDrag {
                handle_out: [1.0, 0.0, 0.0],
                handle_in: [-1.0, 0.0, 0.0],
            },
            None,
            [3.0, 0.0, 0.0],
        )
        .unwrap();
        assert_eq!(s, 0.0);
    }

    // ---- press-time context gate (canceled-gesture findings) -------------

    #[test]
    fn escape_mid_drag_does_not_auto_create() {
        // Editing track canceled (staged Escape / Back / delete) mid-hold:
        // the release must NOT fall through into the auto-create branch.
        assert_eq!(
            pen_gesture_fate(Some("track:1"), None, Some("petal:1"), Some("petal:1")),
            PenGestureFate::Drop
        );
    }

    #[test]
    fn editing_track_changed_mid_gesture_drops_the_anchor() {
        assert_eq!(
            pen_gesture_fate(Some("track:1"), Some("track:2"), None, None),
            PenGestureFate::Drop
        );
        // A track opened mid-hold must not receive a no-track press either.
        assert_eq!(
            pen_gesture_fate(None, Some("track:2"), Some("petal:1"), Some("petal:1")),
            PenGestureFate::Drop
        );
    }

    #[test]
    fn unchanged_context_appends_or_auto_creates() {
        assert_eq!(
            pen_gesture_fate(Some("track:1"), Some("track:1"), None, Some("petal:1")),
            PenGestureFate::Append
        );
        assert_eq!(
            pen_gesture_fate(None, None, Some("petal:1"), Some("petal:1")),
            PenGestureFate::AutoCreate
        );
    }

    #[test]
    fn petal_changed_mid_gesture_drops_the_auto_create() {
        assert_eq!(
            pen_gesture_fate(None, None, Some("petal:1"), Some("petal:2")),
            PenGestureFate::Drop
        );
        assert_eq!(
            pen_gesture_fate(None, None, Some("petal:1"), None),
            PenGestureFate::Drop
        );
    }
}
