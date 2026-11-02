//! Commit gimbal transforms locally and apply inbound API transforms.

use bevy::prelude::*;

use super::NodeManager;
use crate::plugin::SpawnedNodeMarker;

/// FR-4 (path_interaction_20260716): bake the whole-path gimbal into one point.
/// Express the raw world point relative to the `centroid` baseline, then apply
/// the entity's post-gimbal `transform` (translate + rotate + scale about the
/// centroid). At the identity baseline it returns the point unchanged. Pure.
fn bake_transformed_point(world: Vec3, centroid: Vec3, transform: &Transform) -> Vec3 {
    transform.transform_point(world - centroid)
}

pub(super) fn broadcast_transform(
    mut manager: ResMut<NodeManager>,
    mut transform_query: Query<(&mut Transform, &SpawnedNodeMarker)>,
    pick_shapes: Query<&super::TrackPickShape>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut undo: ResMut<crate::undo::UndoState>,
    nav: Res<crate::navigation_manager::NavigationManager>,
    clock: Res<crate::notice::NoticeClock>,
) {
    let before = manager.pending_transform_before.take();
    let Some(sel) = manager.selected.as_mut() else {
        return;
    };
    if !sel.drag_committed {
        return;
    }
    sel.drag_committed = false;
    let entity = sel.entity;

    // FR-4: a committed gimbal on a track ribbon bakes its transform delta into
    // ALL gpx points (persisted via `PathTransformPoints` → in-place MovePoints),
    // NOT a node transform. The ribbon carries a `TrackPickShape` whose
    // `centroid` is the exact baseline the mesh was anchored at, so the bake
    // pivots identically to what the gimbal displayed.
    if let Ok(pick) = pick_shapes.get(entity) {
        undo.invalidate_redo();
        let Ok((mut transform, marker)) = transform_query.get_mut(entity) else {
            return;
        };
        let baked: Vec<[f32; 3]> = pick
            .points
            .iter()
            .map(|p| {
                let w = bake_transformed_point(*p, pick.centroid, &transform);
                [w.x, w.y, w.z]
            })
            .collect();
        if !baked.is_empty() {
            ui_mgr.push_action(crate::actions::UiAction::PathTransformPoints {
                track_node_id: marker.node_id.clone(),
                points: baked,
            });
        }
        // Reset to the centroid baseline; the queued MovePoints drive the gpx
        // bridge's despawn/respawn redraw (fresh mesh + markers + TrackPickShape),
        // so the entity never lingers in a doubled transform.
        *transform = Transform::from_translation(pick.centroid);
        return;
    }

    let Ok((transform, marker)) = transform_query.get(entity) else {
        return;
    };
    let after = *transform;
    let node_id = marker.node_id.clone();
    let petal_id = marker.petal_id.clone();
    let Some(before) = before else {
        ui_mgr.show_advisory(
            "Transform had no captured starting state; edit was not saved",
            clock.elapsed_secs_f64(),
        );
        return;
    };
    match undo.transform(
        &nav,
        &node_id,
        &petal_id,
        before,
        after,
        verse_mgr.committed_versions.get(&node_id).copied(),
        &db_sender.0,
    ) {
        Ok(true) => ui_mgr.show_toast("Saving transform...", clock.elapsed_secs_f64()),
        Ok(false) => {}
        Err(error) => {
            verse_mgr.update_node_transform(&node_id, before);
            if let Ok((mut target, _)) = transform_query.get_mut(entity) {
                *target = before;
            }
            ui_mgr.show_advisory(error, clock.elapsed_secs_f64());
        }
    }
}

pub(super) fn apply_inbound_transforms(
    inbound_rx: Option<Res<fe_runtime::app::InboundTransformReceiver>>,
    mut node_query: Query<(Entity, &mut Transform, &SpawnedNodeMarker)>,
    mut commands: Commands,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
) {
    let Some(rx) = inbound_rx else { return };

    // Drain all pending updates (non-blocking)
    while let Ok(update) = rx.0.try_recv() {
        verse_mgr.update_node_transform(
            &update.node_id,
            Transform {
                translation: Vec3::from_array(update.position),
                rotation: Quat::from_euler(
                    EulerRot::XYZ,
                    update.rotation[0],
                    update.rotation[1],
                    update.rotation[2],
                ),
                scale: Vec3::from_array(update.scale),
            },
        );
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

// ---------------------------------------------------------------------------
// Tests — FR-4 whole-path transform bake (pure geometry, Bevy-App-free).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn bake_identity_baseline_is_unchanged() {
        // At the baseline transform (translation = centroid, no rot/scale) every
        // point maps back to itself — the invariant that keeps an un-dragged
        // commit a no-op.
        let centroid = Vec3::new(10.0, 2.0, -4.0);
        let world = Vec3::new(13.0, 2.0, -1.0);
        let t = Transform::from_translation(centroid);
        let out = bake_transformed_point(world, centroid, &t);
        assert!((out - world).length() < 1e-5, "out = {out:?}");
    }

    #[test]
    fn bake_pure_translation_shifts_by_delta() {
        let centroid = Vec3::new(5.0, 0.0, 5.0);
        let world = Vec3::new(7.0, 0.0, 5.0);
        let delta = Vec3::new(100.0, 3.0, -50.0);
        let t = Transform::from_translation(centroid + delta);
        let out = bake_transformed_point(world, centroid, &t);
        assert!((out - (world + delta)).length() < 1e-4, "out = {out:?}");
    }

    #[test]
    fn bake_rotation_pivots_about_centroid() {
        // A point 1 unit +X of the centroid, rotated 90° about Y, lands 1 unit
        // -Z of the centroid (Bevy right-handed: +X → -Z).
        let centroid = Vec3::new(2.0, 0.0, 2.0);
        let world = centroid + Vec3::X;
        let t = Transform {
            translation: centroid,
            rotation: Quat::from_rotation_y(FRAC_PI_2),
            scale: Vec3::ONE,
        };
        let out = bake_transformed_point(world, centroid, &t);
        let expected = centroid + Vec3::new(0.0, 0.0, -1.0);
        assert!((out - expected).length() < 1e-4, "out = {out:?}");
    }

    #[test]
    fn bake_scale_grows_distance_from_centroid() {
        let centroid = Vec3::new(-3.0, 1.0, 4.0);
        let world = centroid + Vec3::new(1.0, 1.0, 1.0);
        let t = Transform {
            translation: centroid,
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let out = bake_transformed_point(world, centroid, &t);
        let expected = centroid + Vec3::new(2.0, 2.0, 2.0);
        assert!((out - expected).length() < 1e-4, "out = {out:?}");
    }
}
