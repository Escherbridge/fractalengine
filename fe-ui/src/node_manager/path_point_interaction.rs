//! Path-point viewport interaction: place / drag / annotate the edited track's
//! points. See `fe-ui/src/node_manager/AGENTS.md` §path-points.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::NodeManager;
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::panels::toolbar::Tool;
use crate::plugin::{ToolState, ViewportRect};

/// Marker sphere for point `index` in the currently-edited track's point list.
#[derive(Component, Debug)]
pub struct PathPointMarker {
    pub index: usize,
}

/// Marker sphere radius (world units).
const MARKER_SIZE: f32 = 0.35;
/// Manual ray/marker hit radius (world units) — see AGENTS.md §path-points.
const PICK_RADIUS: f32 = 0.7;

/// In-progress drag of a path-point marker: `(index, plane_y)`, `None` when idle.
#[derive(Resource, Default)]
pub struct PathPointDrag {
    pub active: Option<(usize, f32)>,
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
        if (pos - closest).length() < PICK_RADIUS && best.map_or(true, |b| along < b.1) {
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

/// Path-point interaction: Pen-tool click-to-place, drag-to-move, modifier-
/// click-to-annotate. Sets `manager.path_edit_capturing` so node-pick yields
/// while editing. See `node_manager/AGENTS.md` §pen-tool.
pub(super) fn handle_path_point_interaction(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut manager: ResMut<NodeManager>,
    mut path_state: ResMut<PathEditorState>,
    mut drag: ResMut<PathPointDrag>,
    viewport_rect: Res<ViewportRect>,
    tool: Res<ToolState>,
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
            // Modifier-click → open the inline annotation form for this point.
            path_state.open_annotate_form(index);
        } else {
            // Plain click → begin dragging this point on its current y-plane.
            let plane_y = marker_pick
                .iter()
                .find(|(_, m)| m.index == index)
                .map(|(g, _)| g.translation().y)
                .unwrap_or(0.0);
            drag.active = Some((index, plane_y));
        }
        return;
    }

    // Empty click on terrain while the Pen tool is active → append a point at
    // the Y=0 plane. Gated on Tool::Pen so Select-mode clicks (marker pick,
    // node selection) don't also grow the polyline — see AGENTS.md §pen-tool.
    if tool.active_tool != Tool::Pen {
        return;
    }
    if let Some(hit) = ray_plane_y(&ray, 0.0) {
        ui_mgr.push_action(UiAction::PathAppendPoint {
            track_node_id: track_id,
            position: [hit.x, hit.y, hit.z],
        });
    }
}
