//! Respawns scene entities in-place when the active petal changes, without a
//! DB round-trip (spawns directly from the in-memory `VerseManager` tree), and
//! the shared `NodeSpawner` both it and `apply_node_residency` use. Only nodes
//! the residency ledger admits are spawned — see `fe-ui/src/AGENTS.md`
//! §residency-ledger; primitive branches in `verse_manager/AGENTS.md` §primitives.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use super::path_asset_materialize::PathAssetApplied;
use super::primitive_materialize::{spawn_branch, PrimitiveDescriptorCache, SpawnBranch};
use super::primitive_reconcile::{resolve_primitive_material, PrimitiveMaterialAssets};
use super::residency_ledger::ResidencyLedger;
use super::{NodeEntry, TextureRegistryRes, VerseManager};
use crate::navigation_manager::NavigationManager;
use crate::plugin::{MeshInstanceBudget, SpawnedNodeMarker};

/// Everything needed to materialise one tree node through the FR-1 branches
/// (GLTF → cached primitive → fallback sign + one properties fetch), bundled
/// so each spawner stays under Bevy's 16-param limit.
#[derive(SystemParam)]
pub(super) struct NodeSpawner<'w> {
    asset_server: Res<'w, AssetServer>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    primitive_cache: ResMut<'w, PrimitiveDescriptorCache>,
    texture_registry: Res<'w, TextureRegistryRes>,
    mat_assets: Res<'w, PrimitiveMaterialAssets>,
    db_sender: Res<'w, DbCommandSender>,
}

impl NodeSpawner<'_> {
    /// Spawn `node` of `petal_id` through its FR-1 branch.
    pub(super) fn spawn(&mut self, commands: &mut Commands, node: &NodeEntry, petal_id: &str) {
        match spawn_branch(node, &self.primitive_cache) {
            SpawnBranch::Gltf(asset_path) => super::spawn::spawn_node_entity(
                commands,
                &self.asset_server,
                &node.id,
                petal_id,
                &node.name,
                node.position,
                &asset_path,
            ),
            SpawnBranch::Primitive(descriptor) => {
                // FR-1 third branch: warm cache → materialize immediately.
                let material = resolve_primitive_material(
                    descriptor.texture_ref.as_deref(),
                    &self.texture_registry,
                    &self.mat_assets,
                    &mut self.materials,
                    &mut self.images,
                );
                super::spawn::spawn_primitive_entity(
                    commands,
                    &mut self.meshes,
                    &node.id,
                    petal_id,
                    &node.name,
                    node.position,
                    descriptor,
                    material,
                );
            }
            SpawnBranch::Fallback => {
                super::spawn::spawn_fallback_sign(
                    commands,
                    &mut self.meshes,
                    &mut self.materials,
                    &node.id,
                    petal_id,
                    &node.name,
                    node.position,
                );
                // Cold cache: fetch the property bag once; a primitive
                // result promotes the sign via `materialize_cached_primitives`.
                if self.primitive_cache.mark_requested(&node.id)
                    && self
                        .db_sender
                        .0
                        .send(DbCommand::GetNodeProperties {
                            node_id: node.id.clone(),
                            correlation_id: None,
                        })
                        .is_err()
                {
                    bevy::log::error!(
                        "db_sender channel closed during node spawn — DB thread may have crashed"
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn respawn_on_petal_change(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    mut last: Local<Option<String>>,
    mut initialized: Local<bool>,
    spawned: Query<(Entity, &SpawnedNodeMarker)>,
    mut commands: Commands,
    mut spawner: NodeSpawner,
    mut path_applied: ResMut<PathAssetApplied>,
    ledger: Res<ResidencyLedger>,
    mesh_budget: Res<MeshInstanceBudget>,
) {
    if !*initialized {
        *last = nav.active_petal_id.clone();
        *initialized = true;
        return;
    }
    if *last == nav.active_petal_id {
        return;
    }

    let new_petal = nav.active_petal_id.clone();
    bevy::log::info!(
        "Petal changed {:?} → {:?} — respawning entities",
        *last,
        new_petal
    );

    // FR-2: the path-asset stamp groups for the old petal are about to be
    // despawned by the loop below; clear the per-track applied gate so
    // `materialize_path_assets` restamps every active-petal cached track on
    // (re)entry instead of a stale gate skipping them. See AGENTS.md
    // §path-asset-materialization.
    path_applied.clear();

    // Collect which node_ids are staying alive (kept entities) before issuing any despawns.
    // commands.entity(e).despawn() is deferred — entities just despawned are still visible
    // in the `spawned` Query during the same frame, so we must not rely on the query after
    // despawn commands have been issued.
    let kept_node_ids: std::collections::HashSet<&str> = spawned
        .iter()
        .filter(|(_, m)| {
            new_petal
                .as_deref()
                .map(|pid| pid == m.petal_id.as_str())
                .unwrap_or(false)
        })
        .map(|(_, m)| m.node_id.as_str())
        .collect();

    // Despawn entities that don't belong to the new petal.
    for (entity, marker) in spawned.iter() {
        let keep = new_petal
            .as_deref()
            .map(|pid| pid == marker.petal_id.as_str())
            .unwrap_or(false);
        if !keep {
            commands.entity(entity).despawn();
        }
    }

    // Spawn the new petal's ADMITTED nodes directly from in-memory data — the
    // ledger (chained just before) already ranked them against the camera and
    // capped them at `entity_cap`; MAX_PETAL_NODES + the mesh-budget gate stay
    // as hard backstops. Later admissions stream in via `apply_node_residency`.
    if let Some(ref pid) = new_petal {
        if let Some(petal) = verse_mgr.find_petal(pid) {
            let mut remaining = if mesh_budget.exceeded {
                0
            } else {
                super::spawn::spawn_allowance(
                    usize::MAX,
                    kept_node_ids.len(),
                    super::spawn::MAX_PETAL_NODES,
                )
            };
            let mut skipped: usize = 0;
            for node in &petal.nodes {
                if kept_node_ids.contains(node.id.as_str()) || !ledger.admits_node(&node.id) {
                    continue;
                }
                if remaining == 0 {
                    skipped += 1;
                    continue;
                }
                remaining -= 1;
                spawner.spawn(&mut commands, node, pid);
            }
            if skipped > 0 {
                bevy::log::warn!(
                    "petal respawn saturated: {} node(s) not materialized (cap {} / budget gate)",
                    skipped,
                    super::spawn::MAX_PETAL_NODES
                );
            }
        }
    }

    *last = new_petal;
}
