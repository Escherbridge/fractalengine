//! Pure ribbon/quad-strip geometry for GPX track lines (FR-11); see
//! `fe-terrain/src/AGENTS.md` §path-editor. No Bevy types so it's always
//! compiled + unit-tested like `mesh::skirt::build_skirt`.

/// Vertex/index buffers for a flat ribbon walked along a polyline. `positions`
/// are laid out as (left, right) pairs per input point; `indices` reference
/// them as a `TriangleList`.
pub struct RibbonGeometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Build a horizontal ribbon of half-width `width/2` centered on `points`,
/// each vertex lifted by `y_offset` to avoid z-fighting with terrain. The
/// ribbon is extruded perpendicular to each segment's tangent in the XZ plane
/// (tangent averaged at interior points for mitred corners). Fewer than two
/// finite points yields empty geometry.
pub fn build_ribbon(points: &[[f32; 3]], width: f32, y_offset: f32) -> RibbonGeometry {
    let mut out = RibbonGeometry {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
    };
    let n = points.len();
    if n < 2 || !(width > 0.0) {
        return out;
    }

    let half_w = width * 0.5;
    let total = polyline_length(points).max(1e-6);
    let mut acc = 0.0f32;

    for i in 0..n {
        // XZ tangent: forward diff at the start, backward at the end, central
        // (averaged) at interior points so corners mitre instead of pinching.
        let tangent = if i == 0 {
            seg_dir(points[0], points[1])
        } else if i == n - 1 {
            seg_dir(points[n - 2], points[n - 1])
        } else {
            let a = seg_dir(points[i - 1], points[i]);
            let b = seg_dir(points[i], points[i + 1]);
            let s = [a[0] + b[0], a[1] + b[1]];
            normalize_or(s, a)
        };
        // Right = tangent rotated -90° in XZ (perpendicular, unit length).
        let right = [-tangent[1], tangent[0]];

        if i > 0 {
            acc += distance_xz(points[i - 1], points[i]);
        }
        let u = acc / total;

        let c = points[i];
        let cy = c[1] + y_offset;
        out.positions.push([c[0] - right[0] * half_w, cy, c[2] - right[1] * half_w]);
        out.positions.push([c[0] + right[0] * half_w, cy, c[2] + right[1] * half_w]);
        out.normals.push([0.0, 1.0, 0.0]);
        out.normals.push([0.0, 1.0, 0.0]);
        out.uvs.push([u, 0.0]);
        out.uvs.push([u, 1.0]);
    }

    // Two triangles per segment; double-sided so the ribbon never culls away
    // when viewed from below.
    for s in 0..(n - 1) {
        let l0 = (2 * s) as u32;
        let r0 = l0 + 1;
        let l1 = l0 + 2;
        let r1 = l0 + 3;
        out.indices.extend_from_slice(&[l0, l1, r0, r0, l1, r1]);
        out.indices.extend_from_slice(&[l0, r0, l1, r0, r1, l1]);
    }
    out
}

/// Unit XZ direction from `a` to `b`; falls back to +x for a degenerate pair.
fn seg_dir(a: [f32; 3], b: [f32; 3]) -> [f32; 2] {
    normalize_or([b[0] - a[0], b[2] - a[2]], [1.0, 0.0])
}

/// Normalize a 2D vector, returning `fallback` when it's near-zero length.
fn normalize_or(v: [f32; 2], fallback: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len]
    } else {
        fallback
    }
}

fn distance_xz(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    (dx * dx + dz * dz).sqrt()
}

fn polyline_length(points: &[[f32; 3]]) -> f32 {
    points.windows(2).map(|w| distance_xz(w[0], w[1])).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_for_too_few_points_or_zero_width() {
        let one = [[0.0, 0.0, 0.0]];
        assert!(build_ribbon(&one, 0.4, 0.05).positions.is_empty());
        let two = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        assert!(build_ribbon(&two, 0.0, 0.05).positions.is_empty());
    }

    #[test]
    fn straight_line_counts_and_offset() {
        // 3 points along +x → 6 verts (2 per point), 2 segments × 12 idx = 24.
        let pts = [[0.0, 10.0, 0.0], [1.0, 10.0, 0.0], [2.0, 10.0, 0.0]];
        let g = build_ribbon(&pts, 0.4, 0.05);
        assert_eq!(g.positions.len(), 6);
        assert_eq!(g.normals.len(), 6);
        assert_eq!(g.uvs.len(), 6);
        assert_eq!(g.indices.len(), 24);
        // All verts lifted by y_offset.
        for p in &g.positions {
            assert!((p[1] - 10.05).abs() < 1e-5, "y not offset: {p:?}");
        }
        // Every index is in range.
        assert!(g.indices.iter().all(|&i| (i as usize) < g.positions.len()));
    }

    #[test]
    fn straight_line_along_x_extrudes_in_z() {
        // Right perpendicular of +x tangent is ±z at half-width 0.2.
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let g = build_ribbon(&pts, 0.4, 0.0);
        // Point 0: left then right vertex.
        assert!((g.positions[0][2] - -0.2).abs() < 1e-5 || (g.positions[0][2] - 0.2).abs() < 1e-5);
        assert!((g.positions[0][2].abs() - 0.2).abs() < 1e-5);
        assert!((g.positions[1][2].abs() - 0.2).abs() < 1e-5);
        // The two verts straddle the centerline (opposite z signs).
        assert!(g.positions[0][2] * g.positions[1][2] < 0.0);
    }

    #[test]
    fn no_degenerate_triangles_for_straight_line() {
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let g = build_ribbon(&pts, 0.4, 0.0);
        for tri in g.indices.chunks(3) {
            let (a, b, c) = (
                g.positions[tri[0] as usize],
                g.positions[tri[1] as usize],
                g.positions[tri[2] as usize],
            );
            let area = tri_area(a, b, c);
            assert!(area > 1e-6, "degenerate triangle {tri:?} area={area}");
        }
    }

    #[test]
    fn uv_u_runs_zero_to_one_along_length() {
        let pts = [[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 0.0, 4.0]];
        let g = build_ribbon(&pts, 0.4, 0.0);
        assert!((g.uvs[0][0]).abs() < 1e-5, "first u should be 0");
        assert!((g.uvs.last().unwrap()[0] - 1.0).abs() < 1e-5, "last u should be 1");
    }

    fn tri_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cx = ab[1] * ac[2] - ab[2] * ac[1];
        let cy = ab[2] * ac[0] - ab[0] * ac[2];
        let cz = ab[0] * ac[1] - ab[1] * ac[0];
        0.5 * (cx * cx + cy * cy + cz * cz).sqrt()
    }
}
