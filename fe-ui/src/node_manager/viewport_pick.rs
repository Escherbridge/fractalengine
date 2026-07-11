//! Viewport click → select / deselect nearest spawned node.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{SpawnedNodeMarker, ViewportRect};

pub(super) fn handle_viewport_click(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    node_query: Query<(Entity, &GlobalTransform, &SpawnedNodeMarker)>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    viewport_rect: Res<ViewportRect>,
    mut egui_ctx: EguiContexts,
) {
    // If a drag was just started this frame (by handle_gimbal_interaction), skip.
    if manager.is_dragging() {
        return;
    }
    if !mouse_button.just_pressed(MouseButton::Left) {
        return;
    }

    // Guard against egui-owned pointer (panel resize, button hold, etc.)
    let egui_using = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.is_using_pointer())
        .unwrap_or(false);
    if egui_using {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor) = window.cursor_position() else { return };

    if !viewport_rect.0.contains(bevy_egui::egui::pos2(cursor.x, cursor.y)) {
        return;
    }

    let Ok((camera, cam_tx)) = cameras.single() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_tx, cursor) else { return };

    let active_petal = nav.active_petal_id.as_deref();
    const PICK_RADIUS: f32 = 1.5;
    let mut best: Option<(Entity, f32, String)> = None;

    for (entity, g_tx, marker) in node_query.iter() {
        if active_petal
            .map(|pid| pid != marker.petal_id.as_str())
            .unwrap_or(false)
        {
            continue;
        }
        let pos = g_tx.translation();
        let along = (pos - ray.origin).dot(*ray.direction);
        if along < 0.0 {
            continue;
        }
        let closest = ray.origin + *ray.direction * along;
        if (pos - closest).length() < PICK_RADIUS {
            if best.is_none() || along < best.as_ref().unwrap().1 {
                best = Some((entity, along, marker.node_id.clone()));
            }
        }
    }

    if let Some((entity, _, node_id)) = best {
        manager.select(entity, node_id.clone());
    } else {
        manager.deselect();
    }
}
