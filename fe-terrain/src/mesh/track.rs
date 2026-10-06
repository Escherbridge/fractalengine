use bevy::asset::RenderAssetUsages;
use bevy::color::Color;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};

const MAX_ZONE_POINTS: usize = 2_048;

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

/// path_interaction_20260716 (FR-4): arithmetic-mean position of `points`, the
/// anchor `render_gpx_tracks` builds the ribbon relative to (and spawns the
/// entity `Transform` at). Callers must pass the SAME filtered position list the
/// mesh is built from so the whole-path gimbal bakes about the exact baseline
/// the entity renders at. `[0,0,0]` for an empty list. Pure + unit-tested.
pub fn track_centroid(points: &[[f32; 3]]) -> [f32; 3] {
    if points.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let n = points.len() as f32;
    let mut sum = [0.0f32; 3];
    for p in points {
        sum[0] += p[0];
        sum[1] += p[1];
        sum[2] += p[2];
    }
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

/// Generate a ribbon mesh along a track path.
///
/// - `points`: ordered 3D positions along the track.
/// - `width`: ribbon width in the SAME units as `points` (petal-local meters —
///   the ribbon is NOT world-scaled, so width must not be either; see
///   `src/AGENTS.md` §track-styling).
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

fn zone_orient(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f64 {
    (b[0] as f64 - a[0] as f64) * (c[2] as f64 - a[2] as f64)
        - (b[2] as f64 - a[2] as f64) * (c[0] as f64 - a[0] as f64)
}

fn zone_on_segment(point: [f32; 3], a: [f32; 3], b: [f32; 3]) -> bool {
    let scale = (b[0] as f64 - a[0] as f64)
        .hypot(b[2] as f64 - a[2] as f64)
        .max(1.0);
    if zone_orient(a, b, point).abs() > scale * scale * 1e-10 {
        return false;
    }
    let epsilon = scale * 1e-8;
    point[0] as f64 >= a[0].min(b[0]) as f64 - epsilon
        && point[0] as f64 <= a[0].max(b[0]) as f64 + epsilon
        && point[2] as f64 >= a[2].min(b[2]) as f64 - epsilon
        && point[2] as f64 <= a[2].max(b[2]) as f64 + epsilon
}

fn zone_segments_intersect(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> bool {
    let sign = |value: f64| {
        if value > 1e-10 {
            1
        } else if value < -1e-10 {
            -1
        } else {
            0
        }
    };
    let o1 = sign(zone_orient(a, b, c));
    let o2 = sign(zone_orient(a, b, d));
    let o3 = sign(zone_orient(c, d, a));
    let o4 = sign(zone_orient(c, d, b));
    (o1 != 0 && o2 != 0 && o3 != 0 && o4 != 0 && o1 != o2 && o3 != o4)
        || (o1 == 0 && zone_on_segment(c, a, b))
        || (o2 == 0 && zone_on_segment(d, a, b))
        || (o3 == 0 && zone_on_segment(a, c, d))
        || (o4 == 0 && zone_on_segment(b, c, d))
}

fn zone_polygon_is_simple(points: &[[f32; 3]]) -> bool {
    let edge_count = points.len();
    if edge_count < 3 {
        return false;
    }
    for first in 0..edge_count {
        let first_next = (first + 1) % edge_count;
        for second in first + 1..edge_count {
            let second_next = (second + 1) % edge_count;
            if first == second_next || first_next == second {
                continue;
            }
            if zone_segments_intersect(
                points[first],
                points[first_next],
                points[second],
                points[second_next],
            ) {
                return false;
            }
        }
    }
    true
}

fn uniformly_sample_ring(points: &[[f32; 3]], limit: usize) -> Vec<[f32; 3]> {
    if points.len() <= limit {
        return points.to_vec();
    }
    (0..limit)
        .map(|index| points[index * points.len() / limit])
        .collect()
}

fn zone_area_abs(points: &[[f32; 3]]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a[0] as f64 * b[2] as f64 - b[0] as f64 * a[2] as f64)
        .sum::<f64>()
        .abs()
}

fn zone_has_three_unique_points(points: &[[f32; 3]]) -> bool {
    let mut unique = Vec::<[f32; 2]>::with_capacity(3);
    for point in points {
        let xz = [point[0], point[2]];
        if !unique.contains(&xz) {
            unique.push(xz);
            if unique.len() == 3 {
                return true;
            }
        }
    }
    false
}

/// Accepted border/fill geometry shared by rendering and exact fill picking.
#[derive(Debug, Clone, Default)]
pub struct PreparedTrackZone {
    pub points: Vec<[f32; 3]>,
    pub triangle_indices: Vec<u32>,
}

/// Sanitize, cap, validate, and triangulate one closed track zone.
pub fn prepare_track_zone(points: &[[f32; 3]]) -> PreparedTrackZone {
    let mut polygon = Vec::with_capacity(points.len().min(MAX_ZONE_POINTS));
    for point in points
        .iter()
        .copied()
        .filter(|point| point.iter().all(|value| value.is_finite()))
    {
        if polygon.last().is_none_or(|last| *last != point) {
            polygon.push(point);
        }
    }
    if polygon.len() > 1
        && polygon.first().is_some_and(|first| {
            polygon
                .last()
                .is_some_and(|last| (first[0] - last[0]).hypot(first[2] - last[2]) <= f32::EPSILON)
        })
    {
        polygon.pop();
    }
    let original_area = zone_area_abs(&polygon);
    polygon = uniformly_sample_ring(&polygon, MAX_ZONE_POINTS);

    let mut prepared = PreparedTrackZone {
        points: polygon,
        triangle_indices: Vec::new(),
    };
    if !zone_has_three_unique_points(&prepared.points) || !zone_polygon_is_simple(&prepared.points)
    {
        return prepared;
    }
    let area = zone_area_abs(&prepared.points);
    if !area.is_finite()
        || area == 0.0
        || (original_area.is_finite() && original_area > 0.0 && area < original_area * 1e-6)
    {
        return prepared;
    }
    let flat: Vec<f64> = prepared
        .points
        .iter()
        .flat_map(|point| [point[0] as f64, point[2] as f64])
        .collect();
    if let Ok(indices) = earcutr::earcut(&flat, &[], 2) {
        prepared.triangle_indices = indices.into_iter().map(|index| index as u32).collect();
    }
    prepared
}

/// Build a closed border ribbon plus an earcut fill in one cleanup entity.
pub fn track_zone_mesh(
    points: &[[f32; 3]],
    width: f32,
    border_color: Color,
    fill_color: Color,
) -> Mesh {
    let prepared = prepare_track_zone(points);
    track_zone_mesh_prepared(&prepared, width, border_color, fill_color)
}

/// Build a zone mesh from geometry already accepted by [`prepare_track_zone`].
pub fn track_zone_mesh_prepared(
    prepared: &PreparedTrackZone,
    width: f32,
    border_color: Color,
    fill_color: Color,
) -> Mesh {
    let polygon = &prepared.points;
    if polygon.len() < 3 {
        return track_mesh(polygon, width, ColorMode::Solid(border_color));
    }

    let mut closed_border = polygon.to_vec();
    closed_border.push(polygon[0]);
    let mut mesh = track_mesh(&closed_border, width, ColorMode::Solid(border_color));
    if prepared.triangle_indices.is_empty() {
        return mesh;
    }

    let base_vertex = mesh.count_vertices() as u32;
    let fill_positions: Vec<[f32; 3]> = polygon
        .iter()
        .map(|point| [point[0], point[1] + 0.48, point[2]])
        .collect();
    let fill_normals = vec![[0.0, 1.0, 0.0]; polygon.len()];
    let fill_uvs = vec![[0.0, 0.0]; polygon.len()];
    let linear_fill = fill_color.to_linear();
    let fill_colors = vec![
        [
            linear_fill.red,
            linear_fill.green,
            linear_fill.blue,
            linear_fill.alpha,
        ];
        polygon.len()
    ];

    if let Some(VertexAttributeValues::Float32x3(values)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        values.extend(fill_positions);
    }
    if let Some(VertexAttributeValues::Float32x3(values)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
    {
        values.extend(fill_normals);
    }
    if let Some(VertexAttributeValues::Float32x2(values)) = mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
    {
        values.extend(fill_uvs);
    }
    if let Some(VertexAttributeValues::Float32x4(values)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR)
    {
        values.extend(fill_colors);
    }
    if let Some(Indices::U32(indices)) = mesh.indices_mut() {
        indices.extend(
            prepared
                .triangle_indices
                .iter()
                .map(|index| base_vertex + *index),
        );
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centroid_of_empty_is_origin() {
        assert_eq!(track_centroid(&[]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn centroid_of_single_point_is_itself() {
        assert_eq!(track_centroid(&[[3.0, 4.0, 5.0]]), [3.0, 4.0, 5.0]);
    }

    #[test]
    fn centroid_is_arithmetic_mean() {
        // Symmetric square on XZ centered at (1, 0, 1).
        let pts = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 2.0],
            [0.0, 0.0, 2.0],
        ];
        let c = track_centroid(&pts);
        assert!((c[0] - 1.0).abs() < 1e-6);
        assert_eq!(c[1], 0.0);
        assert!((c[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn centroid_relative_points_recenter_on_origin() {
        // Subtracting the centroid must place the mean at the origin — the exact
        // invariant `render_gpx_tracks` relies on for the whole-path gimbal.
        let pts = [[10.0, 2.0, -4.0], [14.0, 6.0, 0.0], [12.0, 4.0, -2.0]];
        let c = track_centroid(&pts);
        let rel: Vec<[f32; 3]> = pts
            .iter()
            .map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
            .collect();
        let rc = track_centroid(&rel);
        assert!(rc[0].abs() < 1e-5 && rc[1].abs() < 1e-5 && rc[2].abs() < 1e-5);
    }

    #[test]
    fn zone_mesh_adds_fill_and_closing_border() {
        let polygon = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [4.0, 0.0, 3.0],
            [0.0, 0.0, 3.0],
        ];
        let mesh = track_zone_mesh(
            &polygon,
            0.2,
            Color::srgb(0.0, 0.8, 1.0),
            Color::srgba(0.2, 0.4, 0.6, 0.5),
        );
        assert_eq!(mesh.count_vertices(), 14);
        assert!(mesh.indices().is_some_and(|indices| indices.len() == 30));
    }

    #[test]
    fn invalid_zone_polygon_keeps_border_without_fill() {
        let line = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
        let mesh = track_zone_mesh(&line, 0.2, Color::WHITE, Color::srgba(1.0, 0.0, 0.0, 0.5));
        assert_eq!(mesh.count_vertices(), 8);
    }

    #[test]
    fn self_intersecting_zone_keeps_border_without_fill() {
        let bow_tie = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 4.0],
            [0.0, 0.0, 4.0],
            [4.0, 0.0, 0.0],
        ];
        let mesh = track_zone_mesh(
            &bow_tie,
            0.2,
            Color::WHITE,
            Color::srgba(1.0, 0.0, 0.0, 0.5),
        );
        assert_eq!(mesh.count_vertices(), 10);
        assert!(mesh.indices().is_some_and(|indices| indices.len() == 24));
        assert!(prepare_track_zone(&bow_tie).triangle_indices.is_empty());
    }

    #[test]
    fn high_point_zone_is_simplified_and_capped_before_fill() {
        let input_len = crate::simplify::SIMPLIFY_THRESHOLD + 100;
        let polygon: Vec<[f32; 3]> = (0..input_len)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / input_len as f32;
                [angle.cos() * 500.0, 0.0, angle.sin() * 500.0]
            })
            .collect();
        let prepared = prepare_track_zone(&polygon);
        assert!(prepared.points.len() <= MAX_ZONE_POINTS);
        assert!(prepared.points.len() < polygon.len());
        assert!(!prepared.triangle_indices.is_empty());
        let min_x = prepared
            .points
            .iter()
            .map(|point| point[0])
            .fold(f32::MAX, f32::min);
        let max_x = prepared
            .points
            .iter()
            .map(|point| point[0])
            .fold(f32::MIN, f32::max);
        let min_z = prepared
            .points
            .iter()
            .map(|point| point[2])
            .fold(f32::MAX, f32::min);
        let max_z = prepared
            .points
            .iter()
            .map(|point| point[2])
            .fold(f32::MIN, f32::max);
        assert!(min_x < -490.0 && max_x > 490.0);
        assert!(min_z < -490.0 && max_z > 490.0);
        let mesh = track_zone_mesh(
            &polygon,
            0.2,
            Color::WHITE,
            Color::srgba(0.2, 0.4, 0.6, 0.5),
        );
        assert!(mesh.count_vertices() <= MAX_ZONE_POINTS * 3 + 2);
        assert!(mesh
            .indices()
            .is_some_and(|indices| indices.len() > prepared.points.len() * 6));
    }

    #[test]
    fn dense_small_circle_survives_large_default_rdp_epsilon() {
        let count = MAX_ZONE_POINTS * 2;
        let circle: Vec<[f32; 3]> = (0..count)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / count as f32;
                [angle.cos() * 0.05, 0.0, angle.sin() * 0.05]
            })
            .collect();
        let prepared = prepare_track_zone(&circle);
        assert!(prepared.points.len() >= 3);
        assert!(prepared.points.len() <= MAX_ZONE_POINTS);
        assert!(!prepared.triangle_indices.is_empty());
        assert!(zone_area_abs(&prepared.points) > 0.01);
    }

    #[test]
    fn dense_narrow_zone_retains_width_and_fill_topology() {
        let side = MAX_ZONE_POINTS / 2;
        let mut narrow = Vec::with_capacity(side * 4);
        for index in 0..side {
            let t = index as f32 / side as f32;
            narrow.push([t * 10.0, 0.0, 0.0]);
        }
        for index in 0..side {
            let t = index as f32 / side as f32;
            narrow.push([10.0, 0.0, t * 0.01]);
        }
        for index in 0..side {
            let t = index as f32 / side as f32;
            narrow.push([10.0 - t * 10.0, 0.0, 0.01]);
        }
        for index in 0..side {
            let t = index as f32 / side as f32;
            narrow.push([0.0, 0.0, 0.01 - t * 0.01]);
        }
        let prepared = prepare_track_zone(&narrow);
        assert!(prepared.points.len() >= 4);
        assert!(prepared.points.len() <= MAX_ZONE_POINTS);
        assert!(!prepared.triangle_indices.is_empty());
        let min_z = prepared
            .points
            .iter()
            .map(|point| point[2])
            .fold(f32::MAX, f32::min);
        let max_z = prepared
            .points
            .iter()
            .map(|point| point[2])
            .fold(f32::MIN, f32::max);
        assert!(min_z <= 0.0 && max_z >= 0.01);
    }
}
