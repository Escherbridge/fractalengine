//! Commit gimbal transforms locally and apply inbound API transforms.

use std::time::Instant;

use bevy::prelude::*;
use fe_runtime::scene_hook_gate::{
    SceneAugment, SceneCaller, SceneGateTicket, SceneHookGate, SceneHookInput, SceneVerdict,
};

use super::NodeManager;
use crate::plugin::SpawnedNodeMarker;

/// One committed single-node gimbal transform.
pub(super) struct CommittedTransform {
    entity: Entity,
    node_id: String,
    petal_id: String,
    before: Transform,
    after: Transform,
}

impl CommittedTransform {
    /// The `scene.transform` input; rotation as stored (Euler XYZ radians).
    fn gate_input(&self) -> SceneHookInput {
        let (x, y, z) = self.after.rotation.to_euler(EulerRot::XYZ);
        SceneHookInput::Transform {
            petal_id: self.petal_id.clone(),
            node_id: self.node_id.clone(),
            position: self.after.translation.to_array(),
            rotation: [x, y, z],
            scale: self.after.scale.to_array(),
        }
    }
}

/// A commit held while the `scene.transform` gate answers (at most its 250 ms deadline).
pub(super) struct GatedTransform {
    commit: CommittedTransform,
    ticket: SceneGateTicket,
}

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
    scene_gate: Option<Res<SceneHookGate>>,
    mut gated: Local<Option<GatedTransform>>,
) {
    for (entity, restore) in manager.take_cancelled_restores() {
        if let Ok((mut transform, _)) = transform_query.get_mut(entity) {
            restore.apply(&mut transform);
        }
    }
    // A gated commit settles before any newer commit is read; the gate's deadline bounds the wait.
    if let Some(mut held) = gated.take() {
        let Some(verdict) = held.ticket.poll(Instant::now()) else {
            *gated = Some(held);
            return;
        };
        settle_transform(
            verdict,
            &held.commit,
            &mut manager,
            &mut transform_query,
            &mut verse_mgr,
            &mut ui_mgr,
            &mut undo,
            &nav,
            &db_sender.0,
            clock.elapsed_secs_f64(),
        );
    }
    if manager.selection_count() > 1 {
        return;
    }

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
    let commit = CommittedTransform {
        entity,
        node_id,
        petal_id,
        before,
        after,
    };
    let unchanged =
        crate::undo::transform_value(&before).matches(&crate::undo::transform_value(&after));
    // scene.transform (bindings.md §6): only a real move asks the bound extension.
    let mut ticket = match scene_gate.as_deref() {
        Some(gate) if !unchanged => gate.begin(commit.gate_input(), SceneCaller::ui()),
        _ => SceneGateTicket::proceed(),
    };
    match ticket.poll(Instant::now()) {
        Some(verdict) => settle_transform(
            verdict,
            &commit,
            &mut manager,
            &mut transform_query,
            &mut verse_mgr,
            &mut ui_mgr,
            &mut undo,
            &nav,
            &db_sender.0,
            clock.elapsed_secs_f64(),
        ),
        None => *gated = Some(GatedTransform { commit, ticket }),
    }
}

/// Apply a settled gate verdict: submit the commit, or roll back a refused one like `transform_failed`.
fn settle_transform(
    verdict: SceneVerdict,
    commit: &CommittedTransform,
    manager: &mut NodeManager,
    transform_query: &mut Query<(&mut Transform, &SpawnedNodeMarker)>,
    verse_mgr: &mut crate::verse_manager::VerseManager,
    ui_mgr: &mut crate::actions::UiManager,
    undo: &mut crate::undo::UndoState,
    nav: &crate::navigation_manager::NavigationManager,
    db: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>,
    now_secs: f64,
) {
    let augment = match verdict {
        SceneVerdict::Proceed { augment } => augment,
        refused @ (SceneVerdict::Refused { .. } | SceneVerdict::InputTooLarge { .. }) => {
            rollback_transform(commit, transform_query, verse_mgr);
            // A drag released while this one was gated started from the refused pose: drop it too.
            if let Some(sel) = manager
                .selected
                .as_mut()
                .filter(|sel| sel.entity == commit.entity && sel.drag_committed)
            {
                sel.drag_committed = false;
                manager.pending_transform_before = None;
            }
            if let Some(message) = refused.refusal_message() {
                ui_mgr.show_advisory(message, now_secs);
            }
            return;
        }
    };
    match undo.transform(
        nav,
        &commit.node_id,
        &commit.petal_id,
        commit.before,
        commit.after,
        verse_mgr.committed_versions.get(&commit.node_id).copied(),
        db,
    ) {
        Ok(true) => {
            stage_augment(augment);
            ui_mgr.show_toast("Saving transform...", now_secs);
        }
        Ok(false) => {}
        Err(error) => {
            rollback_transform(commit, transform_query, verse_mgr);
            ui_mgr.show_advisory(error, now_secs);
        }
    }
}

/// Restore the pre-drag pose in the tree and on the entity.
fn rollback_transform(
    commit: &CommittedTransform,
    transform_query: &mut Query<(&mut Transform, &SpawnedNodeMarker)>,
    verse_mgr: &mut crate::verse_manager::VerseManager,
) {
    verse_mgr.update_node_transform(&commit.node_id, commit.before);
    if let Ok((mut target, _)) = transform_query.get_mut(commit.entity) {
        *target = commit.before;
    }
}

/// Stage an allowed augment once the transform is queued; it never blocks or fails the edit.
fn stage_augment(augment: Option<SceneAugment>) {
    if let Some(Err(error)) = augment.map(SceneAugment::stage) {
        bevy::log::warn!(%error, "scene.transform augment was not staged");
    }
}

pub(super) fn apply_inbound_transforms(
    inbound_rx: Option<Res<fe_runtime::app::InboundTransformReceiver>>,
    mut node_query: Query<(Entity, &mut Transform, &SpawnedNodeMarker)>,
    mut batch_ui: ResMut<crate::actions::UiManager>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    mut manager: ResMut<NodeManager>,
) {
    let Some(rx) = inbound_rx else { return };

    // Drain all pending updates (non-blocking)
    while let Ok(update) = rx.0.try_recv() {
        manager.invalidate_external_preview(&update.node_id);
        batch_ui
            .editor_batch
            .invalidate_preview_capture(&update.node_id);

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
        for (_, mut transform, marker) in node_query.iter_mut() {
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
                // The API broadcast is an optimistic preview, not a committed receipt.
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
