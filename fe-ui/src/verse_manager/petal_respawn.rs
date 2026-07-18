//! Respawns scene entities in-place when the active petal changes, without a
//! DB round-trip (spawns directly from the in-memory `VerseManager` tree).
//! Primitive-bearing nodes take the third branch via the descriptor cache —
//! see `fe-ui/src/verse_manager/AGENTS.md` §primitives.

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use super::path_asset_materialize::PathAssetApplied;
use super::primitive_materialize::{spawn_branch, PrimitiveDescriptorCache, SpawnBranch};
use super::primitive_reconcile::{resolve_primitive_material, PrimitiveMaterialAssets};
use super::{TextureRegistryRes, VerseManager};
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

#[allow(clippy::too_many_arguments)]
pub(super) fn respawn_on_petal_change(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    mut last: Local<Option<String>>,
    mut initialized: Local<bool>,
    spawned: Query<(Entity, &SpawnedNodeMarker)>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut primitive_cache: ResMut<PrimitiveDescriptorCache>,
    mut path_applied: ResMut<PathAssetApplied>,
    texture_registry: Res<TextureRegistryRes>,
    mat_assets: Res<PrimitiveMaterialAssets>,
    db_sender: Res<DbCommandSender>,
    mesh_budget: Res<crate::plugin::MeshInstanceBudget>,
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

    // Spawn entities for the new petal directly from in-memory data — no DB round-trip
    // for the entities themselves (only cache-miss primitive descriptors fetch async).
    if let Some(ref pid) = new_petal {
        if let Some(petal) = verse_mgr.find_petal(pid) {
            // Spawn budget: saturate at MAX_PETAL_NODES (mirrors MAX_STAMPS) and
            // halt entirely under the app-wide mesh-budget gate (§mesh-budget).
            let mut remaining = if mesh_budget.exceeded {
                0
            } else {
                super::spawn::spawn_allowance(
                    petal.nodes.len(),
                    kept_node_ids.len(),
                    super::spawn::MAX_PETAL_NODES,
                )
            };
            let mut skipped: usize = 0;
            for node in &petal.nodes {
                // Skip if already spawned and being kept (petal didn't fully change).
                if kept_node_ids.contains(node.id.as_str()) {
                    continue;
                }
                if remaining == 0 {
                    skipped += 1;
                    continue;
                }
                remaining -= 1;
                match spawn_branch(node, &primitive_cache) {
                    SpawnBranch::Gltf(asset_path) => {
                        super::spawn::spawn_node_entity(
                            &mut commands,
                            &asset_server,
                            &node.id,
                            pid,
                            &node.name,
                            node.position,
                            &asset_path,
                        );
                    }
                    SpawnBranch::Primitive(descriptor) => {
                        // FR-1 third branch: warm cache → materialize immediately.
                        let material = resolve_primitive_material(
                            descriptor.texture_ref.as_deref(),
                            &texture_registry,
                            &mat_assets,
                            &mut materials,
                            &mut images,
                        );
                        super::spawn::spawn_primitive_entity(
                            &mut commands,
                            &mut meshes,
                            &node.id,
                            pid,
                            &node.name,
                            node.position,
                            descriptor,
                            material,
                        );
                    }
                    SpawnBranch::Fallback => {
                        super::spawn::spawn_fallback_sign(
                            &mut commands,
                            &mut meshes,
                            &mut materials,
                            &node.id,
                            pid,
                            &node.name,
                            node.position,
                        );
                        // Cold cache: fetch the property bag once; a primitive
                        // result promotes the sign via `materialize_cached_primitives`.
                        if primitive_cache.mark_requested(&node.id)
                            && db_sender
                                .0
                                .send(DbCommand::GetNodeProperties {
                                    node_id: node.id.clone(),
                                })
                                .is_err()
                        {
                            bevy::log::error!(
                                "db_sender channel closed during petal respawn — DB thread may have crashed"
                            );
                        }
                    }
                }
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
