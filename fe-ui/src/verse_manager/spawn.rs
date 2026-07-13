//! Scene-entity materialisation for hierarchy nodes: GLTF-backed nodes spawn
//! a `SceneRoot`; primitive-descriptor nodes spawn a Bevy shape mesh; a
//! path-driven wall extrudes a `gpx_points` polyline; nodes without an asset
//! get a fallback placard sign. See `fe-ui/src/verse_manager/AGENTS.md`
//! §primitives / §wall for the reconcile-by-marker discipline (FR-2/FR-5).

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
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

/// Marker for a path-driven wall entity (FR-5). Holds the `source_path` track
/// node_id it re-projects from and the wall height, so the wall-reconcile
/// system can rebuild it in place when that track's `gpx_points` change (via a
/// Track-1 `DbResult::NodePropertiesLoaded` event) and despawn it on
/// `NodeDeleted`. See `fe-ui/src/verse_manager/AGENTS.md` §wall.
#[derive(Component, Debug, Clone)]
pub struct WallNode {
    /// The `gpx_points`-carrying track node_id whose polyline drives this wall.
    pub source_path: String,
    /// Extrusion height (world units) = descriptor `dims[0]`.
    pub height: f32,
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
        // A wall's geometry is driven by its `source_path` polyline, not
        // `dims` — see `build_wall_mesh`. This branch only fires if a Wall
        // descriptor is meshed without a polyline (no source points yet);
        // yield an empty mesh so the entity exists but renders nothing until
        // the polyline resolves.
        PrimitiveKind::Wall => build_wall_mesh(&[], dim_or(d, 0, 2.0)),
    }
}

/// Extrude a polyline into a vertical quad-strip wall mesh (FR-5).
///
/// Each consecutive `polyline[i] -> polyline[i+1]` segment becomes one vertical
/// quad (two triangles) rising `height` world-units along +Y from the base
/// points. Positions/normals/UVs/indices are hand-built (raw `Mesh`), mirroring
/// the `bake_splat_mesh` idiom in `fe-terrain/src/splat/render.rs` — but with no
/// dependency on fe-terrain internals. Fewer than 2 points yields an empty mesh.
///
/// `height` is clamped to a small positive minimum; the polyline `y` (index 1)
/// is treated as the base elevation, and the top row sits at `base_y + height`.
pub fn build_wall_mesh(polyline: &[[f32; 3]], height: f32) -> Mesh {
    let h = if height.is_finite() && height > 0.0 { height } else { 2.0 };
    let seg_count = polyline.len().saturating_sub(1);

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(seg_count * 4);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(seg_count * 4);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(seg_count * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(seg_count * 6);

    for i in 0..seg_count {
        let a = polyline[i];
        let b = polyline[i + 1];

        // Horizontal segment direction (XZ); the wall's outward normal is the
        // horizontal perpendicular. Degenerate (zero-length) segments fall back
        // to +Z so the quad still has a defined orientation.
        let dir = Vec3::new(b[0] - a[0], 0.0, b[2] - a[2]);
        let normal = if dir.length_squared() > 1e-8 {
            Vec3::new(dir.z, 0.0, -dir.x).normalize_or(Vec3::Z)
        } else {
            Vec3::Z
        };
        let n = [normal.x, normal.y, normal.z];

        let base = positions.len() as u32;
        // Bottom-left, bottom-right, top-right, top-left (CCW seen from +normal).
        positions.push([a[0], a[1], a[2]]);
        positions.push([b[0], b[1], b[2]]);
        positions.push([b[0], b[1] + h, b[2]]);
        positions.push([a[0], a[1] + h, a[2]]);
        for _ in 0..4 {
            normals.push(n);
        }
        uvs.push([0.0, 1.0]);
        uvs.push([1.0, 1.0]);
        uvs.push([1.0, 0.0]);
        uvs.push([0.0, 0.0]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
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

/// Spawn a path-driven wall entity (FR-5): extrude `polyline` (a source
/// track's `gpx_points`, petal-local meters) into a quad-strip wall of
/// `descriptor.dims[0]` height. `material` is the resolved `StandardMaterial`
/// handle (shared default when `texture_ref` is `None`, per FR-3). The wall's
/// transform is identity — polyline vertices are already in petal-local world
/// coordinates, so the geometry carries its own position (unlike a cube whose
/// mesh is centered and positioned via `Transform`).
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_wall_entity(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    node_id: &str,
    petal_id: &str,
    name: &str,
    source_path: &str,
    polyline: &[[f32; 3]],
    height: f32,
    material: Handle<StandardMaterial>,
) -> Entity {
    let mesh = meshes.add(build_wall_mesh(polyline, height));
    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Name::new(name.to_string()),
            SpawnedNodeMarker {
                node_id: node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
            WallNode {
                source_path: source_path.to_string(),
                height,
            },
        ))
        .id();
    bevy::log::debug!(
        "Spawned wall '{}' entity={:?} (petal={}, source_path={})",
        name, entity, petal_id, source_path
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A polyline of N points has N-1 segments → 4 verts + 6 indices each.
    #[test]
    fn wall_mesh_vertex_and_index_counts_match_segment_count() {
        let polyline = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [2.0, 0.0, 1.0],
        ];
        let mesh = build_wall_mesh(&polyline, 3.0);
        let segs = polyline.len() - 1; // 3

        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions present");
        assert_eq!(positions.len(), segs * 4, "4 verts per segment");

        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("normals present");
        assert_eq!(normals.len(), segs * 4);

        let uvs = mesh.attribute(Mesh::ATTRIBUTE_UV_0).expect("uvs present");
        assert_eq!(uvs.len(), segs * 4);

        match mesh.indices().expect("indices present") {
            Indices::U32(idx) => assert_eq!(idx.len(), segs * 6, "2 tris (6 idx) per segment"),
            _ => panic!("expected U32 indices"),
        }
    }

    /// A single segment lifts the top row by `height` above the base points.
    #[test]
    fn wall_mesh_top_row_is_height_above_base() {
        let polyline = [[0.0, 5.0, 0.0], [4.0, 5.0, 0.0]];
        let mesh = build_wall_mesh(&polyline, 2.5);
        let pos = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .expect("expected Float32x3 positions");
        // Verts 0,1 are base (y=5.0); verts 2,3 are top (y=5.0+2.5=7.5).
        assert_eq!(pos[0][1], 5.0);
        assert_eq!(pos[1][1], 5.0);
        assert_eq!(pos[2][1], 7.5);
        assert_eq!(pos[3][1], 7.5);
    }

    /// Fewer than two points cannot form a segment → empty mesh, no panic.
    #[test]
    fn wall_mesh_empty_for_degenerate_polyline() {
        for pts in [&[][..], &[[0.0, 0.0, 0.0]][..]] {
            let mesh = build_wall_mesh(pts, 2.0);
            let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION).expect("attr present");
            assert_eq!(positions.len(), 0, "no vertices for < 2 points");
        }
    }

    /// A non-finite/zero height falls back to a sane positive default (no NaN
    /// geometry, no panic) — mirrors `dim_or`'s guard for the shape kinds.
    #[test]
    fn wall_mesh_clamps_bad_height() {
        let polyline = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mesh = build_wall_mesh(&polyline, bad);
            let pos = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .expect("expected Float32x3 positions");
            let top_y = pos[2][1];
            assert!(top_y.is_finite() && top_y > 0.0, "bad height {bad} must clamp to positive, got {top_y}");
        }
    }
}
