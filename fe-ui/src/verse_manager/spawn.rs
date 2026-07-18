//! Scene-entity materialisation for hierarchy nodes: GLTF-backed nodes spawn
//! a `SceneRoot`; primitive-descriptor nodes spawn a Bevy shape mesh; nodes
//! without an asset get a fallback placard sign. See
//! `fe-ui/src/verse_manager/AGENTS.md` §primitives for the reconcile-by-marker
//! discipline (FR-2).

use bevy::prelude::*;
use fe_sdk::primitive::{PrimitiveDescriptor, PrimitiveKind};

use crate::plugin::SpawnedNodeMarker;

/// Hard cap on scene entities spawned per petal — mirrors `MAX_STAMPS`; a
/// runaway node count saturates here instead of wedging the renderer.
pub(super) const MAX_PETAL_NODES: usize = 10_000;

/// Additional spawns allowed given `already` live and `requested` wanted,
/// saturating at `max`. Pure so the cap math is unit-testable.
pub(super) fn spawn_allowance(requested: usize, already: usize, max: usize) -> usize {
    max.saturating_sub(already).min(requested)
}

/// Resolve an asset path to a loadable scene path: append `#Scene0` only for
/// gltf/glb assets that don't already carry a label; pass anything else through.
fn scene_asset_path(asset_path: &str) -> String {
    let is_gltf = asset_path.ends_with(".gltf") || asset_path.ends_with(".glb");
    if is_gltf && !asset_path.contains('#') {
        format!("{}#Scene0", asset_path)
    } else {
        asset_path.to_string()
    }
}

pub(super) fn spawn_node_entity(
    commands: &mut Commands,
    asset_server: &AssetServer,
    node_id: &str,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
    asset_path: &str,
) {
    let handle: Handle<Scene> = asset_server.load(scene_asset_path(asset_path));
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
    bevy::log::debug!(
        "Spawned '{}' entity={:?} (petal={})",
        name,
        entity,
        petal_id
    );
}

/// Marker component for fallback sign entities (nodes without geometry).
#[derive(Component, Debug)]
pub struct FallbackSign;

/// Marker component for a single path-asset stamp instance, carrying the
/// source track id + petal so the reconcile system can despawn/rebuild the
/// whole stamped group when the descriptor or `gpx_points` change. See
/// `fe-ui/src/verse_manager/AGENTS.md` §path-asset-stamp.
#[derive(Component, Debug, Clone)]
pub struct PathAssetInstance {
    pub source_track_id: String,
    pub petal_id: String,
}

/// Spawn one path-asset stamp instance: a GLTF `SceneRoot` at a full
/// `Transform` (so the caller can bake in the tangent rotation), tagged with a
/// [`PathAssetInstance`] marker keyed to its source track. Additive sibling of
/// [`spawn_node_entity`] that lets the caller supply rotation/scale rather
/// than translation-only. See `fe-ui/src/verse_manager/AGENTS.md`
/// §path-asset-stamp.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_stamped_entity(
    commands: &mut Commands,
    asset_server: &AssetServer,
    node_id: &str,
    source_track_id: &str,
    petal_id: &str,
    name: &str,
    transform: Transform,
    asset_path: &str,
) {
    let handle: Handle<Scene> = asset_server.load(scene_asset_path(asset_path));
    let entity = commands
        .spawn((
            SceneRoot(handle),
            transform,
            Name::new(name.to_string()),
            SpawnedNodeMarker {
                node_id: node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
            PathAssetInstance {
                source_track_id: source_track_id.to_string(),
                petal_id: petal_id.to_string(),
            },
        ))
        .id();
    bevy::log::debug!(
        "Spawned path-asset stamp '{}' entity={:?} (track={} petal={})",
        name,
        entity,
        source_track_id,
        petal_id
    );
}

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
            let (w, h, dp) = (dim_or(d, 0, 1.0), dim_or(d, 1, 1.0), dim_or(d, 2, 1.0));
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
        name,
        entity,
        petal_id
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
        name,
        entity,
        petal_id
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_allowance_passes_through_under_cap() {
        assert_eq!(spawn_allowance(5, 0, 10), 5);
        assert_eq!(spawn_allowance(10, 0, 10), 10);
    }

    #[test]
    fn spawn_allowance_saturates_at_cap() {
        assert_eq!(spawn_allowance(100, 0, 10), 10);
        assert_eq!(spawn_allowance(100, 7, 10), 3);
    }

    #[test]
    fn spawn_allowance_zero_when_cap_reached_or_overshot() {
        assert_eq!(spawn_allowance(100, 10, 10), 0);
        // Already over the cap (e.g. pre-existing entities) must not underflow.
        assert_eq!(spawn_allowance(100, 999, 10), 0);
    }

    #[test]
    fn spawn_allowance_degenerate_inputs() {
        assert_eq!(spawn_allowance(0, 0, 10), 0);
        assert_eq!(
            spawn_allowance(usize::MAX, 0, MAX_PETAL_NODES),
            MAX_PETAL_NODES
        );
    }
}
