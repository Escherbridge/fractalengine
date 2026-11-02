//! Applies current committed snapshots; historical previews have no materialization path.

use bevy::prelude::*;
use fe_runtime::messages::{CommittedNodeMaterialization, DbConfirmedTransform, NodeDto};
use fe_runtime::node_history::{NodeMutationReceipt, stored_rotation_quaternion};

use super::super::{NodeEntry, VerseManager};
use crate::plugin::SpawnedNodeMarker;

pub(super) fn from_receipt(
    receipt: &NodeMutationReceipt,
    manager: &VerseManager,
) -> Option<CommittedNodeMaterialization> {
    let state = &receipt.state;
    if receipt.node_id != state.node_id || state.tombstone.is_some() {
        return None;
    }
    let position = state.position.map(|value| value as f32);
    let scale = state.scale.map(|value| value as f32);
    if position
        .iter()
        .chain(&scale)
        .any(|value| !value.is_finite())
    {
        return None;
    }
    let rotation = stored_rotation_quaternion(&state.rotation)?;
    let (rx, ry, rz) = Quat::from_array(rotation).to_euler(EulerRot::XYZ);
    let asset_path = manager
        .all_nodes()
        .find(|node| node.id == state.node_id)
        .and_then(|node| node.asset_path.clone());
    Some(CommittedNodeMaterialization {
        node: NodeDto {
            node_id: state.node_id.clone(),
            petal_id: state.petal_id.clone(),
            name: state
                .display_name
                .clone()
                .unwrap_or_else(|| state.node_id.clone()),
            position,
            rotation,
            scale,
            has_asset: state.asset_id.is_some(),
            asset_path,
        },
        rotation_euler: [rx, ry, rz],
        asset_id: state.asset_id.clone(),
        properties: serde_json::Value::Object(state.properties.clone()),
        history_version: receipt.history_version,
    })
}

pub(super) fn apply(
    current: &CommittedNodeMaterialization,
    manager: &mut VerseManager,
    commands: &mut Commands,
    entities: &Query<
        (
            Entity,
            &SpawnedNodeMarker,
            Option<&super::super::spawn::FallbackSign>,
        ),
        With<super::super::spawn::ResidentNode>,
    >,
) -> bool {
    let node = &current.node;
    if manager
        .committed_versions
        .get(&node.node_id)
        .is_some_and(|version| *version > current.history_version)
        || manager
            .petal_id_of(&node.node_id)
            .is_some_and(|petal| petal != node.petal_id)
    {
        return false;
    }
    let transform = Transform {
        translation: Vec3::from_array(node.position),
        rotation: Quat::from_array(node.rotation),
        scale: Vec3::from_array(node.scale),
    };
    if !transform.translation.is_finite()
        || !transform.rotation.is_finite()
        || !transform.scale.is_finite()
    {
        bevy::log::warn!(
            "Invalid committed scene transform withheld for {}",
            node.node_id
        );
        return false;
    }
    let webpage_url = manager
        .all_nodes()
        .find(|entry| entry.id == node.node_id)
        .and_then(|entry| entry.webpage_url.clone());
    if !manager.upsert_node(
        &node.petal_id,
        NodeEntry {
            id: node.node_id.clone(),
            name: node.name.clone(),
            has_asset: node.has_asset,
            position: node.position,
            transform,
            webpage_url,
            asset_path: node.asset_path.clone(),
        },
    ) {
        bevy::log::warn!(
            "Committed node {} has an unloaded parent; hierarchy reload is required",
            node.node_id
        );
        return false;
    }
    manager
        .committed_versions
        .insert(node.node_id.clone(), current.history_version);
    for (entity, marker, fallback) in entities.iter() {
        if marker.node_id != node.node_id || marker.petal_id != node.petal_id {
            continue;
        }
        let mut scene_transform = transform;
        if fallback.is_some() {
            scene_transform.translation.y += 0.5;
        }
        commands.entity(entity).insert((
            scene_transform,
            Name::new(node.name.clone()),
            DbConfirmedTransform {
                position: node.position,
                rotation: current.rotation_euler,
                scale: node.scale,
            },
        ));
    }
    true
}
