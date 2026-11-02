//! Applies current committed snapshots; historical previews have no materialization path.

use bevy::prelude::*;
use fe_runtime::messages::{CommittedNodeMaterialization, DbConfirmedTransform, NodeDto};
use fe_runtime::node_history::{NodeMutationReceipt, stored_rotation_quaternion};

use super::super::{NodeEntry, VerseManager};
use crate::plugin::SpawnedNodeMarker;

pub(super) enum MaterializationOutcome {
    Applied { transform_changed: bool },
    NeedsHierarchy,
    Ignored,
}

/// Admit only a complete committed asset birth matching its routed legacy identity.
pub(super) fn from_gltf_import(
    result: &fe_runtime::messages::DbResult,
) -> Option<&CommittedNodeMaterialization> {
    let fe_runtime::messages::DbResult::GltfImported {
        node_id,
        asset_id,
        petal_id,
        name,
        asset_path,
        position,
        materialization,
        ..
    } = result
    else {
        return None;
    };
    let current = materialization.as_ref()?;
    let node = &current.node;
    (current.history_version == 1
        && node.has_asset
        && node.node_id == *node_id
        && node.petal_id == *petal_id
        && node.name == *name
        && node.position == *position
        && current.asset_id.as_deref() == Some(asset_id.as_str())
        && node.asset_path.as_deref() == Some(asset_path.as_str())
        && current.properties.is_object())
    .then_some(current)
}

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
    let existing = manager.all_nodes().find(|node| node.id == state.node_id);
    if state.asset_id.is_some() && existing.is_none() {
        return None;
    }
    let asset_path = existing.and_then(|node| node.asset_path.clone());
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
        (Entity, &SpawnedNodeMarker, &Transform),
        With<super::super::spawn::ResidentNode>,
    >,
) -> MaterializationOutcome {
    let node = &current.node;
    if manager
        .committed_versions
        .get(&node.node_id)
        .is_some_and(|version| *version > current.history_version)
    {
        return MaterializationOutcome::Ignored;
    }
    if manager
        .petal_id_of(&node.node_id)
        .is_some_and(|petal| petal != node.petal_id)
    {
        return MaterializationOutcome::NeedsHierarchy;
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
        return MaterializationOutcome::Ignored;
    }
    let previous = manager
        .all_nodes()
        .find(|entry| entry.id == node.node_id)
        .map(NodeEntry::world_transform);
    let same_url_model = manager
        .url_asset_ids
        .get(&node.node_id)
        .is_some_and(|asset| asset == &current.asset_id);
    let url_storage = manager.node_url_storage(&node.node_id);
    let webpage_url = manager
        .all_nodes()
        .find(|entry| entry.id == node.node_id)
        .filter(|_| same_url_model)
        .and_then(|entry| entry.webpage_url.clone());
    if !manager.upsert_node(
        &node.petal_id,
        NodeEntry {
            id: node.node_id.clone(),
            name: node.name.clone(),
            has_asset: node.has_asset,
            position: node.position,
            transform,
            webpage_url: webpage_url.clone(),
            asset_path: node.asset_path.clone(),
        },
    ) {
        bevy::log::warn!(
            "Committed node {} has an unloaded parent; hierarchy reload is required",
            node.node_id
        );
        return MaterializationOutcome::NeedsHierarchy;
    }
    if same_url_model {
        manager.confirm_node_url(&node.node_id, webpage_url, current.asset_id.clone());
        if let Some(storage) = url_storage {
            manager.confirm_node_url_storage(&node.node_id, storage);
        }
    }
    manager
        .committed_versions
        .insert(node.node_id.clone(), current.history_version);
    let mut changed = false;
    for (entity, marker, existing_transform) in entities.iter() {
        if marker.node_id != node.node_id || marker.petal_id != node.petal_id {
            continue;
        }
        commands.entity(entity).insert((
            Name::new(node.name.clone()),
            DbConfirmedTransform {
                position: node.position,
                rotation: current.rotation_euler,
                scale: node.scale,
            },
        ));
        if previous != Some(transform) || *existing_transform != transform {
            commands.entity(entity).insert(transform);
            changed = true;
        }
    }
    MaterializationOutcome::Applied {
        transform_changed: changed,
    }
}
