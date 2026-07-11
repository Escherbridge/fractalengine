//! Sidebar click → NodeManager selection resolution.

use bevy::prelude::*;

use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

pub(super) fn sync_sidebar_to_manager(
    nav: Res<NavigationManager>,
    mut manager: ResMut<NodeManager>,
    node_query: Query<(Entity, &SpawnedNodeMarker)>,
) {
    let Some(node_id) = manager.pending_sidebar_select.take() else {
        return;
    };

    let active_petal = nav.active_petal_id.as_deref();
    let matched = node_query.iter().find(|(_, m)| {
        m.node_id == node_id
            && active_petal
                .map(|pid| pid == m.petal_id.as_str())
                .unwrap_or(true)
    });

    if let Some((entity, _)) = matched {
        manager.select(entity, node_id);
    } else {
        manager.deselect();
    }
}
