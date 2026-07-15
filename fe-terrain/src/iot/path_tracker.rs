//! IoT path tracking — snap a position to a recorded route.
//!
//! [`PathTracker`] builds a cumulative-distance index over route points and
//! provides [`PathTracker::snap_to_route`] to find the nearest segment and
//! compute deviation, progress, and distance remaining.

use serde::{Deserialize, Serialize};

/// Result of snapping a position to the route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapResult {
    /// The nearest point on the route (projected onto the segment).
    pub nearest_point: [f64; 3],
    /// Index of the nearest segment (between `route_points[i]` and `route_points[i+1]`).
    pub segment_index: usize,
    /// Progress along the route (0.0 = start, 1.0 = end).
    pub route_progress: f64,
    /// Distance remaining to the end of the route, in meters.
    pub distance_remaining_m: f64,
    /// Perpendicular distance from the position to the route, in meters.
    pub deviation_m: f64,
}

/// A route tracker that maps arbitrary positions to route-relative metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathTracker {
    /// Route points in world-space [x, y, z] (meters).
    pub route_points: Vec<[f64; 3]>,
    /// Cumulative distance at each point (index 0 = 0.0).
    pub cumulative_distances: Vec<f64>,
    /// Total route length in meters.
    pub total_distance: f64,
}

impl PathTracker {
    /// Build a `PathTracker` from a sequence of route points.
    ///
    /// Computes `cumulative_distances` and `total_distance`.
    pub fn new(route_points: Vec<[f64; 3]>) -> Self {
        let mut cumulative_distances = Vec::with_capacity(route_points.len());
        cumulative_distances.push(0.0);

        let mut total = 0.0;
        for pair in route_points.windows(2) {
            let dist = euclidean_distance_3d(&pair[0], &pair[1]);
            total += dist;
            cumulative_distances.push(total);
        }

        Self {
            route_points,
            cumulative_distances,
            total_distance: total,
        }
    }

    /// Snap a world-space position to the route.
    ///
    /// Finds the nearest point on any line segment of the route and returns
    /// a [`SnapResult`] with the projected position, segment index, progress,
    /// remaining distance, and deviation.
    pub fn snap_to_route(&self, position: [f64; 3]) -> SnapResult {
        if self.route_points.is_empty() {
            return SnapResult {
                nearest_point: position,
                segment_index: 0,
                route_progress: 0.0,
                distance_remaining_m: 0.0,
                deviation_m: 0.0,
            };
        }

        if self.route_points.len() == 1 {
            let p = self.route_points[0];
            return SnapResult {
                nearest_point: p,
                segment_index: 0,
                route_progress: 0.0,
                distance_remaining_m: self.total_distance,
                deviation_m: euclidean_distance_3d(&position, &p),
            };
        }

        let mut best_dist_sq = f64::MAX;
        let mut best_proj = position;
        let mut best_seg_idx: usize = 0;

        for i in 0..self.route_points.len() - 1 {
            let a = &self.route_points[i];
            let b = &self.route_points[i + 1];
            let (proj, dist_sq) = nearest_point_on_segment(position, *a, *b);

            if dist_sq < best_dist_sq {
                best_dist_sq = dist_sq;
                best_proj = proj;
                best_seg_idx = i;
            }
        }

        let deviation_m = best_dist_sq.sqrt();

        // Route progress: cumulative distance at segment start + fraction along segment
        let seg_start_dist = self.cumulative_distances[best_seg_idx];
        let seg_len = euclidean_distance_3d(
            &self.route_points[best_seg_idx],
            &self.route_points[best_seg_idx + 1],
        );
        let seg_fraction = if seg_len > 1e-9 {
            euclidean_distance_3d(&self.route_points[best_seg_idx], &best_proj) / seg_len
        } else {
            0.0
        };
        let progress_dist = seg_start_dist + seg_fraction * seg_len;
        let route_progress = if self.total_distance > 1e-9 {
            progress_dist / self.total_distance
        } else {
            0.0
        };
        let distance_remaining_m = self.total_distance - progress_dist;

        SnapResult {
            nearest_point: best_proj,
            segment_index: best_seg_idx,
            route_progress: route_progress.clamp(0.0, 1.0),
            distance_remaining_m: distance_remaining_m.max(0.0),
            deviation_m,
        }
    }

    /// Get the interpolated position at a given route progress (0.0–1.0).
    pub fn position_at_progress(&self, progress: f64) -> [f64; 3] {
        if self.route_points.is_empty() {
            return [0.0, 0.0, 0.0];
        }
        if self.route_points.len() == 1 {
            return self.route_points[0];
        }

        let target_dist = progress.clamp(0.0, 1.0) * self.total_distance;

        // Find the segment containing target_dist.
        // We want the last cumulative distance <= target_dist.
        let mut seg_idx = self
            .cumulative_distances
            .iter()
            .position(|&d| d > target_dist)
            .unwrap_or(self.route_points.len() - 1)
            .saturating_sub(1);

        // Clamp to valid segment range
        seg_idx = seg_idx.min(self.route_points.len() - 2);

        let seg_start = self.cumulative_distances[seg_idx];
        let seg_end = self.cumulative_distances[seg_idx + 1];
        let seg_len = seg_end - seg_start;
        let t = if seg_len > 1e-9 {
            (target_dist - seg_start) / seg_len
        } else {
            0.0
        };

        let a = &self.route_points[seg_idx];
        let b = &self.route_points[seg_idx + 1];
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }
}

/// Euclidean distance squared between two 3D points.
fn euclidean_distance_sq_3d(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    dx * dx + dy * dy + dz * dz
}

/// Euclidean distance between two 3D points.
fn euclidean_distance_3d(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    euclidean_distance_sq_3d(a, b).sqrt()
}

/// Find the nearest point on the line segment AB to point P.
///
/// Returns (nearest_point, distance_squared).
fn nearest_point_on_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> ([f64; 3], f64) {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];

    let ab_dot_ab = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let ab_dot_ap = ab[0] * ap[0] + ab[1] * ap[1] + ab[2] * ap[2];

    // Clamp t to [0, 1] to stay on the segment
    let t = if ab_dot_ab > 1e-12 {
        (ab_dot_ap / ab_dot_ab).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let nearest = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];

    (nearest, euclidean_distance_sq_3d(&p, &nearest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_route() {
        let tracker = PathTracker::new(vec![]);
        let result = tracker.snap_to_route([5.0, 5.0, 5.0]);
        assert_eq!(result.nearest_point, [5.0, 5.0, 5.0]);
        assert_eq!(result.route_progress, 0.0);
    }

    #[test]
    fn test_single_point_route() {
        let tracker = PathTracker::new(vec![[0.0, 0.0, 0.0]]);
        let result = tracker.snap_to_route([3.0, 4.0, 0.0]);
        assert_eq!(result.nearest_point, [0.0, 0.0, 0.0]);
        assert!((result.deviation_m - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_snap_to_segment() {
        // Route: (0,0,0) → (10,0,0) → (10,10,0)
        let tracker = PathTracker::new(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]]);

        // Snap to point near middle of first segment
        let result = tracker.snap_to_route([5.0, 0.5, 0.0]);
        assert_eq!(result.segment_index, 0);
        assert!((result.nearest_point[0] - 5.0).abs() < 1e-9);
        assert!((result.nearest_point[1]).abs() < 1e-9);
        assert!((result.deviation_m - 0.5).abs() < 1e-9);
        assert!((result.route_progress - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_snap_beyond_endpoint() {
        let tracker = PathTracker::new(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);

        // Point beyond the end
        let result = tracker.snap_to_route([15.0, 0.0, 0.0]);
        assert_eq!(result.segment_index, 0);
        assert_eq!(result.nearest_point, [10.0, 0.0, 0.0]);
        assert!((result.route_progress - 1.0).abs() < 1e-9);
        assert!((result.distance_remaining_m).abs() < 1e-9);
    }

    #[test]
    fn test_position_at_progress() {
        let tracker = PathTracker::new(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]]);

        let p0 = tracker.position_at_progress(0.0);
        assert_eq!(p0, [0.0, 0.0, 0.0]);

        let p1 = tracker.position_at_progress(1.0);
        assert_eq!(p1, [10.0, 10.0, 0.0]);

        let p_half = tracker.position_at_progress(0.5);
        // 50% of 20m total = 10m → end of segment 0: [10, 0, 0]
        assert!((p_half[0] - 10.0).abs() < 1e-9);
        assert!((p_half[1]).abs() < 1e-9);

        // 75% of 20m = 15m → halfway through segment 1: [10, 5, 0]
        let p_three_quarters = tracker.position_at_progress(0.75);
        assert!((p_three_quarters[0] - 10.0).abs() < 1e-9);
        assert!((p_three_quarters[1] - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_deviation_alert_threshold() {
        let tracker = PathTracker::new(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);

        // Point 60m away from route
        let result = tracker.snap_to_route([5.0, 60.0, 0.0]);
        assert!((result.deviation_m - 60.0).abs() < 1e-9);
    }
}
