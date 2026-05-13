//! IoT track animation — animate an entity along a recorded route.
//!
//! When the `render` feature is enabled, `TrackAnimator` is a Bevy [`Component`]
//! and the `advance_track_animations` system updates entity transforms every frame.
//!
//! Without `render`, the struct is still available for non-Bevy contexts.

#[cfg(feature = "render")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, bevy::prelude::Component)]
pub struct TrackAnimator {
    /// The node ID of the track route this animator follows.
    pub track_node_id: String,
    /// Playback speed multiplier (1.0 = real-time based on trackpoint timestamps).
    pub playback_speed: f32,
    /// Current animation time in seconds.
    pub current_time: f64,
    /// Whether the animation is currently playing.
    pub playing: bool,
}

#[cfg(not(feature = "render"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackAnimator {
    /// The node ID of the track route this animator follows.
    pub track_node_id: String,
    /// Playback speed multiplier (1.0 = real-time based on trackpoint timestamps).
    pub playback_speed: f32,
    /// Current animation time in seconds.
    pub current_time: f64,
    /// Whether the animation is currently playing.
    pub playing: bool,
}

impl TrackAnimator {
    pub fn new(track_node_id: String) -> Self {
        Self {
            track_node_id,
            playback_speed: 1.0,
            current_time: 0.0,
            playing: false,
        }
    }
}

/// Interpolated position from a set of route points with timestamps.
#[derive(Debug, Clone)]
pub struct TimestampedRoutePoint {
    pub position: [f64; 3],
    pub time_seconds: f64,
}

/// Advance track animations for all entities with a `TrackAnimator`.
///
/// This system reads each animator's `current_time` and `playing` flag,
/// interpolates the position between trackpoints, and updates the entity's
/// `Transform` component.
///
/// Requires a route map that maps track node IDs to route points.
#[cfg(feature = "render")]
pub fn advance_track_animations(
    mut query: bevy::ecs::system::Query<(
        &mut TrackAnimator,
        &mut bevy::prelude::Transform,
    )>,
    route_map: bevy::ecs::system::Res<TrackRouteMap>,
    time: bevy::ecs::system::Res<bevy::prelude::Time>,
) {
    for (mut animator, mut transform) in query.iter_mut() {
        if !animator.playing {
            continue;
        }

        let routes = route_map.routes.get(&animator.track_node_id);
        let Some(route) = routes else {
            continue;
        };

        if route.points.is_empty() {
            continue;
        }

        // Advance time
        animator.current_time += time.delta_secs_f64() * animator.playback_speed as f64;

        // Build timestamped points
        let total_duration = route.total_duration_secs;
        if total_duration <= 0.0 {
            continue;
        }

        let progress = (animator.current_time / total_duration).clamp(0.0, 1.0);

        // Interpolate position
        let pos = interpolate_route(&route.points, progress);
        transform.translation = bevy::prelude::Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32);

        // Loop or stop
        if animator.current_time >= total_duration {
            animator.current_time = 0.0; // loop
        }
    }
}

/// Route data stored in a Bevy resource for animation lookup.
#[cfg(feature = "render")]
#[derive(Debug, Default, bevy::prelude::Resource)]
pub struct TrackRouteMap {
    pub routes: std::collections::HashMap<String, TrackRoute>,
}

/// A single track route with timestamps.
#[cfg(feature = "render")]
#[derive(Debug, Clone)]
pub struct TrackRoute {
    pub points: Vec<TimestampedRoutePoint>,
    pub total_duration_secs: f64,
}

/// Interpolate a position along timestamped route points at a given progress.
#[cfg(feature = "render")]
fn interpolate_route(points: &[TimestampedRoutePoint], progress: f64) -> [f64; 3] {
    if points.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    if points.len() == 1 {
        return points[0].position;
    }

    let target_time = progress * points.last().unwrap().time_seconds;

    // Find the segment: last point with time <= target_time
    let mut seg_idx = points
        .iter()
        .position(|p| p.time_seconds > target_time)
        .unwrap_or(points.len() - 1)
        .saturating_sub(1);

    // Clamp to valid segment range
    seg_idx = seg_idx.min(points.len() - 2);

    let a = &points[seg_idx];
    let b = &points[seg_idx + 1];
    let duration = b.time_seconds - a.time_seconds;
    let t = if duration > 1e-9 {
        (target_time - a.time_seconds) / duration
    } else {
        0.0
    };

    [
        a.position[0] + (b.position[0] - a.position[0]) * t,
        a.position[1] + (b.position[1] - a.position[1]) * t,
        a.position[2] + (b.position[2] - a.position[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_animator_creation() {
        let animator = TrackAnimator::new("track_123".into());
        assert_eq!(animator.track_node_id, "track_123");
        assert_eq!(animator.playback_speed, 1.0);
        assert!(!animator.playing);
    }
}
