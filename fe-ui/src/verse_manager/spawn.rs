//! Scene-entity materialisation for hierarchy nodes: GLTF-backed nodes spawn
//! a `SceneRoot`; nodes without an asset get a fallback placard sign.

use bevy::prelude::*;

use crate::plugin::SpawnedNodeMarker;

pub(super) fn spawn_node_entity(
    commands: &mut Commands,
    asset_server: &AssetServer,
    node_id: &str,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
    asset_path: &str,
) {
    let handle: Handle<Scene> = asset_server.load(format!("{}#Scene0", asset_path));
    let entity = commands
        .spawn((
            SceneRoot(handle),
            Transform::from_xyz(position[0], position[1], position[2]),
            Name::new(name.to_string()),
            SpawnedNodeMarker {
                node_id: node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
        ))
        .id();
    bevy::log::debug!("Spawned '{}' entity={:?} (petal={})", name, entity, petal_id);
}

/// Marker component for fallback sign entities (nodes without geometry).
#[derive(Component, Debug)]
pub struct FallbackSign;

/// Spawn a simple vertical plane (sign) for nodes that lack a scene asset.
/// The sign is a thin cuboid standing upright with the node name visible
/// via the `Name` component (Bevy inspector / gizmo overlays show it).
pub(super) fn spawn_fallback_sign(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    node_id: &str,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
) {
    // A thin vertical cuboid: 0.8 wide, 0.6 tall, 0.02 deep — like a placard.
    let mesh = meshes.add(Cuboid::new(0.8, 0.6, 0.02));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.3, 0.35, 0.5, 0.9),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    // Position the sign slightly above the node's Y so it hovers at eye level.
    let sign_y = position[1] + 0.5;
    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_xyz(position[0], sign_y, position[2]),
            Name::new(format!("[{}]", name)),
            SpawnedNodeMarker {
                node_id: node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
            FallbackSign,
        ))
        .id();
    bevy::log::debug!(
        "Spawned fallback sign '{}' entity={:?} (petal={})",
        name, entity, petal_id
    );
}
