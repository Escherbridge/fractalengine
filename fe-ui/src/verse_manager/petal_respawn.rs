//! Respawns scene entities in-place when the active petal changes, without a
//! DB round-trip (spawns directly from the in-memory `VerseManager` tree).

use bevy::prelude::*;

use super::VerseManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

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
        *last, new_petal
    );

    // Collect which node_ids are staying alive (kept entities) before issuing any despawns.
    // commands.entity(e).despawn() is deferred — entities just despawned are still visible
    // in the `spawned` Query during the same frame, so we must not rely on the query after
    // despawn commands have been issued.
    let kept_node_ids: std::collections::HashSet<&str> = spawned
        .iter()
        .filter(|(_, m)| new_petal.as_deref().map(|pid| pid == m.petal_id.as_str()).unwrap_or(false))
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

    // Spawn entities for the new petal directly from in-memory data — no DB round-trip.
    if let Some(ref pid) = new_petal {
        if let Some(petal) = verse_mgr.find_petal(pid) {
            for node in &petal.nodes {
                // Skip if already spawned and being kept (petal didn't fully change).
                if kept_node_ids.contains(node.id.as_str()) {
                    continue;
                }
                if let Some(ref ap) = node.asset_path {
                    super::spawn::spawn_node_entity(
                        &mut commands,
                        &asset_server,
                        &node.id,
                        pid,
                        &node.name,
                        node.position,
                        ap,
                    );
                } else {
                    super::spawn::spawn_fallback_sign(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &node.id,
                        pid,
                        &node.name,
                        node.position,
                    );
                }
            }
        }
    }

    *last = new_petal;
}
