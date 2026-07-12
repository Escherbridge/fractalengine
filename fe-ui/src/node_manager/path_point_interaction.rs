//! Path-point viewport interaction (FR-6/7/9): pickable sphere markers for the
//! currently-edited track, click-to-place, drag-to-move, modifier-click-to-
//! annotate. fe-ui owns the full marker lifecycle here (fe-terrain is not a
//! dependency) — see `fe-ui/src/node_manager/AGENTS.md` §path-points and the
//! GPX path editor track spec §Phase-2.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::NodeManager;
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::plugin::ViewportRect;

/// Marker sphere for a single point in the currently-edited track. `index`
/// is the point's position in `PathEditorState.points`; markers are respawned
/// whenever the edited track or its point count changes.
#[derive(Component, Debug)]
pub struct PathPointMarker {
    pub index: usize,
}

/// Marker sphere radius (world units) and pick radius for the manual ray test.
const MARKER_SIZE: f32 = 0.35;
const PICK_RADIUS: f32 = 0.7;

/// In-progress drag of a path-point marker (mirrors `AxisDrag`'s role, but for
/// world-plane translation rather than screen-axis projection).
#[derive(Resource, Default)]
pub struct PathPointDrag {
    /// `(index, plane_y)` of the point being dragged, `None` when idle.
    pub active: Option<(usize, f32)>,
}

/// Keeps one `PathPointMarker` sphere entity per point in the edited track.
/// Despawns everything when not editing. Runs before the interaction system so
/// picks always see current markers.
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

    // Fast path: nothing to do (count matches). Positions are kept live by the
    // drag system writing straight into the marker Transform, and by full
    // rebuild whenever the count changes below.
    if want == have {
        return;
    }

    // Simplest correct strategy: despawn all, respawn `want`. Point counts are
    // small (authored paths), so per-change rebuild is cheap and avoids index
    // bookkeeping when points are removed mid-list.
    for (entity, _) in markers.iter() {
        commands.entity(entity).despawn();
    }
    if want == 0 {
        return;
    }

    let mesh = mesh_handle
        .get_or_insert_with(|| meshes.add(Sphere::new(MARKER_SIZE)))
        .clone();
    let material = mat_handle
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.85, 0.2),
                unlit: true,
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
        ));
    }
}

/// Manual ray/marker hit test → the nearest marker index under `cursor`, if any.
/// Mirrors `viewport_pick.rs`'s along-ray + radius test (no Bevy picking).
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
        if (pos - closest).length() < PICK_RADIUS && best.map_or(true, |b| along < b.1) {
            best = Some((marker.index, along));
        }
    }
    best
}

/// Intersect `ray` with the horizontal plane `y = plane_y`; `None` if parallel.
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

/// Path-point interaction lifecycle: click-to-place (FR-6), drag-to-move
/// (FR-7), modifier-click-to-annotate (FR-9). Runs before `handle_viewport_click`
/// and sets `manager.path_edit_capturing` so node-pick yields while editing.
pub(super) fn handle_path_point_interaction(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut manager: ResMut<NodeManager>,
    mut path_state: ResMut<PathEditorState>,
    mut drag: ResMut<PathPointDrag>,
    viewport_rect: Res<ViewportRect>,
    mut ui_mgr: ResMut<UiManager>,
    mut marker_tx: Query<(&mut Transform, &PathPointMarker)>,
    marker_pick: Query<(&GlobalTransform, &PathPointMarker)>,
    mut egui_ctx: EguiContexts,
) {
    let Some(track_id) = path_state.editing_track_id.clone() else {
        manager.path_edit_capturing = false;
        drag.active = None;
        return;
    };
    // While a track is being edited, path interaction owns viewport clicks.
    manager.path_edit_capturing = true;

    let egui_using = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.is_using_pointer())
        .unwrap_or(false);

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else { return };

    // Release → commit the drag as a MovePoint (no index churn).
    if mouse_button.just_released(MouseButton::Left) {
        if let Some((index, _)) = drag.active.take() {
            if let Some((tx, _)) = marker_tx.iter().find(|(_, m)| m.index == index) {
                let p = tx.translation;
                ui_mgr.push_action(UiAction::PathMovePoint {
                    track_node_id: track_id.clone(),
                    index,
                    position: [p.x, p.y, p.z],
                });
            }
        }
        return;
    }

    // Hold → update the dragged marker's world position on its own y-plane.
    if mouse_button.pressed(MouseButton::Left) {
        if let Some((index, plane_y)) = drag.active {
            let Some(cursor_pos) = cursor else { return };
            let Ok(ray) = camera.viewport_to_world(cam_tx, cursor_pos) else { return };
            if let Some(hit) = ray_plane_y(&ray, plane_y) {
                for (mut tx, marker) in marker_tx.iter_mut() {
                    if marker.index == index {
                        tx.translation = hit;
                    }
                }
            }
            return;
        }
    }

    // Press → pick a marker (drag / annotate) or place a new point.
    if !mouse_button.just_pressed(MouseButton::Left) || egui_using {
        return;
    }
    let Some(cursor_pos) = cursor else { return };
    if !viewport_rect.0.contains(bevy_egui::egui::pos2(cursor_pos.x, cursor_pos.y)) {
        return;
    }
    let Ok(ray) = camera.viewport_to_world(cam_tx, cursor_pos) else { return };

    if let Some((index, _)) = pick_marker(&ray, &marker_pick) {
        let modifier = keys.pressed(KeyCode::ShiftLeft)
            || keys.pressed(KeyCode::ShiftRight)
            || keys.pressed(KeyCode::AltLeft)
            || keys.pressed(KeyCode::AltRight);
        if modifier {
            // FR-9: open the inline annotation form for this point.
            path_state.open_annotate_form(index);
        } else {
            // FR-7: begin dragging this point on its current y-plane.
            let plane_y = marker_pick
                .iter()
                .find(|(_, m)| m.index == index)
                .map(|(g, _)| g.translation().y)
                .unwrap_or(0.0);
            drag.active = Some((index, plane_y));
        }
        return;
    }

    // FR-6: empty click on terrain while editing → append a point at Y=0 plane.
    if let Some(hit) = ray_plane_y(&ray, 0.0) {
        ui_mgr.push_action(UiAction::PathAppendPoint {
            track_node_id: track_id,
            position: [hit.x, hit.y, hit.z],
        });
    }
}
