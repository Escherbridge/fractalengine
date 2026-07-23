//! path_interaction_20260716 — precise ribbon picking geometry (FR-1), path
//! SEGMENT selection (FR-3), and live metric measurement of the edited track.
//! See `fe-ui/src/node_manager/AGENTS.md` §track-picking + §path-segments.

use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority};
use crate::gis::PathEditorState;

/// Vertical lift baked into the rendered ribbon (`fe_terrain`'s `track_mesh`
/// `y_offset`). Picking geometry adds it so the ray test hits where the ribbon
/// is actually drawn, not the raw (un-lifted) gpx points.
const RIBBON_Y_LIFT: f32 = 0.5;
/// Extra pick tolerance beyond the ribbon half-width (world units) so a thin
/// ribbon stays comfortably clickable (FR-1).
const PICK_SLOP: f32 = 0.3;

/// Precise pick geometry for a rendered track ribbon, attached by the
/// fractalengine gpx bridge (`tag_track_lines_selectable`) alongside
/// `SpawnedNodeMarker`. Replaces the giant flat AABB with the actual polyline
/// so a km-scale track no longer swallows clicks meant for nearby objects
/// (FR-1). `points`/`centroid` are RAW world positions (no y-lift); the lift is
/// applied only inside the ray test. `centroid` is the render entity's baseline
/// `Transform` translation, so FR-4 bakes the gimbal delta about it.
#[derive(Component, Debug, Clone)]
pub struct TrackPickShape {
    /// Raw world-space polyline points in track order (no y-lift).
    pub points: Vec<Vec3>,
    /// Half the ribbon width in world units.
    pub half_width: f32,
    /// Centroid of `points` — the render entity's baseline translation.
    pub centroid: Vec3,
}

/// Closest-approach distance between a ray (`origin + t·dir`, `dir` unit, `t ≥
/// 0`) and the finite segment `a→b`, plus the ray `t` at that closest approach.
/// Pure; the FR-1 ribbon pick and FR-3 segment select both build on it.
fn ray_segment_distance(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3) -> (f32, f32) {
    let v = b - a;
    let r = origin - a;
    let vv = v.dot(v);
    let dv = dir.dot(v);
    let dr = dir.dot(r);
    let vr = v.dot(r);
    // det = (v·v) - (d·v)^2 with d a unit vector.
    let det = vv - dv * dv;
    let s = if det.abs() > 1e-8 {
        ((vr - dv * dr) / det).clamp(0.0, 1.0)
    } else if vv > 1e-8 {
        // Ray ~parallel to the segment: project the origin onto the segment.
        (vr / vv).clamp(0.0, 1.0)
    } else {
        0.0 // degenerate (a == b)
    };
    // t = s·(d·v) − (d·r), clamped in front of the origin.
    let t = (s * dv - dr).max(0.0);
    let on_ray = origin + dir * t;
    let on_seg = a + v * s;
    ((on_ray - on_seg).length(), t)
}

/// Nearest ribbon segment the ray hits within `half_width + PICK_SLOP`, as
/// `(segment_index, ray_t)`. Adds the ribbon y-lift so the test matches the
/// drawn ribbon. Among hits it keeps the front-most (smallest `t`), so a nearer
/// real object still out-picks a track behind it. Pure.
fn closest_ribbon_segment(
    points: &[Vec3],
    origin: Vec3,
    dir: Vec3,
    half_width: f32,
) -> Option<(usize, f32)> {
    if points.len() < 2 {
        return None;
    }
    let tol = half_width + PICK_SLOP;
    let lift = Vec3::new(0.0, RIBBON_Y_LIFT, 0.0);
    let mut best: Option<(usize, f32)> = None;
    for i in 0..points.len() - 1 {
        let (dist, t) = ray_segment_distance(origin, dir, points[i] + lift, points[i + 1] + lift);
        if dist <= tol && best.is_none_or(|(_, bt)| t < bt) {
            best = Some((i, t));
        }
    }
    best
}

/// FR-1: along-ray `t` where the ray first hits the ribbon (within tolerance),
/// or `None` on a miss. `viewport_pick` compares this against other nodes' AABB
/// entry `t` so nearer objects win. Pure (Bevy-App-free).
pub(super) fn ray_polyline_hit(
    points: &[Vec3],
    origin: Vec3,
    dir: Vec3,
    half_width: f32,
) -> Option<f32> {
    closest_ribbon_segment(points, origin, dir, half_width).map(|(_, t)| t)
}

/// FR-3: index of the ribbon segment the ray selects (nearest within
/// tolerance), or `None`. Pure over the edited track's raw points.
fn nearest_segment(points: &[Vec3], origin: Vec3, dir: Vec3, half_width: f32) -> Option<usize> {
    closest_ribbon_segment(points, origin, dir, half_width).map(|(i, _)| i)
}

/// Guard `world_scale` (world units per real meter) to a positive, finite value;
/// falls back to `1.0` (human scale) on `≤ 0` / non-finite. Mirrors
/// `fe_terrain::scale::sanitize_world_scale` (fe-ui must not depend on it).
fn guard_world_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Ground-plane (XZ) world distance between two points — height (Y) ignored, to
/// match `fe_terrain::ruler::world_to_real_distance`'s convention.
fn ground_len_world(a: [f32; 3], b: [f32; 3]) -> f64 {
    let dx = a[0] as f64 - b[0] as f64;
    let dz = a[2] as f64 - b[2] as f64;
    (dx * dx + dz * dz).sqrt()
}

// ---------------------------------------------------------------------------
// System: segment selection (FR-3). Claims `PathSegment` — between `PathPlace`
// and `NodePick` — so while editing it wins the re-pick of the ribbon-as-node.
// ---------------------------------------------------------------------------

pub(super) fn handle_path_segment_interaction(
    mut arbiter: ResMut<ClickArbiter>,
    mut path_state: ResMut<PathEditorState>,
) {
    if !arbiter.is_fresh_press() || !arbiter.is_available() {
        return;
    }
    if path_state.editing_track_id.is_none() {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };

    // Half-width from the edited track's live style (petal-local meters, same
    // frame as the rendered ribbon — width is NOT world-scaled); floored so a
    // hair-thin ribbon is still selectable (PICK_SLOP does most of the work).
    let half_width = (path_state.edited_track_style.width * 0.5).max(0.05);
    let pts: Vec<Vec3> = path_state
        .points
        .iter()
        .map(|p| Vec3::from(p.position))
        .collect();

    match nearest_segment(&pts, ray.origin, *ray.direction, half_width) {
        Some(index) => {
            // A marker pick (PathMarker) or gimbal already claimed → claim fails,
            // leaving that selection intact. Otherwise select this segment.
            if arbiter.claim(ClickPriority::PathSegment) {
                path_state.selected_segment = Some(index);
                path_state.selected_point = None;
            }
        }
        None => {
            // Genuine empty click while editing (no higher-priority consumer
            // claimed it) → drop the vertex/segment selection.
            if !arbiter.is_claimed() {
                path_state.selected_point = None;
                path_state.selected_segment = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// System: live metric measurement (FR-3). Computed here (not in the panel)
// because the Paths-tab call site can't reach `PetalMapState.world_scale`.
// ---------------------------------------------------------------------------

pub(super) fn sync_path_measurements(
    mut path_state: ResMut<PathEditorState>,
    petal_map: Res<crate::terrain_map::PetalMapState>,
) {
    if path_state.editing_track_id.is_none() {
        if path_state.total_length_m.is_some() || path_state.selected_segment_length_m.is_some() {
            path_state.total_length_m = None;
            path_state.selected_segment_length_m = None;
        }
        return;
    }
    let scale = guard_world_scale(petal_map.world_scale);
    let total_world: f64 = path_state
        .points
        .windows(2)
        .map(|w| ground_len_world(w[0].position, w[1].position))
        .sum();
    let total_m = Some(total_world / scale);

    let sel_seg = path_state.selected_segment;
    let seg_m = sel_seg.and_then(|i| {
        let p = &path_state.points;
        (i + 1 < p.len()).then(|| ground_len_world(p[i].position, p[i + 1].position) / scale)
    });

    // Conditional writes so change-detection isn't tripped every frame.
    if path_state.total_length_m != total_m {
        path_state.total_length_m = total_m;
    }
    if path_state.selected_segment_length_m != seg_m {
        path_state.selected_segment_length_m = seg_m;
    }
}

// ---------------------------------------------------------------------------
// Tests — pure ray/segment + measurement math (Bevy-App-free).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_segment_straight_down() {
        // Ray from y=5 straight down onto a horizontal segment through origin.
        let (dist, t) = ray_segment_distance(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::NEG_Y,
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
        );
        assert!(dist < 1e-5, "dist = {dist}");
        assert!((t - 5.0).abs() < 1e-4, "t = {t}");
    }

    #[test]
    fn ray_misses_segment_returns_gap() {
        // Parallel ray 5 above a ground segment → closest distance is 5.
        let (dist, _t) = ray_segment_distance(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::X,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
        );
        assert!((dist - 5.0).abs() < 1e-4, "dist = {dist}");
    }

    #[test]
    fn ray_clamps_to_nearest_segment_endpoint() {
        // Ray passes beyond the segment end; closest point is the endpoint.
        let (dist, _t) = ray_segment_distance(
            Vec3::new(20.0, 1.0, 0.0),
            Vec3::NEG_Y,
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
        );
        // Horizontal gap 19 (20 → endpoint x=1) dominates.
        assert!(dist > 18.0, "dist = {dist}");
    }

    fn zig() -> Vec<Vec3> {
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 10.0),
        ]
    }

    #[test]
    fn polyline_hit_selects_first_segment() {
        // Straight-down ray over the first segment's midpoint (x=5).
        let t = ray_polyline_hit(&zig(), Vec3::new(5.0, 5.0, 0.0), Vec3::NEG_Y, 0.25);
        assert!(t.is_some(), "expected a hit");
        // Enters near the lifted ribbon (y = 0.5), i.e. ~4.5 units down.
        assert!((t.unwrap() - 4.5).abs() < 0.1, "t = {:?}", t);
    }

    #[test]
    fn polyline_hit_misses_off_ribbon() {
        // Ray well beside the polyline → no hit within half_width + slop.
        let t = ray_polyline_hit(&zig(), Vec3::new(5.0, 5.0, 20.0), Vec3::NEG_Y, 0.25);
        assert!(t.is_none(), "expected miss, got {:?}", t);
    }

    #[test]
    fn nearest_segment_picks_second_leg() {
        // Straight-down ray over the second segment's midpoint (x=10, z=5).
        let idx = nearest_segment(&zig(), Vec3::new(10.0, 5.0, 5.0), Vec3::NEG_Y, 0.25);
        assert_eq!(idx, Some(1));
    }

    #[test]
    fn nearest_segment_none_when_off_ribbon() {
        let idx = nearest_segment(&zig(), Vec3::new(50.0, 5.0, 50.0), Vec3::NEG_Y, 0.25);
        assert_eq!(idx, None);
    }

    #[test]
    fn nearest_segment_needs_two_points() {
        assert_eq!(
            nearest_segment(&[Vec3::ZERO], Vec3::Y, Vec3::NEG_Y, 1.0),
            None
        );
    }

    #[test]
    fn guard_world_scale_defaults_bad_input() {
        assert_eq!(guard_world_scale(0.001), 0.001);
        assert_eq!(guard_world_scale(1.0), 1.0);
        assert_eq!(guard_world_scale(0.0), 1.0);
        assert_eq!(guard_world_scale(-2.0), 1.0);
        assert_eq!(guard_world_scale(f64::NAN), 1.0);
        assert_eq!(guard_world_scale(f64::INFINITY), 1.0);
    }

    #[test]
    fn ground_len_ignores_height() {
        // 3-4-5 on the ground plane; a big Y delta must not change it.
        let d = ground_len_world([0.0, 0.0, 0.0], [3.0, 100.0, 4.0]);
        assert!((d - 5.0).abs() < 1e-9, "d = {d}");
    }

    #[test]
    fn ground_len_scaled_to_real_meters() {
        // 5 world units at 0.001 wu/m = 5000 real meters.
        let d = ground_len_world([0.0, 0.0, 0.0], [3.0, 0.0, 4.0]) / guard_world_scale(0.001);
        assert!((d - 5000.0).abs() < 1e-6, "d = {d}");
    }
}
