use bevy::asset::RenderAssetUsages;
use bevy::color::Color;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};

/// How to color the track geometry.
#[derive(Debug, Clone)]
pub enum ColorMode {
    /// Uniform color.
    Solid(Color),
    /// Color mapped by elevation (low=green, high=red).
    ElevationGradient,
    /// Color mapped by speed between consecutive points.
    SpeedGradient,
    /// Color mapped by time progression.
    TimeGradient,
}

/// Generate a ribbon mesh along a track path.
///
/// - `points`: ordered 3D positions along the track.
/// - `width`: ribbon width in world units.
/// - `color_mode`: determines vertex colors.
///
/// The ribbon is extruded perpendicular to the path direction in the XZ plane,
/// with a small Y offset to prevent z-fighting with terrain.
pub fn track_mesh(points: &[[f32; 3]], width: f32, color_mode: ColorMode) -> Mesh {
    if points.len() < 2 {
        return Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
    }

    // Simplify large tracks using RDP to keep rendering fast
    let points = if points.len() > crate::simplify::SIMPLIFY_THRESHOLD {
        crate::simplify::rdp_simplify(points, crate::simplify::DEFAULT_EPSILON_M)
    } else {
        points.to_vec()
    };
    let points = &points;

    let n = points.len();
    let half_w = width / 2.0;
    let y_offset = 0.5;

    let mut positions = Vec::with_capacity(n * 2);
    let mut normals = Vec::with_capacity(n * 2);
    let mut uvs = Vec::with_capacity(n * 2);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(n * 2);

    // Compute accumulated distances for UV mapping
    let mut distances = vec![0.0f32; n];
    for i in 1..n {
        let dx = points[i][0] - points[i - 1][0];
        let dy = points[i][1] - points[i - 1][1];
        let dz = points[i][2] - points[i - 1][2];
        distances[i] = distances[i - 1] + (dx * dx + dy * dy + dz * dz).sqrt();
    }
    let total_dist = distances[n - 1].max(0.001);

    // Min/max elevation for gradient
    let min_ele = points.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
    let max_ele = points.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    let ele_range = (max_ele - min_ele).max(0.001);

    for i in 0..n {
        let dir = if i == 0 {
            Vec3::new(
                points[1][0] - points[0][0],
                0.0,
                points[1][2] - points[0][2],
            )
        } else if i == n - 1 {
            Vec3::new(
                points[n - 1][0] - points[n - 2][0],
                0.0,
                points[n - 1][2] - points[n - 2][2],
            )
        } else {
            Vec3::new(
                points[i + 1][0] - points[i - 1][0],
                0.0,
                points[i + 1][2] - points[i - 1][2],
            )
        };

        let right = Vec3::new(-dir.z, 0.0, dir.x).normalize_or(Vec3::X) * half_w;
        let center = Vec3::new(points[i][0], points[i][1] + y_offset, points[i][2]);

        let left_pos = center - right;
        let right_pos = center + right;

        positions.push([left_pos.x, left_pos.y, left_pos.z]);
        positions.push([right_pos.x, right_pos.y, right_pos.z]);

        normals.push([0.0, 1.0, 0.0]);
        normals.push([0.0, 1.0, 0.0]);

        let u = distances[i] / total_dist;
        uvs.push([u, 0.0]);
        uvs.push([u, 1.0]);

        let t = i as f32 / (n - 1).max(1) as f32;
        let rgba: [f32; 4] = match &color_mode {
            ColorMode::Solid(c) => {
                let lin = c.to_linear();
                [lin.red, lin.green, lin.blue, 1.0]
            }
            ColorMode::ElevationGradient => {
                let frac = (points[i][1] - min_ele) / ele_range;
                [frac, 1.0 - frac, 0.2, 1.0]
            }
            ColorMode::SpeedGradient | ColorMode::TimeGradient => [t, 0.2, 1.0 - t, 1.0],
        };
        colors.push(rgba);
        colors.push(rgba);
    }

    // Indices
    let mut indices = Vec::with_capacity((n - 1) * 6);
    for i in 0..(n - 1) {
        let base = (i * 2) as u32;
        indices.push(base);
        indices.push(base + 2);
        indices.push(base + 1);
        indices.push(base + 1);
        indices.push(base + 2);
        indices.push(base + 3);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
