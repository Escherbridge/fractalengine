//! Applies current committed snapshots; historical previews have no materialization path.

use bevy::prelude::*;
use fe_runtime::messages::{CommittedNodeMaterialization, DbConfirmedTransform, NodeDto};
use fe_runtime::node_history::{NodeMutationReceipt, stored_rotation_quaternion};

use super::super::{NodeEntry, VerseManager};
use crate::plugin::SpawnedNodeMarker;

pub(super) fn external_editor_batch(
    receipt: &fe_runtime::editor_batch::EditorBatchReceipt,
    manager: &VerseManager,
) -> Result<crate::editor_batch::BatchProjection, String> {
    let scope = manager.verses.iter().find_map(|verse| {
        verse.fractals.iter().find_map(|fractal| {
            fractal
                .petals
                .iter()
                .any(|petal| petal.id == receipt.petal_id)
                .then(|| {
                    fe_database::build_scope(&verse.id, Some(&fractal.id), Some(&receipt.petal_id))
                })
        })
    });
    if scope.as_deref() != Some(receipt.scope.as_str())
        || (receipt.members.is_empty() && matches!(&receipt.target, fe_runtime::editor_batch::EditorBatchTarget::Nodes))
        || receipt.members.len() > fe_runtime::editor_batch::MAX_EDITOR_BATCH_TARGETS
        || receipt.members.iter().enumerate().any(|(index, member)| {
            receipt.members[..index]
                .iter()
                .any(|old| old.node_id == member.node_id)
        })
    {
        return Err(
            "External selection commit has an incomplete or unexpected target scope".into(),
        );
    }
    let projection = crate::editor_batch::BatchProjection {
        petal_id: receipt.petal_id.clone(),
        members: receipt.members.clone(),
        groups: receipt.groups.clone(),
        source: crate::editor_batch::ProjectionSource::ExternalCommit,
        draft_before: Vec::new(),
    };
    editor_batch(&projection, manager)?;
    Ok(projection)
}

pub(super) fn editor_batch(
    projection: &crate::editor_batch::BatchProjection,
    manager: &VerseManager,
) -> Result<Vec<CommittedNodeMaterialization>, String> {
    if !matches!(projection.source, crate::editor_batch::ProjectionSource::Commit | crate::editor_batch::ProjectionSource::ExternalCommit) {
        return Ok(Vec::new());
    }
    let petal = manager
        .find_petal(&projection.petal_id)
        .ok_or("Selection's Petal is not loaded; current hierarchy is required")?;
    let mut converted = Vec::new();
    for member in &projection.members {
        let version = member
            .history_version
            .filter(|version| *version > 0)
            .ok_or("A batch member has no committed version")?;
        if member.state.node_id != member.node_id
            || member.state.petal_id != projection.petal_id
            || !petal.nodes.iter().any(|node| node.id == member.node_id)
            || !local_receipt_is_current(manager, &member.node_id, &projection.petal_id, version)
        {
            return Err(
                "A batch member changed or is unavailable; the complete projection was withheld"
                    .into(),
            );
        }
        let receipt = NodeMutationReceipt {
            node_id: member.node_id.clone(),
            history_version: version,
            state: member.state.clone(),
            persist: fe_runtime::node_history::PersistStatus::Committed,
        };
        converted.push(from_receipt(&receipt, manager).ok_or(
            "A batch member could not be materialized; the complete projection was withheld",
        )?);
    }
    Ok(converted)
}

pub(super) enum MaterializationOutcome {
    Applied { transform_changed: bool },
    NeedsHierarchy,
    Ignored,
}

fn local_receipt_is_current(manager: &VerseManager, node_id: &str, petal_id: &str, version: i64) -> bool {
    manager.committed_versions.get(node_id).is_none_or(|known| {
        *known < version || (*known == version && manager.local_scene_admission(node_id, petal_id, version).is_some())
    })
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
    if !local_receipt_is_current(manager, &node.node_id, &node.petal_id, current.history_version) {
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
    manager.admit_local_scene(&node.node_id, current.history_version, &current.properties, current.asset_id.as_deref());
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
