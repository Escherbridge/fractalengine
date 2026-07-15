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
    mut query: bevy::ecs::system::Query<(&mut TrackAnimator, &mut bevy::prelude::Transform)>,
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
        transform.translation =
            bevy::prelude::Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32);

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

/// Per-track render style — color, ribbon width, visibility. A plain data
/// struct (no Bevy dep) so `fe-ui` and `fractalengine` can both reference it
/// without a render-crate cycle. Persisted as node properties (`gis.track.*`)
/// and read back into [`TrackStyleMap`]. See `src/AGENTS.md` §track-styling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackStyle {
    /// Linear RGBA in `0.0..=1.0`. Default matches the historic hardcoded cyan.
    pub color: [f32; 4],
    /// Ribbon width in world units (petal-local meters).
    pub width: f32,
    /// When `false`, the track's mesh is hidden (entity kept for cheap toggle).
    pub visible: bool,
}

impl Default for TrackStyle {
    fn default() -> Self {
        // Matches the pre-styling hardcoded look (`Color::srgb(0.0, 0.8, 1.0)`),
        // a 2 m ribbon, always visible — so untouched tracks are unchanged.
        Self {
            color: [0.0, 0.8, 1.0, 1.0],
            width: 2.0,
            visible: true,
        }
    }
}

impl TrackStyle {
    /// The color as the `[u8; 4]` `mesh::track::ColorMode::Solid` wants (the
    /// ribbon builder re-linearizes it, so pass sRGB-ish bytes straight through).
    pub fn color_u8(&self) -> [u8; 4] {
        [
            (self.color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }
}

/// Parse a `gis.track.color` hex string (`#rrggbb`, `#rrggbbaa`, or the same
/// without the leading `#`) into linear-ish `[f32; 4]` RGBA in `0.0..=1.0`.
/// Any malformed input yields `None` so callers fall back to a default —
/// never panics (FR-4). A missing alpha defaults to fully opaque.
pub fn parse_track_color_hex(hex: &str) -> Option<[f32; 4]> {
    let s = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    let (r, g, b, a) = match s.len() {
        6 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            255u8,
        ),
        8 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            u8::from_str_radix(&s[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

/// Encode a `[f32; 4]` RGBA color as a `#rrggbbaa` hex string for persistence.
pub fn track_color_to_hex(color: [f32; 4]) -> String {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        c(color[0]),
        c(color[1]),
        c(color[2]),
        c(color[3])
    )
}

/// Per-track style lookup keyed by track node id. Populated from persisted
/// `gis.track.*` node properties (by the fractalengine gpx bridge) and read by
/// `render_gpx_tracks`. Absent entries render with [`TrackStyle::default`].
#[cfg(feature = "render")]
#[derive(Debug, Default, bevy::prelude::Resource)]
pub struct TrackStyleMap {
    pub styles: std::collections::HashMap<String, TrackStyle>,
}

#[cfg(feature = "render")]
impl TrackStyleMap {
    /// The style for `track_node_id`, or the default when none is stored.
    pub fn style_for(&self, track_node_id: &str) -> TrackStyle {
        self.styles.get(track_node_id).copied().unwrap_or_default()
    }
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

    #[test]
    fn track_style_default_matches_historic_cyan() {
        // FR-4: the default must reproduce the pre-styling look so untouched
        // tracks are unchanged — cyan, 2 m wide, visible.
        let s = TrackStyle::default();
        assert_eq!(s.color, [0.0, 0.8, 1.0, 1.0]);
        assert_eq!(s.width, 2.0);
        assert!(s.visible);
    }

    #[test]
    fn parse_track_color_hex_accepts_rrggbb_and_rrggbbaa() {
        assert_eq!(parse_track_color_hex("#ff0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_track_color_hex("00ff00"), Some([0.0, 1.0, 0.0, 1.0]));
        // Alpha honored when present.
        assert_eq!(
            parse_track_color_hex("#0000ff80"),
            Some([0.0, 0.0, 1.0, 128.0 / 255.0])
        );
        // Leading/trailing whitespace tolerated.
        assert_eq!(
            parse_track_color_hex("  #ffffff  "),
            Some([1.0, 1.0, 1.0, 1.0])
        );
    }

    #[test]
    fn parse_track_color_hex_rejects_malformed_as_none() {
        // FR-4: invalid → None (caller substitutes the default), never panics.
        assert_eq!(parse_track_color_hex(""), None);
        assert_eq!(parse_track_color_hex("#fff"), None);
        assert_eq!(parse_track_color_hex("#gggggg"), None);
        assert_eq!(parse_track_color_hex("#ff00"), None);
        assert_eq!(parse_track_color_hex("not-a-color"), None);
    }

    #[test]
    fn track_color_roundtrips_through_hex() {
        let hex = track_color_to_hex([1.0, 0.5019608, 0.0, 1.0]);
        assert_eq!(hex, "#ff8000ff");
        assert_eq!(
            parse_track_color_hex(&hex),
            Some([1.0, 128.0 / 255.0, 0.0, 1.0])
        );
    }

    #[test]
    fn track_style_color_u8_clamps_and_rounds() {
        let s = TrackStyle {
            color: [0.0, 0.8, 1.0, 1.0],
            ..TrackStyle::default()
        };
        assert_eq!(s.color_u8(), [0, 204, 255, 255]);
    }
}
