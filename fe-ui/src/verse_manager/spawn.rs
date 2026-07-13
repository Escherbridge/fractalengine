//! Scene-entity materialisation for hierarchy nodes: GLTF-backed nodes spawn
//! a `SceneRoot`; primitive-descriptor nodes spawn a Bevy shape mesh; nodes
//! without an asset get a fallback placard sign. See
//! `fe-ui/src/verse_manager/AGENTS.md` §primitives for the reconcile-by-marker
//! discipline (FR-2).

use bevy::prelude::*;
use fe_sdk::primitive::{PrimitiveDescriptor, PrimitiveKind};

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

/// Marker component for primitive entities, carrying the descriptor that
/// produced the current mesh/material so the reconcile system (FR-2) can
/// detect changes without re-parsing the property bag every frame.
#[derive(Component, Debug, Clone)]
pub struct PrimitiveNode {
    pub descriptor: PrimitiveDescriptor,
}

/// Build a Bevy [`Mesh`] for a primitive descriptor's `kind`/`dims`.
///
/// Dims out of range or wrong length fall back to a unit-scale default for
/// that kind rather than panicking (no `unwrap`/`expect` in prod paths).
pub fn build_primitive_mesh(desc: &PrimitiveDescriptor) -> Mesh {
    let d = &desc.dims;
    match desc.kind {
        PrimitiveKind::Cube => {
            let (w, h, dp) = (
                dim_or(d, 0, 1.0),
                dim_or(d, 1, 1.0),
                dim_or(d, 2, 1.0),
            );
            Mesh::from(Cuboid::new(w, h, dp))
        }
        PrimitiveKind::Plane => {
            let (w, dp) = (dim_or(d, 0, 1.0), dim_or(d, 1, 1.0));
            // `Plane3d::new` takes a normal + half-size; full size is dims[0]/[1].
            Mesh::from(Plane3d::new(Vec3::Y, Vec2::new(w, dp) / 2.0))
        }
        PrimitiveKind::Cylinder => {
            let (r, h) = (dim_or(d, 0, 0.5), dim_or(d, 1, 1.0));
            Mesh::from(Cylinder::new(r, h))
        }
        PrimitiveKind::Sphere => {
            let r = dim_or(d, 0, 0.5);
            Mesh::from(Sphere::new(r))
        }
    }
}

/// Read `dims[idx]`, clamped to a small positive minimum, or `default` if absent/invalid.
fn dim_or(dims: &[f32], idx: usize, default: f32) -> f32 {
    dims.get(idx)
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(default)
}

/// Spawn a primitive entity (cube/plane/cylinder/sphere) for a node carrying
/// a `primitive` JSON property (FR-1). `material` is the resolved
/// `StandardMaterial` handle — a shared default when `texture_ref` is `None`
/// (FR-3), or a loaded texture material otherwise.
pub(super) fn spawn_primitive_entity(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    node_id: &str,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
    descriptor: PrimitiveDescriptor,
    material: Handle<StandardMaterial>,
) -> Entity {
    let mesh = meshes.add(build_primitive_mesh(&descriptor));
    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_xyz(position[0], position[1], position[2]),
            Name::new(name.to_string()),
            SpawnedNodeMarker {
                node_id: node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
            PrimitiveNode { descriptor },
        ))
        .id();
    bevy::log::debug!(
        "Spawned primitive '{}' entity={:?} (petal={})",
        name, entity, petal_id
    );
    entity
}

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
