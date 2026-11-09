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
        let (cumulative_distances, total_distance) =
            fe_sdk::path_geometry::cumulative_distances_f64(&route_points);
        Self {
            route_points,
            cumulative_distances,
            total_distance,
        }
    }

    /// Snap a world-space position to the route (delegates to `fe_sdk::path_geometry`).
    pub fn snap_to_route(&self, position: [f64; 3]) -> SnapResult {
        let snap = fe_sdk::path_geometry::snap_to_polyline(
            &self.route_points,
            &self.cumulative_distances,
            self.total_distance,
            position,
        );
        SnapResult {
            nearest_point: snap.nearest_point,
            segment_index: snap.segment_index,
            route_progress: snap.route_progress,
            distance_remaining_m: snap.distance_remaining_m,
            deviation_m: snap.deviation_m,
        }
    }

    /// Get the interpolated position at a given route progress (0.0–1.0).
    pub fn position_at_progress(&self, progress: f64) -> [f64; 3] {
        fe_sdk::path_geometry::position_at_progress_f64(
            &self.route_points,
            &self.cumulative_distances,
            self.total_distance,
            progress,
        )
    }
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
