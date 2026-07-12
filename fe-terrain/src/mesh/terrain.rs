use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};

use crate::mesh::skirt::{build_skirt, skirt_depth};

/// Skirt depth as a multiple of the grid cell size (hides small inter-tile seams).
const SKIRT_TEXELS: f32 = 2.0;
/// Skirt depth floor as a fraction of tile edge length so huge low-zoom tiles
/// still get a visible skirt versus inter-tile elevation disagreements.
const SKIRT_MIN_FRACTION: f32 = 0.015;
/// Outward skirt flare as a fraction of tile edge length; covers the horizontal
/// XZ gaps between independently-anchored neighbour tiles (see AGENTS.md).
const SKIRT_OVERLAP_FRACTION: f32 = 0.02;

/// Generate a terrain mesh from a grid of elevation values.
///
/// - `elevations`: row-major array of height values (width * height).
/// - `width`, `height`: grid dimensions in samples.
/// - `tile_world_size`: world-space size of one tile edge (meters).
///
/// Returns a triangle-list mesh with positions, normals, UVs, and downward edge
/// skirts (see `src/AGENTS.md` §terrain_plugin) so seams between adjacent tiles
/// never reveal the background.
pub fn terrain_mesh(
    elevations: &[f32],
    width: u32,
    height: u32,
    tile_world_size: f64,
) -> Mesh {
    assert_eq!(
        elevations.len(),
        (width * height) as usize,
        "elevation count must match width * height"
    );

    let w = width as usize;
    let h = height as usize;
    let cell_size_x = tile_world_size as f32 / (w - 1).max(1) as f32;
    let cell_size_z = tile_world_size as f32 / (h - 1).max(1) as f32;

    // Vertex positions + UVs
    let mut positions = Vec::with_capacity(w * h);
    let mut uvs = Vec::with_capacity(w * h);
    for row in 0..h {
        for col in 0..w {
            let x = col as f32 * cell_size_x;
            let z = row as f32 * cell_size_z;
            let y = elevations[row * w + col];
            positions.push([x, y, z]);
            uvs.push([
                col as f32 / (w - 1).max(1) as f32,
                row as f32 / (h - 1).max(1) as f32,
            ]);
        }
    }

    // Indices (two triangles per quad)
    let quad_count = (w - 1) * (h - 1);
    let mut indices = Vec::with_capacity(quad_count * 6);
    for row in 0..(h - 1) {
        for col in 0..(w - 1) {
            let tl = (row * w + col) as u32;
            let tr = tl + 1;
            let bl = ((row + 1) * w + col) as u32;
            let br = bl + 1;
            indices.push(tl);
            indices.push(bl);
            indices.push(tr);
            indices.push(tr);
            indices.push(bl);
            indices.push(br);
        }
    }

    // Compute normals via cross product of triangle edges
    let mut normals = vec![[0.0f32, 1.0, 0.0]; w * h];
    for tri in indices.chunks(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let v0 = Vec3::from(positions[i0]);
        let v1 = Vec3::from(positions[i1]);
        let v2 = Vec3::from(positions[i2]);
        let edge1 = v1 - v0;
        let edge2 = v2 - v0;
        let face_normal = edge1.cross(edge2);
        for &i in &[i0, i1, i2] {
            normals[i][0] += face_normal.x;
            normals[i][1] += face_normal.y;
            normals[i][2] += face_normal.z;
        }
    }
    for n in &mut normals {
        let v = Vec3::from(*n).normalize_or(Vec3::Y);
        *n = [v.x, v.y, v.z];
    }

    // Extrude the four edges down + outward into skirt walls once the base normals
    // are final, so neither elevation-step nor horizontal-gap seams reveal the
    // background. Both depth and flare scale with the (already-scaled) tile size.
    let base = positions.len() as u32;
    let tile_size = tile_world_size as f32;
    let depth = skirt_depth(cell_size_x, cell_size_z, SKIRT_TEXELS, tile_size, SKIRT_MIN_FRACTION);
    let overlap = tile_size.abs() * SKIRT_OVERLAP_FRACTION;
    let skirt = build_skirt(&positions, &uvs, &normals, w, h, depth, overlap, base);
    positions.extend(skirt.positions);
    normals.extend(skirt.normals);
    uvs.extend(skirt.uvs);
    indices.extend(skirt.indices);

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_mesh_adds_skirt_vertices_and_indices() {
        // Flat 3x3 grid → 9 base vertices, (3-1)^2 * 6 = 24 base indices.
        let elevations = vec![0.0f32; 9];
        let mesh = terrain_mesh(&elevations, 3, 3, 100.0);
        // Skirts add perimeter walls, so vertex + index counts exceed the base grid.
        assert!(
            mesh.count_vertices() > 9,
            "expected skirt vertices beyond the 9 base, got {}",
            mesh.count_vertices()
        );
        let idx_len = mesh.indices().map(|i| i.len()).unwrap_or(0);
        assert!(idx_len > 24, "expected skirt indices beyond the 24 base, got {idx_len}");
    }

    #[test]
    fn terrain_mesh_base_grid_attributes_are_consistent() {
        // Positions, normals, and UVs must stay equal length (base + skirt).
        let elevations = vec![1.0f32; 16];
        let mesh = terrain_mesh(&elevations, 4, 4, 50.0);
        let positions = mesh.count_vertices();
        let normals_len = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .map(|v| v.len())
            .unwrap_or(0);
        let uv_len = mesh
            .attribute(Mesh::ATTRIBUTE_UV_0)
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(positions, normals_len);
        assert_eq!(positions, uv_len);
    }
}
