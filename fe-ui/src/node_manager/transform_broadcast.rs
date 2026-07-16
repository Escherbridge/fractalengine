//! Broadcast committed gimbal transforms to DB + P2P sync, and apply inbound
//! transforms coming from the API bridge.

use bevy::prelude::*;

use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

pub(super) fn broadcast_transform(
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    transform_query: Query<(&Transform, &SpawnedNodeMarker)>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
) {
    let Some(sel) = manager.selected.as_mut() else {
        return;
    };
    if !sel.drag_committed {
        return;
    }
    sel.drag_committed = false;

    let Ok((transform, marker)) = transform_query.get(sel.entity) else {
        return;
    };
    let pos = transform.translation;
    let (rx, ry, rz) = transform.rotation.to_euler(EulerRot::XYZ);
    let sc = transform.scale;

    // Keep in-memory NodeEntry in sync so respawn_on_petal_change uses the
    // updated position instead of the stale initial one.
    verse_mgr.update_node_position(&marker.node_id, [pos.x, pos.y, pos.z]);

    if db_sender
        .0
        .send(fe_runtime::messages::DbCommand::UpdateNodeTransform {
            node_id: marker.node_id.clone(),
            position: [pos.x, pos.y, pos.z],
            rotation: [rx, ry, rz],
            scale: [sc.x, sc.y, sc.z],
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — DB thread may have crashed");
    }

    if let Some(sync) = sync_sender {
        if let Some(ref verse_id) = nav.active_verse_id {
            if sync
                .0
                .send(fe_sync::SyncCommand::UpdateNodeTransform {
                    verse_id: verse_id.clone(),
                    node_id: marker.node_id.clone(),
                    position: [pos.x, pos.y, pos.z],
                    rotation: [rx, ry, rz],
                    scale: [sc.x, sc.y, sc.z],
                })
                .is_err()
            {
                bevy::log::warn!("sync_sender channel closed — sync thread may have crashed");
            }
        }
    }
}

pub(super) fn apply_inbound_transforms(
    inbound_rx: Option<Res<fe_runtime::app::InboundTransformReceiver>>,
    mut node_query: Query<(Entity, &mut Transform, &SpawnedNodeMarker)>,
    mut commands: Commands,
) {
    let Some(rx) = inbound_rx else { return };

    // Drain all pending updates (non-blocking)
    while let Ok(update) = rx.0.try_recv() {
        let mut found = false;
        for (entity, mut transform, marker) in node_query.iter_mut() {
            if marker.node_id == update.node_id {
                transform.translation =
                    Vec3::new(update.position[0], update.position[1], update.position[2]);
                transform.rotation = Quat::from_euler(
                    EulerRot::XYZ,
                    update.rotation[0],
                    update.rotation[1],
                    update.rotation[2],
                );
                transform.scale = Vec3::new(update.scale[0], update.scale[1], update.scale[2]);
                // Stamp the entity with the DB-acknowledged (confirmed) transform
                // so that rollback logic can read it back if needed.
                commands
                    .entity(entity)
                    .try_insert(fe_runtime::messages::DbConfirmedTransform {
                        position: update.position,
                        rotation: update.rotation,
                        scale: update.scale,
                    });
                found = true;
                bevy::log::info!(
                    "API transform applied: node={} pos=[{:.2}, {:.2}, {:.2}]",
                    update.node_id,
                    update.position[0],
                    update.position[1],
                    update.position[2],
                );
                break;
            }
        }
        if !found {
            bevy::log::warn!(
                "API transform: no ECS entity for node_id={} (node may not be in active petal)",
                update.node_id,
            );
        }
    }
}
