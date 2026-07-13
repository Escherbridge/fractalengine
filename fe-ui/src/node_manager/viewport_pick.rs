//! Viewport click → select / deselect nearest spawned node. Claims `NodePick`.

use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority};
use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

pub(super) fn handle_viewport_click(
    node_query: Query<(Entity, &GlobalTransform, &SpawnedNodeMarker)>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Only act on a fresh left-press that reached the viewport (egui/rect gating
    // already applied by `resolve_pointer_frame`).
    if !arbiter.is_fresh_press() {
        return;
    }
    // Yield if a higher-priority consumer (gimbal / path-point) already claimed
    // this frame's click.
    if !arbiter.claim(ClickPriority::NodePick) {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };

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
