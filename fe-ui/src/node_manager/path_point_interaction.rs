//! Path-point viewport interaction: place / drag / annotate the edited track's
//! points. See `fe-ui/src/node_manager/AGENTS.md` §path-points.

use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority};
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
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
/// Manual ray/marker hit radius (world units) — see AGENTS.md §path-points.
const PICK_RADIUS: f32 = 0.7;

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

/// Keeps one `PathPointMarker` sphere per edited-track point; despawns all when
/// not editing. Runs before the interaction system so picks see current markers.
pub(super) fn sync_path_point_markers(
    path_state: Res<PathEditorState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    markers: Query<(Entity, &PathPointMarker)>,
    mut mesh_handle: Local<Option<Handle<Mesh>>>,
    mut mat_handle: Local<Option<Handle<StandardMaterial>>>,
) {
    let editing = path_state.editing_track_id.is_some();
    let want = if editing { path_state.points.len() } else { 0 };
    let have = markers.iter().count();

    // Count matches: positions stay live via the drag system + count-change rebuild.
    if want == have {
        return;
    }

    // Despawn-all + respawn: point counts are small, so a per-change rebuild is
    // cheaper than index bookkeeping when a mid-list point is removed.
    for (entity, _) in markers.iter() {
        commands.entity(entity).despawn();
    }
    if want == 0 {
        return;
    }

    // FR-2 (data_icons_20260713): a flat, camera-facing icon quad instead of a
    // solid sphere. `Rectangle` lies in local XY (+Z normal); the `Billboard`
    // tag + `billboard_face_camera` keep it turned toward the viewport. Unlit +
    // double-sided so it reads at any orbit angle before the first face frame.
    let mesh = mesh_handle
        .get_or_insert_with(|| meshes.add(Rectangle::new(MARKER_QUAD_SIZE, MARKER_QUAD_SIZE)))
        .clone();
    let material = mat_handle
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.85, 0.2),
                unlit: true,
                cull_mode: None,
                ..default()
            })
        })
        .clone();

    for (i, point) in path_state.points.iter().enumerate() {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(point.position[0], point.position[1], point.position[2]),
            Name::new(format!("PathPoint {i}")),
            PathPointMarker { index: i },
            Billboard,
        ));
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
/// or behind the origin.
fn ray_plane_y(ray: &Ray3d, plane_y: f32) -> Option<Vec3> {
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

/// Path-point interaction: Pen-tool click-to-place, drag-to-move (Ctrl-held →
/// vertical height, FR-1a), modifier-click-to-annotate. Claims `PathMarker` on
/// marker pick and `PathPlace` on pen append. See `node_manager/AGENTS.md`
/// §pen-tool.
pub(super) fn handle_path_point_interaction(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut path_state: ResMut<PathEditorState>,
    mut drag: ResMut<PathPointDrag>,
    tool: Res<ToolState>,
    nav: Res<NavigationManager>,
    mut ui_mgr: ResMut<UiManager>,
    mut marker_tx: Query<(&mut Transform, &PathPointMarker)>,
    marker_pick: Query<(&GlobalTransform, &PathPointMarker)>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Only act while the Pen tool is active (new-point placement, incl. the
    // no-track auto-create case) or a marker drag is in flight. In
    // Select/Move/Rotate/Scale with no active drag, node selection + gimbal
    // keep the click. See `AGENTS.md` §pen-tool.
    let pen_active = tool.active_tool == Tool::Pen;
    if !pen_active && drag.active.is_none() {
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

    // Release → commit the drag as a MovePoint (no index churn). The committed
    // y is read from the marker `Transform`, so a Ctrl-raised height flows
    // through automatically (FR-1a).
    if mouse_button.just_released(MouseButton::Left) {
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
        return;
    }

    // Hold → update the dragged marker. Ctrl-held: raise/lower height (Bevy Y)
    // by vertical cursor delta, decoupled from the ray-plane hit (FR-1a).
    // Otherwise: reproject through the horizontal `plane_y` (existing behavior).
    if mouse_button.pressed(MouseButton::Left) {
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
    }

    // Press → pick a marker (drag / annotate) or place a new point. The arbiter
    // has already applied egui + viewport gating and computed the ray.
    if !arbiter.is_fresh_press() || !arbiter.is_available() {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };

    if let Some((index, _)) = pick_marker(&ray, &marker_pick) {
        // A marker pick claims `PathMarker`, but only reachable in Pen mode (or
        // while a drag is already active) — the guard above returns before this
        // in Select/Move/Rotate/Scale with no active drag, so node selection +
        // gimbal keep the click there.
        if !arbiter.claim(ClickPriority::PathMarker) {
            return;
        }
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

    // Empty click on terrain while the Pen tool is active → append a point at
    // the Y=0 plane. Claims `PathPlace` only in Pen mode, so Select-mode empty
    // clicks yield to node selection — see AGENTS.md §pen-tool.
    if tool.active_tool != Tool::Pen {
        return;
    }
    // Claim `PathPlace` first (even in the no-track auto-create case) so
    // `viewport_pick`'s `NodePick` doesn't also fire on this frame's click.
    if !arbiter.claim(ClickPriority::PathPlace) {
        return;
    }
    let Some(hit) = ray_plane_y(&ray, 0.0) else {
        return;
    };
    if let Some(track_id) = editing_track_id {
        // A track is being edited → append normally.
        ui_mgr.push_action(UiAction::PathAppendPoint {
            track_node_id: track_id,
            position: [hit.x, hit.y, hit.z],
        });
    } else if !path_state.has_pending_pen_create() {
        // No track yet → auto-create one in the active petal and stash this
        // click's world position under a fe-ui-generated correlation id; the
        // append is deferred until the new track's `NodeCreated` echoes that id
        // (`pen_autocreate_track_20260713`, FR-1/FR-2). Guarded on
        // `!has_pending_pen_create()` so a rapid second click before the create
        // round-trips doesn't queue a second track.
        let Some(petal_id) = nav.active_petal_id.clone() else {
            // No active petal → nowhere to put a track; keep the no-op (FR-4).
            bevy::log::info!("Pen: no active petal — select a petal before drawing a path");
            return;
        };
        let correlation_id = crate::gis::next_pen_correlation_id();
        path_state.pending_pen_create = Some(crate::gis::PendingPenCreate {
            correlation_id: correlation_id.clone(),
            first_point: [hit.x, hit.y, hit.z],
        });
        ui_mgr.push_action(UiAction::PathCreateTrack {
            petal_id,
            name: AUTO_TRACK_NAME.to_string(),
            correlation_id: Some(correlation_id),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
