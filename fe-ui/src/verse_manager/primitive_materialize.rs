//! Petal-wide primitive materialization (FR-1 third branch): a fe-ui-local
//! descriptor cache fed by `GetNodeProperties` round-trips, consumed by
//! `petal_respawn` and this module's promote/reconcile system. See
//! `fe-ui/src/verse_manager/AGENTS.md` §primitives.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use fe_sdk::primitive::{PrimitiveDescriptor, PRIMITIVE_PROPERTY_KEY};

use super::primitive_reconcile::{resolve_primitive_material, PrimitiveMaterialAssets};
use super::spawn::{build_primitive_mesh, spawn_primitive_entity, FallbackSign, PrimitiveNode};
use super::{NodeEntry, TextureRegistryRes, VerseManager};
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

/// Live primitive entities (marker + descriptor + mesh + material handles).
pub(super) type PrimitiveEntityQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static SpawnedNodeMarker,
        &'static mut PrimitiveNode,
        &'static mut Mesh3d,
        &'static mut MeshMaterial3d<StandardMaterial>,
    ),
>;

/// Fallback placard signs eligible for promotion to a primitive mesh.
pub(super) type FallbackSignQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static SpawnedNodeMarker, &'static Transform),
    (With<FallbackSign>, Without<PrimitiveNode>),
>;

/// Cache of `node_id → PrimitiveDescriptor` parsed from `NodePropertiesLoaded`
/// broadcasts, plus the in-flight `GetNodeProperties` request set. See
/// AGENTS.md §primitives for why this exists (hierarchy payload carries no
/// properties).
#[derive(Resource, Default)]
pub struct PrimitiveDescriptorCache {
    descriptors: HashMap<String, PrimitiveDescriptor>,
    requested: HashSet<String>,
}

impl PrimitiveDescriptorCache {
    /// Cached descriptor for a node, if its properties carried one.
    pub fn get(&self, node_id: &str) -> Option<&PrimitiveDescriptor> {
        self.descriptors.get(node_id)
    }

    /// Ingest a `NodePropertiesLoaded` payload: cache the parsed `primitive`
    /// descriptor, or drop a stale entry when the key is absent/invalid.
    pub fn note_properties(&mut self, node_id: &str, properties: &serde_json::Value) {
        self.requested.remove(node_id);
        match properties
            .get(PRIMITIVE_PROPERTY_KEY)
            .and_then(|raw| PrimitiveDescriptor::from_json(raw).ok())
        {
            Some(desc) => {
                self.descriptors.insert(node_id.to_string(), desc);
            }
            None => {
                self.descriptors.remove(node_id);
            }
        }
    }

    /// Drop a node's cached descriptor + request mark (primitive prop written/deleted).
    pub fn invalidate(&mut self, node_id: &str) {
        self.descriptors.remove(node_id);
        self.requested.remove(node_id);
    }

    /// Drop everything (database reset).
    pub fn clear(&mut self) {
        self.descriptors.clear();
        self.requested.clear();
    }

    /// Mark a properties fetch in flight; `true` when the caller should send
    /// `GetNodeProperties` (not cached, not already requested).
    pub fn mark_requested(&mut self, node_id: &str) -> bool {
        if self.descriptors.contains_key(node_id) {
            return false;
        }
        self.requested.insert(node_id.to_string())
    }
}

/// Which materialization branch a node takes on petal (re)spawn (FR-1).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum SpawnBranch {
    /// GLTF-backed node — `spawn_node_entity`.
    Gltf(String),
    /// Cached primitive descriptor — `spawn_primitive_entity`.
    Primitive(PrimitiveDescriptor),
    /// No asset, no known descriptor — fallback sign (+ a properties fetch).
    Fallback,
}

/// Classify a node for petal (re)spawn: GLTF asset wins, then a cached
/// primitive descriptor, else the fallback sign. Selection plays no part.
pub(super) fn spawn_branch(node: &NodeEntry, cache: &PrimitiveDescriptorCache) -> SpawnBranch {
    if let Some(ref ap) = node.asset_path {
        return SpawnBranch::Gltf(ap.clone());
    }
    if let Some(desc) = cache.get(&node.id) {
        return SpawnBranch::Primitive(desc.clone());
    }
    SpawnBranch::Fallback
}

/// Materialize cached descriptors for the active petal as they arrive:
/// promote fallback signs (or spawn fresh) into primitive meshes and
/// reconcile already-spawned primitives whose cached descriptor changed —
/// no selection required. Runs chained after `apply_db_results` +
/// `respawn_on_petal_change`, gated on cache change.
#[allow(clippy::too_many_arguments)]
pub(super) fn materialize_cached_primitives(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    cache: Res<PrimitiveDescriptorCache>,
    mesh_budget: Res<crate::plugin::MeshInstanceBudget>,
    texture_registry: Res<TextureRegistryRes>,
    mat_assets: Res<PrimitiveMaterialAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    mut primitives: PrimitiveEntityQuery,
    fallback_signs: FallbackSignQuery,
) {
    if !cache.is_changed() {
        return;
    }
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        return;
    };
    let Some(petal) = verse_mgr.find_petal(petal_id) else {
        return;
    };

    // Spawn budget: halt under the app-wide mesh-budget gate, else saturate
    // fresh spawns at MAX_PETAL_NODES (mirrors petal_respawn; §mesh-budget).
    let mut remaining = if mesh_budget.exceeded {
        0
    } else {
        let live = primitives.iter().count() + fallback_signs.iter().count();
        super::spawn::spawn_allowance(usize::MAX, live, super::spawn::MAX_PETAL_NODES)
    };
    let mut skipped: usize = 0;

    for node in &petal.nodes {
        if node.asset_path.is_some() {
            continue; // GLTF branch owns asset-backed nodes
        }
        let Some(descriptor) = cache.get(&node.id) else {
            continue;
        };

        // Already a primitive → reconcile in place on descriptor drift.
        let mut found = false;
        for (marker, mut prim, mut mesh3d, mut mat3d) in primitives.iter_mut() {
            if marker.node_id != node.id || marker.petal_id != petal_id {
                continue;
            }
            found = true;
            if prim.descriptor == *descriptor {
                continue;
            }
            mesh3d.0 = meshes.add(build_primitive_mesh(descriptor));
            if descriptor.texture_ref != prim.descriptor.texture_ref {
                mat3d.0 = resolve_primitive_material(
                    descriptor.texture_ref.as_deref(),
                    &texture_registry,
                    &mat_assets,
                    &mut materials,
                    &mut images,
                );
            }
            prim.descriptor = descriptor.clone();
        }
        if found {
            continue;
        }
        if remaining == 0 {
            skipped += 1;
            continue;
        }
        remaining -= 1;

        // Promote a fallback sign (undoing its +0.5 hover offset), or spawn
        // fresh at the tree position when no placeholder exists yet.
        let pos = if let Some((entity, _, transform)) = fallback_signs
            .iter()
            .find(|(_, m, _)| m.node_id == node.id && m.petal_id == petal_id)
        {
            commands.entity(entity).despawn();
            [
                transform.translation.x,
                transform.translation.y - 0.5,
                transform.translation.z,
            ]
        } else {
            node.position
        };
        let material = resolve_primitive_material(
            descriptor.texture_ref.as_deref(),
            &texture_registry,
            &mat_assets,
            &mut materials,
            &mut images,
        );
        spawn_primitive_entity(
            &mut commands,
            &mut meshes,
            &node.id,
            petal_id,
            &node.name,
            pos,
            descriptor.clone(),
            material,
        );
        bevy::log::debug!(
            "Materialized cached primitive node={} without selection (FR-1)",
            node.id
        );
    }
    if skipped > 0 {
        bevy::log::warn!(
            "primitive materialization saturated: {} node(s) skipped (cap {} / budget gate)",
            skipped,
            super::spawn::MAX_PETAL_NODES
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::primitive::PrimitiveKind;
    use serde_json::json;

    fn cube_props() -> serde_json::Value {
        json!({ "primitive": { "kind": "cube", "dims": [1.0, 2.0, 3.0] } })
    }

    fn node(id: &str, asset_path: Option<&str>) -> NodeEntry {
        NodeEntry {
            id: id.to_string(),
            name: id.to_string(),
            has_asset: asset_path.is_some(),
            position: [1.0, 2.0, 3.0],
            webpage_url: None,
            asset_path: asset_path.map(str::to_string),
        }
    }

    #[test]
    fn note_properties_caches_descriptor_and_clears_request_mark() {
        let mut cache = PrimitiveDescriptorCache::default();
        assert!(cache.mark_requested("n1"), "first request must send");
        cache.note_properties("n1", &cube_props());
        let desc = cache.get("n1").expect("descriptor cached");
        assert_eq!(desc.kind, PrimitiveKind::Cube);
        assert_eq!(desc.dims, vec![1.0, 2.0, 3.0]);
        assert!(
            !cache.requested.contains("n1"),
            "request mark cleared on load"
        );
    }

    #[test]
    fn note_properties_drops_entry_when_key_absent_or_invalid() {
        let mut cache = PrimitiveDescriptorCache::default();
        cache.note_properties("n1", &cube_props());
        cache.note_properties("n1", &json!({}));
        assert!(cache.get("n1").is_none(), "absent key must evict");

        cache.note_properties("n1", &cube_props());
        cache.note_properties(
            "n1",
            &json!({ "primitive": { "kind": "torus", "dims": [1.0] } }),
        );
        assert!(cache.get("n1").is_none(), "invalid descriptor must evict");
    }

    #[test]
    fn mark_requested_dedupes_and_skips_cached_nodes() {
        let mut cache = PrimitiveDescriptorCache::default();
        assert!(cache.mark_requested("n1"));
        assert!(
            !cache.mark_requested("n1"),
            "in-flight request must not re-send"
        );
        cache.note_properties("n2", &cube_props());
        assert!(!cache.mark_requested("n2"), "cached node needs no fetch");
    }

    #[test]
    fn invalidate_clears_descriptor_and_request_mark() {
        let mut cache = PrimitiveDescriptorCache::default();
        cache.note_properties("n1", &cube_props());
        cache.mark_requested("n2");
        cache.invalidate("n1");
        cache.invalidate("n2");
        assert!(cache.get("n1").is_none());
        assert!(
            cache.mark_requested("n2"),
            "invalidated request mark must re-arm"
        );
    }

    #[test]
    fn spawn_branch_materializes_primitive_without_selection() {
        // FR-1 petal-wide path: classification keys ONLY on the node + cache —
        // no NodeManager selection is even representable here.
        let mut cache = PrimitiveDescriptorCache::default();
        cache.note_properties("n1", &cube_props());
        match spawn_branch(&node("n1", None), &cache) {
            SpawnBranch::Primitive(desc) => assert_eq!(desc.kind, PrimitiveKind::Cube),
            other => panic!("expected Primitive branch, got {other:?}"),
        }
    }

    #[test]
    fn spawn_branch_leaves_non_primitive_nodes_unaffected() {
        let mut cache = PrimitiveDescriptorCache::default();
        cache.note_properties("n1", &cube_props());
        // GLTF-backed node keeps the asset branch even with a cached descriptor.
        assert_eq!(
            spawn_branch(&node("n1", Some("models/cube.glb")), &cache),
            SpawnBranch::Gltf("models/cube.glb".to_string())
        );
        // Unknown asset-less node keeps the fallback sign.
        assert_eq!(
            spawn_branch(&node("n9", None), &cache),
            SpawnBranch::Fallback
        );
    }
}
