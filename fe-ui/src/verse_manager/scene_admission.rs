//! Current scene-source bindings, separate from retained local history; see AGENTS.md.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

use super::VerseManager;

/// An exact current scene admission; callers cannot mint or rewrite its identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneAdmission {
    incarnation: u128,
    epoch: u64,
    local_history_version: Option<i64>,
}

impl SceneAdmission {
    pub fn local_history_version(self) -> Option<i64> {
        self.local_history_version
    }
}

pub(super) struct SceneSources {
    incarnation: u128,
    next_epoch: u64,
    hierarchy_revision: u64,
    navigation: Option<(u64, [Option<String>; 3])>,
    sources: HashMap<String, Binding>,
}

struct Binding {
    petal_id: String,
    epoch: u64,
    local_history_version: Option<i64>,
    deleted: bool,
    properties_hash: Option<[u8; 32]>,
    path_points_hash: Option<[u8; 32]>,
    route_properties_hash: Option<[u8; 32]>,
    asset_id: Option<String>,
    replica_identity: Option<[u8; 32]>,
    editor_points: Option<Value>,
}

struct Canonical<'a>(&'a Value);

impl Serialize for Canonical<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(object) => {
                let mut fields: Vec<_> = object.iter().collect();
                fields.sort_unstable_by(|left, right| left.0.cmp(right.0));
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, &Canonical(value))?;
                }
                map.end()
            }
            Value::Array(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&Canonical(value))?;
                }
                sequence.end()
            }
            scalar => scalar.serialize(serializer),
        }
    }
}

fn properties_hash(properties: &Value) -> Option<[u8; 32]> {
    serde_json::to_vec(&Canonical(properties))
        .ok()
        .map(|bytes| fe_runtime::blob_transport::content_hash(&bytes))
}

pub(super) fn route_properties(properties: &Value) -> Value {
    let keys = [
        "gpx_type",
        "gpx_points",
        "gis.track.color",
        "gis.track.width",
        "gis.track.visible",
        "gis.track.closed",
        "gis.track.fill.color",
        fe_sdk::path_asset::PATH_ASSET_PROPERTY_KEY,
    ];
    Value::Object(
        keys.into_iter()
            .filter_map(|key| {
                properties
                    .get(key)
                    .map(|value| (key.to_owned(), value.clone()))
            })
            .collect(),
    )
}

impl Default for SceneSources {
    fn default() -> Self {
        Self {
            incarnation: ulid::Ulid::new().into(),
            next_epoch: 0,
            hierarchy_revision: 0,
            navigation: None,
            sources: HashMap::new(),
        }
    }
}

impl SceneSources {
    pub(super) fn retire(&mut self) {
        if !self.sources.is_empty() {
            self.clear();
        }
    }

    pub(super) fn clear(&mut self) {
        self.sources.clear();
        self.hierarchy_revision = self.hierarchy_revision.saturating_add(1);
    }

    pub(super) fn remove(&mut self, node_id: &str) {
        self.sources.remove(node_id);
    }
}

impl VerseManager {
    pub(super) fn current_editor_points(&self, node_id: &str) -> Option<Value> {
        self.scene_sources
            .sources
            .get(node_id)?
            .editor_points
            .clone()
    }

    pub(super) fn matches_replica_current(
        &self,
        current: &fe_runtime::scene_replication::ReplicatedNodeCurrent,
    ) -> bool {
        replica_identity(current).is_some_and(|identity| {
            self.scene_sources
                .sources
                .get(&current.state.node_id)
                .is_some_and(|binding| binding.replica_identity == Some(identity))
        })
    }

    pub(super) fn bind_replica_current(
        &mut self,
        current: &fe_runtime::scene_replication::ReplicatedNodeCurrent,
    ) {
        if let Some(binding) = self.scene_sources.sources.get_mut(&current.state.node_id) {
            binding.replica_identity = replica_identity(current);
        }
    }

    pub fn synchronize_scene_navigation(
        &mut self,
        nav: &crate::navigation_manager::NavigationManager,
    ) {
        let ids = [
            &nav.active_verse_id,
            &nav.active_fractal_id,
            &nav.active_petal_id,
        ];
        if self
            .scene_sources
            .navigation
            .as_ref()
            .is_some_and(|(revision, old)| {
                *revision == nav.intent_revision
                    && old.iter().zip(ids).all(|(left, right)| left == right)
            })
        {
            return;
        }
        self.scene_sources.navigation = Some((nav.intent_revision, ids.map(Clone::clone)));
        self.scene_sources.clear();
        self.current_paths.clear();
        self.scene_refresh.clear();
        self.replica_reads.synchronize(nav);
    }

    /// Capture the manager lifetime when an independent reader produces a projection.
    pub fn scene_incarnation(&self) -> u128 {
        self.scene_sources.incarnation
    }

    pub fn scene_hierarchy_revision(&self) -> u64 {
        self.scene_sources.hierarchy_revision
    }

    /// Match a property projection to the full current source already admitted.
    pub fn current_scene_properties_match(
        &self,
        node_id: &str,
        petal_id: &str,
        properties: &Value,
    ) -> bool {
        self.scene_admission(node_id, petal_id).is_some()
            && self
                .scene_sources
                .sources
                .get(node_id)
                .and_then(|binding| binding.properties_hash)
                .is_some_and(|known| properties_hash(properties) == Some(known))
    }

    pub fn current_scene_points_match(
        &self,
        node_id: &str,
        petal_id: &str,
        points: &Value,
    ) -> bool {
        self.scene_admission(node_id, petal_id).is_some()
            && self
                .scene_sources
                .sources
                .get(node_id)
                .and_then(|binding| binding.path_points_hash)
                .is_some_and(|known| properties_hash(points) == Some(known))
    }

    pub fn current_scene_route_properties_match(
        &self,
        node_id: &str,
        petal_id: &str,
        properties: &Value,
    ) -> bool {
        self.scene_admission(node_id, petal_id).is_some()
            && self
                .scene_sources
                .sources
                .get(node_id)
                .and_then(|binding| binding.route_properties_hash)
                .is_some_and(|known| properties_hash(properties) == Some(known))
    }

    pub(super) fn current_scene_asset_match(
        &self,
        node_id: &str,
        petal_id: &str,
        asset_id: Option<&str>,
    ) -> bool {
        self.scene_admission(node_id, petal_id).is_some()
            && self
                .scene_sources
                .sources
                .get(node_id)
                .is_some_and(|binding| binding.asset_id.as_deref() == asset_id)
    }

    /// Current source identity for one node in its exact owning Petal.
    pub fn scene_admission(&self, node_id: &str, petal_id: &str) -> Option<SceneAdmission> {
        let binding = self.scene_sources.sources.get(node_id)?;
        if binding.petal_id != petal_id
            || (!binding.deleted && self.petal_id_of(node_id) != Some(petal_id))
            || (binding.deleted && self.petal_id_of(node_id).is_some())
        {
            return None;
        }
        Some(SceneAdmission {
            incarnation: self.scene_sources.incarnation,
            epoch: binding.epoch,
            local_history_version: binding.local_history_version,
        })
    }

    /// Bind a local projection only to the receipt admitted for that exact version.
    pub fn local_scene_admission(
        &self,
        node_id: &str,
        petal_id: &str,
        history_version: i64,
    ) -> Option<SceneAdmission> {
        self.scene_admission(node_id, petal_id)
            .filter(|source| source.local_history_version == Some(history_version))
    }

    /// Recheck a captured source after another independent queue has been drained.
    pub fn is_current_scene_admission(
        &self,
        node_id: &str,
        petal_id: &str,
        source: SceneAdmission,
    ) -> bool {
        self.scene_admission(node_id, petal_id) == Some(source)
    }

    pub(super) fn admit_local_scene(
        &mut self,
        node_id: &str,
        history_version: i64,
        properties: &Value,
        asset_id: Option<&str>,
    ) {
        if history_version > 0 && !self.scene_access_blocked() {
            if let Some(petal_id) = self.petal_id_of(node_id).map(str::to_owned) {
                let _ = self.admit_scene(
                    node_id,
                    &petal_id,
                    Some(history_version),
                    false,
                    Some(properties),
                    asset_id,
                );
            }
        }
    }

    pub(super) fn admit_replica_scene(
        &mut self,
        node_id: &str,
        petal_id: &str,
        deleted: bool,
        properties: Option<&Value>,
        asset_id: Option<&str>,
    ) -> Option<SceneAdmission> {
        self.admit_scene(node_id, petal_id, None, deleted, properties, asset_id)
    }

    fn admit_scene(
        &mut self,
        node_id: &str,
        petal_id: &str,
        local_history_version: Option<i64>,
        deleted: bool,
        properties: Option<&Value>,
        asset_id: Option<&str>,
    ) -> Option<SceneAdmission> {
        self.scene_sources.sources.remove(node_id);
        let epoch = self.scene_sources.next_epoch.checked_add(1)?;
        self.scene_sources.next_epoch = epoch;
        self.scene_sources.sources.insert(
            node_id.to_owned(),
            Binding {
                petal_id: petal_id.to_owned(),
                epoch,
                local_history_version,
                deleted,
                properties_hash: properties.and_then(properties_hash),
                path_points_hash: properties
                    .and_then(|properties| properties.get("gpx_points"))
                    .and_then(properties_hash),
                route_properties_hash: properties
                    .and_then(|properties| properties_hash(&route_properties(properties))),
                asset_id: asset_id.map(str::to_owned),
                replica_identity: None,
                editor_points: properties
                    .and_then(|properties| properties.get("gpx_points"))
                    .cloned(),
            },
        );
        self.scene_admission(node_id, petal_id)
    }
}

fn replica_identity(
    current: &fe_runtime::scene_replication::ReplicatedNodeCurrent,
) -> Option<[u8; 32]> {
    if matches!(
        current.provenance,
        fe_runtime::scene_replication::ReplicaNodeProvenance::LegacyReplicaApply
    ) {
        return None;
    }
    serde_json::to_value(current)
        .ok()
        .as_ref()
        .and_then(properties_hash)
}
