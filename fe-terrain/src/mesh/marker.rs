use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};

/// Generate a teardrop/pin marker mesh for waypoints.
///
/// The pin is oriented with the point at the origin, extending upward.
pub fn waypoint_mesh() -> Mesh {
    waypoint_mesh_sized(0.5, 2.0, 16)
}

/// Generate a waypoint marker with custom dimensions.
///
/// - `radius`: radius of the spherical top.
/// - `height`: total height from point to top.
/// - `segments`: number of segments around the circumference.
pub fn waypoint_mesh_sized(radius: f32, height: f32, segments: u32) -> Mesh {
    let seg = segments.max(4);
    let sphere_center_y = height - radius;

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // Vertex 0: tip of the pin at origin
    positions.push([0.0, 0.0, 0.0]);
    normals.push([0.0, -1.0, 0.0]);
    uvs.push([0.5, 1.0]);

    // Generate sphere-like upper portion with latitude rings
    let lat_steps = seg / 2;
    for lat in 0..=lat_steps {
        let theta = std::f32::consts::PI * 0.5 * lat as f32 / lat_steps as f32;
        let ring_radius = radius * theta.sin();
        let ring_y = sphere_center_y + radius * theta.cos();

        for lon in 0..seg {
            let phi = 2.0 * std::f32::consts::PI * lon as f32 / seg as f32;
            let x = ring_radius * phi.cos();
            let z = ring_radius * phi.sin();

            positions.push([x, ring_y, z]);

            let nx = x;
            let ny = ring_y - sphere_center_y;
            let nz = z;
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(0.001);
            normals.push([nx / len, ny / len, nz / len]);

            let u = lon as f32 / seg as f32;
            let v = lat as f32 / lat_steps as f32;
            uvs.push([u, v]);
        }
    }

    // Connect tip to first ring
    for lon in 0..seg {
        let next = (lon + 1) % seg;
        indices.push(0);
        indices.push(1 + lon);
        indices.push(1 + next);
    }

    // Connect latitude rings
    for lat in 0..lat_steps {
        for lon in 0..seg {
            let next_lon = (lon + 1) % seg;
            let curr_ring = 1 + lat * seg;
            let next_ring = 1 + (lat + 1) * seg;

            indices.push(curr_ring + lon);
            indices.push(next_ring + lon);
            indices.push(curr_ring + next_lon);

            indices.push(curr_ring + next_lon);
            indices.push(next_ring + lon);
            indices.push(next_ring + next_lon);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
