//! Projects admitted replica current state without claiming local history; see AGENTS.md.

use std::collections::VecDeque;

use bevy::prelude::*;
use fe_runtime::node_history::stored_rotation_quaternion;
use fe_runtime::scene_replication::ReplicatedNodeCurrent;
use serde_json::Value;

use super::{NodeEntry, SceneAdmission, VerseManager};
use crate::plugin::SpawnedNodeMarker;

const MAX_CURRENT_PATHS: usize = 256;

/// A current route source admitted by the scene manager, never a journal receipt.
#[derive(Clone, Debug)]
pub struct CurrentPathProjection {
    pub node_id: String,
    pub petal_id: String,
    pub properties: Option<Value>,
    admission: SceneAdmission,
    editor_before: Option<Value>,
}

impl CurrentPathProjection {
    pub fn is_current(&self, manager: &VerseManager) -> bool {
        manager.is_current_scene_admission(&self.node_id, &self.petal_id, self.admission)
            && self.properties.as_ref().is_none_or(|properties| {
                manager.current_scene_route_properties_match(
                    &self.node_id,
                    &self.petal_id,
                    properties,
                )
            })
    }

    pub fn apply_editor(&self, state: &mut crate::gis::PathEditorState) -> bool {
        refresh_editor(
            &self.node_id,
            self.properties.as_ref(),
            self.editor_before.as_ref(),
            state,
        )
    }
}

pub(super) fn refresh_editor(
    node_id: &str,
    properties: Option<&Value>,
    before: Option<&Value>,
    state: &mut crate::gis::PathEditorState,
) -> bool {
    if state.editing_track_id.as_deref() != Some(node_id) {
        return false;
    }
    let points = properties.and_then(|properties| properties.get("gpx_points"));
    let rows = points
        .map(crate::gis::decode_gpx_points)
        .unwrap_or_default();
    let awaiting_initial = state.points_pending && state.points.is_empty();
    state.points_pending = false;
    if !awaiting_initial
        && (crate::undo::PathEditorPolicy::PreserveChanged {
            before: before.cloned(),
        })
        .preserves(&state.points, &rows)
    {
        crate::undo::retain_local_draft(state);
        return true;
    }
    if points.is_none() {
        state.stop_editing();
        return false;
    }
    if state.points.len() != rows.len() {
        state.selected_point = None;
        state.selected_segment = None;
    }
    state.points = rows;
    if let Some(properties) = properties {
        state.edited_track_style = crate::gis::TrackStyleFields::from_properties(properties);
        state.edited_track_path_asset = properties
            .get(fe_sdk::path_asset::PATH_ASSET_PROPERTY_KEY)
            .and_then(|raw| fe_sdk::path_asset::PathAssetDescriptor::from_json(raw).ok());
    }
    false
}

#[derive(Default)]
pub(super) struct CurrentPaths(VecDeque<CurrentPathProjection>);

impl CurrentPaths {
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn remove(&mut self, node_id: &str) {
        self.0.retain(|projection| projection.node_id != node_id);
    }
}

impl VerseManager {
    pub(super) fn queue_current_path(
        &mut self,
        node_id: &str,
        petal_id: &str,
        properties: Option<Value>,
    ) -> bool {
        self.queue_current_path_after(node_id, petal_id, properties, None)
    }

    pub(super) fn queue_current_path_after(
        &mut self,
        node_id: &str,
        petal_id: &str,
        properties: Option<Value>,
        editor_before: Option<Value>,
    ) -> bool {
        let Some(admission) = self.scene_admission(node_id, petal_id) else {
            return false;
        };
        self.current_paths.remove(node_id);
        if self.current_paths.0.len() >= MAX_CURRENT_PATHS {
            return false;
        }
        self.current_paths.0.push_back(CurrentPathProjection {
            node_id: node_id.to_owned(),
            petal_id: petal_id.to_owned(),
            properties: properties
                .map(|properties| super::scene_admission::route_properties(&properties)),
            admission,
            editor_before,
        });
        true
    }

    /// Drain current path work only while its original scene source remains current.
    pub fn take_current_path_projection(
        &mut self,
        active_petal_id: Option<&str>,
    ) -> Option<CurrentPathProjection> {
        while let Some(projection) = self.current_paths.0.pop_front() {
            if active_petal_id == Some(projection.petal_id.as_str()) && projection.is_current(self)
            {
                return Some(projection);
            }
        }
        None
    }
}

pub(super) enum Outcome {
    Applied,
    Unchanged,
    Removed,
    NeedsHierarchy,
    Invalid,
}

fn matching_parent(current: &ReplicatedNodeCurrent, manager: &VerseManager) -> bool {
    manager.verses.iter().any(|verse| {
        verse.fractals.iter().any(|fractal| {
            fractal.petals.iter().any(|petal| {
                petal.id == current.state.petal_id
                    && fe_database::build_scope(&verse.id, Some(&fractal.id), Some(&petal.id))
                        == current.scope
            })
        })
    })
}

pub(super) fn apply(
    current: &ReplicatedNodeCurrent,
    manager: &mut VerseManager,
    commands: &mut Commands,
    entities: &Query<(Entity, &SpawnedNodeMarker, &Transform), With<super::spawn::ResidentNode>>,
    active_petal_id: Option<&str>,
) -> Outcome {
    let state = &current.state;
    if state.node_id.is_empty()
        || state.asset_id.as_deref() != current.asset.asset_id()
        || manager
            .petal_id_of(&state.node_id)
            .is_some_and(|petal| petal != state.petal_id)
    {
        return Outcome::Invalid;
    }
    if !matching_parent(current, manager) {
        return Outcome::NeedsHierarchy;
    }
    if state.tombstone.is_some() {
        let local_version_floor = manager.committed_versions.get(&state.node_id).copied();
        manager.remove_node(&state.petal_id, &state.node_id);
        if let Some(floor) = local_version_floor {
            manager
                .committed_versions
                .insert(state.node_id.clone(), floor);
        }
        let _ = manager.admit_replica_scene(
            &state.node_id,
            &state.petal_id,
            true,
            None,
            state.asset_id.as_deref(),
        );
        manager.bind_replica_current(current);
        for (entity, marker, _) in entities {
            if marker.node_id == state.node_id && marker.petal_id == state.petal_id {
                commands.entity(entity).try_despawn();
            }
        }
        return Outcome::Removed;
    }
    let Some(rotation) = stored_rotation_quaternion(&state.rotation) else {
        return Outcome::Invalid;
    };
    let transform = Transform {
        translation: Vec3::from_array(state.position.map(|value| value as f32)),
        rotation: Quat::from_array(rotation),
        scale: Vec3::from_array(state.scale.map(|value| value as f32)),
    };
    if !transform.translation.is_finite() || !transform.scale.is_finite() {
        return Outcome::Invalid;
    }
    let previous = manager.all_nodes().find(|node| node.id == state.node_id);
    let asset_path = current.asset.asset_path().map(str::to_owned);
    let asset_changed = previous.is_some_and(|node| {
        node.asset_path != asset_path || node.has_asset != state.asset_id.is_some()
    });
    let same_model = manager.url_asset_ids.get(&state.node_id) == Some(&state.asset_id);
    let webpage_url = previous
        .filter(|_| same_model)
        .and_then(|node| node.webpage_url.clone());
    let url_storage = manager.node_url_storage(&state.node_id);
    let local_version_floor = manager.committed_versions.get(&state.node_id).copied();
    let name = state
        .display_name
        .clone()
        .unwrap_or_else(|| state.node_id.clone());
    let properties = Value::Object(state.properties.clone());
    if manager.matches_replica_current(current)
        && previous.is_some_and(|node| node.world_transform() == transform && node.name == name)
        && !asset_changed
        && manager.current_scene_asset_match(
            &state.node_id,
            &state.petal_id,
            state.asset_id.as_deref(),
        )
        && manager.current_scene_properties_match(&state.node_id, &state.petal_id, &properties)
    {
        return Outcome::Unchanged;
    }
    if !manager.upsert_node(
        &state.petal_id,
        NodeEntry {
            id: state.node_id.clone(),
            name: name.clone(),
            has_asset: state.asset_id.is_some(),
            position: transform.translation.to_array(),
            transform,
            webpage_url: webpage_url.clone(),
            asset_path,
        },
    ) {
        return Outcome::NeedsHierarchy;
    }
    if let Some(floor) = local_version_floor {
        manager
            .committed_versions
            .insert(state.node_id.clone(), floor);
    }
    if same_model {
        manager.confirm_node_url(&state.node_id, webpage_url, state.asset_id.clone());
        if let Some(storage) = url_storage {
            manager.confirm_node_url_storage(&state.node_id, storage);
        }
    }
    let _ = manager.admit_replica_scene(
        &state.node_id,
        &state.petal_id,
        false,
        Some(&properties),
        state.asset_id.as_deref(),
    );
    manager.bind_replica_current(current);
    let (rx, ry, rz) = transform.rotation.to_euler(EulerRot::XYZ);
    if active_petal_id == Some(state.petal_id.as_str()) {
        for (entity, marker, _) in entities {
            if marker.node_id != state.node_id || marker.petal_id != state.petal_id {
                continue;
            }
            if asset_changed {
                commands.entity(entity).try_despawn();
            } else {
                commands.entity(entity).insert((
                    Name::new(name.clone()),
                    transform,
                    fe_runtime::messages::DbConfirmedTransform {
                        position: transform.translation.to_array(),
                        rotation: [rx, ry, rz],
                        scale: transform.scale.to_array(),
                    },
                ));
            }
        }
    }
    Outcome::Applied
}
