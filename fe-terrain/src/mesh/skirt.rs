//! Pure mesh-skirt geometry: vertical edge walls that hide seams between tiles; see `src/AGENTS.md` §terrain_plugin.

/// Extra geometry to append to a base grid mesh; `indices` already offset by `base`.
pub struct SkirtGeometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Skirt depth in world units: `texels` × the larger grid cell size.
pub fn skirt_depth(cell_size_x: f32, cell_size_z: f32, texels: f32) -> f32 {
    cell_size_x.abs().max(cell_size_z.abs()) * texels.max(0.0)
}

/// Build vertical skirt walls around the four edges of a row-major `w×h` grid.
///
/// Edge vertices are duplicated and their copies dropped by `depth`; each edge
/// segment becomes a two-sided quad so the wall shows from any viewing angle,
/// hiding float-precision gaps and edge-height mismatches between adjacent tiles.
/// `base` is the existing vertex count; emitted indices reference `base + local`.
pub fn build_skirt(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    normals: &[[f32; 3]],
    w: usize,
    h: usize,
    depth: f32,
    base: u32,
) -> SkirtGeometry {
    let mut out = SkirtGeometry {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
    };
    if w < 2 || h < 2 || !(depth > 0.0) {
        return out;
    }

    // Four edges expressed as sequences of row-major grid indices.
    let top: Vec<usize> = (0..w).collect(); // row 0
    let bottom: Vec<usize> = (0..w).map(|c| (h - 1) * w + c).collect(); // row h-1
    let left: Vec<usize> = (0..h).map(|r| r * w).collect(); // col 0
    let right: Vec<usize> = (0..h).map(|r| r * w + (w - 1)).collect(); // col w-1

    for edge in [top, bottom, left, right] {
        append_edge_skirt(&mut out, positions, uvs, normals, &edge, depth, base);
    }
    out
}

/// Append one edge's skirt strip (top + dropped-bottom vertices, two-sided quads).
fn append_edge_skirt(
    out: &mut SkirtGeometry,
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    normals: &[[f32; 3]],
    edge: &[usize],
    depth: f32,
    base: u32,
) {
    let start = base + out.positions.len() as u32;
    for &gi in edge {
        let p = positions[gi];
        out.positions.push(p); // wall top (local even)
        out.positions.push([p[0], p[1] - depth, p[2]]); // wall bottom (local odd)
        let uv = uvs[gi];
        out.uvs.push(uv);
        out.uvs.push(uv);
        let n = normals[gi];
        out.normals.push(n);
        out.normals.push(n);
    }

    let segments = edge.len().saturating_sub(1);
    for s in 0..segments {
        let t0 = start + (2 * s) as u32; // top i
        let b0 = t0 + 1; // bottom i
        let t1 = t0 + 2; // top i+1
        let b1 = t0 + 3; // bottom i+1
        // Two-sided wall: front and reversed windings so it never culls to black.
        out.indices.extend_from_slice(&[t0, b0, t1, t1, b0, b1]);
        out.indices.extend_from_slice(&[t0, t1, b0, t1, b1, b0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skirt_depth_uses_larger_cell_times_texels() {
        assert_eq!(skirt_depth(2.0, 3.0, 2.0), 6.0);
        assert_eq!(skirt_depth(5.0, 1.0, 3.0), 15.0);
        assert_eq!(skirt_depth(2.0, 3.0, 0.0), 0.0);
    }

    #[test]
    fn build_skirt_empty_for_degenerate_grid() {
        let pos = vec![[0.0, 0.0, 0.0]];
        let uv = vec![[0.0, 0.0]];
        let n = vec![[0.0, 1.0, 0.0]];
        let s = build_skirt(&pos, &uv, &n, 1, 1, 1.0, 0);
        assert!(s.positions.is_empty());
        assert!(s.indices.is_empty());

        // Zero depth also yields no skirt.
        let s = build_skirt(&pos, &uv, &n, 1, 1, 0.0, 0);
        assert!(s.positions.is_empty());
    }

    #[test]
    fn build_skirt_2x2_counts_and_lowered_bottoms() {
        // 2x2 grid, positions at y = 10.
        let pos = vec![
            [0.0, 10.0, 0.0],
            [1.0, 10.0, 0.0],
            [0.0, 10.0, 1.0],
            [1.0, 10.0, 1.0],
        ];
        let uv = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        let n = vec![[0.0, 1.0, 0.0]; 4];
        let depth = 2.0;
        let s = build_skirt(&pos, &uv, &n, 2, 2, depth, 100);

        // 4 edges, each with 2 grid vertices → 2 (top+bottom) × 2 = 4 verts per edge.
        assert_eq!(s.positions.len(), 16);
        assert_eq!(s.uvs.len(), 16);
        assert_eq!(s.normals.len(), 16);
        // Each edge: 1 segment × (2 tris front + 2 tris back) × 3 = 12 indices → 48.
        assert_eq!(s.indices.len(), 48);

        // Every odd (bottom) vertex sits `depth` below its even (top) partner.
        for pair in s.positions.chunks(2) {
            assert_eq!(pair[1][1], pair[0][1] - depth);
            assert_eq!(pair[1][0], pair[0][0]);
            assert_eq!(pair[1][2], pair[0][2]);
        }
        // Indices are all offset by base and within the appended range.
        assert!(s.indices.iter().all(|&i| i >= 100 && i < 100 + 16));
    }
}
