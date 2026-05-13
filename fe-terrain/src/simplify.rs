/// Ramer-Douglas-Peucker polyline simplification for 3D points.
///
/// Removes points that deviate less than `epsilon` (meters) from the simplified
/// line, reducing vertex count while preserving the overall shape.

/// Simplify a 3D polyline using Ramer-Douglas-Peucker.
///
/// Returns a new `Vec` of points. If `points.len() <= 2` or `epsilon <= 0`,
/// returns a clone of the input.
pub fn rdp_simplify(points: &[[f32; 3]], epsilon: f32) -> Vec<[f32; 3]> {
    if points.len() <= 2 || epsilon <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp_recursive(points, 0, points.len() - 1, epsilon, &mut keep);
    points
        .iter()
        .zip(keep.iter())
        .filter_map(|(p, &k)| if k { Some(*p) } else { None })
        .collect()
}

fn rdp_recursive(points: &[[f32; 3]], start: usize, end: usize, epsilon: f32, keep: &mut [bool]) {
    if end <= start + 1 {
        return;
    }

    let mut max_dist = 0.0f32;
    let mut max_idx = start;

    let a = points[start];
    let b = points[end];

    for i in (start + 1)..end {
        let d = point_to_line_dist(points[i], a, b);
        if d > max_dist {
            max_dist = d;
            max_idx = i;
        }
    }

    if max_dist > epsilon {
        keep[max_idx] = true;
        rdp_recursive(points, start, max_idx, epsilon, keep);
        rdp_recursive(points, max_idx, end, epsilon, keep);
    }
}

/// Perpendicular distance from point P to line segment AB in 3D.
fn point_to_line_dist(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];

    let ab_len_sq = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if ab_len_sq < 1e-12 {
        return (ap[0] * ap[0] + ap[1] * ap[1] + ap[2] * ap[2]).sqrt();
    }

    // Cross product magnitude = |AB × AP|
    let cross = [
        ab[1] * ap[2] - ab[2] * ap[1],
        ab[2] * ap[0] - ab[0] * ap[2],
        ab[0] * ap[1] - ab[1] * ap[0],
    ];
    let cross_len = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
    cross_len / ab_len_sq.sqrt()
}

/// Default epsilon threshold (meters) for track mesh simplification.
///
/// Tracks with more than `SIMPLIFY_THRESHOLD` points are automatically
/// simplified before mesh generation.
pub const SIMPLIFY_THRESHOLD: usize = 10_000;
pub const DEFAULT_EPSILON_M: f32 = 1.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_input_unchanged() {
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
        let result = rdp_simplify(&pts, 0.1);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_collinear_points_simplified() {
        // Points along a straight line — middle points should be removed
        let pts: Vec<[f32; 3]> = (0..100)
            .map(|i| [i as f32, 0.0, 0.0])
            .collect();
        let result = rdp_simplify(&pts, 0.1);
        assert_eq!(result.len(), 2); // only start and end
    }

    #[test]
    fn test_zigzag_preserved() {
        // Zigzag pattern — deviations are large, points should be kept
        let pts = vec![
            [0.0, 0.0, 0.0],
            [5.0, 10.0, 0.0],
            [10.0, 0.0, 0.0],
            [15.0, 10.0, 0.0],
            [20.0, 0.0, 0.0],
        ];
        let result = rdp_simplify(&pts, 1.0);
        assert_eq!(result.len(), 5); // all points significant
    }

    #[test]
    fn test_endpoints_always_kept() {
        let pts = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.001, 0.0],
            [2.0, 0.0, 0.0],
        ];
        let result = rdp_simplify(&pts, 0.01);
        assert_eq!(result[0], [0.0, 0.0, 0.0]);
        assert_eq!(*result.last().unwrap(), [2.0, 0.0, 0.0]);
    }

    #[test]
    fn test_zero_epsilon_returns_all() {
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 0.5, 0.0], [2.0, 0.0, 0.0]];
        let result = rdp_simplify(&pts, 0.0);
        assert_eq!(result.len(), 3);
    }
}
