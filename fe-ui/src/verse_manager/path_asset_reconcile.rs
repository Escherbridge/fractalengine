//! Path-asset stamp sampler + live-edit cache feeder. The arc-length sampler
//! (`cumulative_distances`/`position_at_progress`/`sample_progresses`/
//! `tangent_yaw`/`sample_transforms`) turns a `path_asset` descriptor
//! (`fe_sdk::path_asset::PathAssetDescriptor`) + a path's points into a set of
//! `(position, yaw)` stamp transforms; `spacing_value` is interpreted in real
//! METERS and converted to world units via the petal `world_scale`.
//!
//! The `reconcile_path_asset` system no longer spawns: it FEEDS the shared
//! `PathAssetCache` with the Paths-tab edited track's live descriptor + points,
//! so the single `materialize_path_assets` system does all spawning (no
//! double-stamping). See `fe-ui/src/verse_manager/AGENTS.md`
//! §path-asset-materialization.

use bevy::prelude::*;
use fe_sdk::path_asset::{PathAssetDescriptor, SpacingMode};

use super::path_asset_materialize::PathAssetCache;
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;

// ---------------------------------------------------------------------------
// Live-edit cache feeder
// ---------------------------------------------------------------------------

/// Feed the Paths-tab **edited** track's live stamp descriptor + points into the
/// shared [`PathAssetCache`], keyed on `PathEditorState.editing_track_id` (NOT
/// the viewport/tree selection). The descriptor is
/// `PathEditorState.edited_track_path_asset` and the points come straight from
/// `PathEditorState.points` (the freshest buffer while dragging), so live edits
/// restamp promptly. `materialize_path_assets` consumes the cache and does all
/// spawning — this system never touches scene entities, which is what makes
/// double-stamping impossible. Only the active petal; a no-op cache write when
/// nothing changed (keeps the materializer idle). See
/// `fe-ui/src/verse_manager/AGENTS.md` §path-asset-materialization.
pub(super) fn reconcile_path_asset(
    nav: Res<NavigationManager>,
    path_state: Res<PathEditorState>,
    mut cache: ResMut<PathAssetCache>,
) {
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        return;
    };
    let Some(track_id) = path_state.editing_track_id.as_deref() else {
        return;
    };
    let Some(descriptor) = path_state.edited_track_path_asset.as_ref() else {
        return;
    };

    let points: Vec<[f32; 3]> = path_state.points.iter().map(|p| p.position).collect();
    let fingerprint = points_fingerprint(&points);

    // Conditional upsert: only mutate (and thus mark the cache changed) when the
    // live descriptor/points/petal actually differ from what's cached — so the
    // materializer stays idle when the user isn't editing.
    let changed = match cache.get(track_id) {
        Some(e) => {
            e.petal_id != petal_id || &e.descriptor != descriptor || e.fingerprint != fingerprint
        }
        None => true,
    };
    if changed {
        cache.upsert(track_id, petal_id, descriptor.clone(), points, fingerprint);
    }
}

/// A cheap order-sensitive fingerprint of the path points so the reconcile can
/// detect a changed `gpx_points` without storing the whole list. Bit-casts
/// each coordinate so `NaN`/`-0.0` compare deterministically.
pub(super) fn points_fingerprint(points: &[[f32; 3]]) -> u64 {
    const FNV_OFFSET: u64 = 1469598103934665603;
    const FNV_PRIME: u64 = 1099511628211;
    let mut hash = FNV_OFFSET;
    for p in points {
        for c in p {
            hash ^= c.to_bits() as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash ^= points.len() as u64;
    hash
}

// ---------------------------------------------------------------------------
// fe-ui-local arc-length sampler ([f32;3]-based; port of fe-terrain PathTracker)
// ---------------------------------------------------------------------------

/// Per-point cumulative distance (index 0 = 0.0) + the total path length.
fn cumulative_distances(points: &[[f32; 3]]) -> (Vec<f32>, f32) {
    let mut cum = Vec::with_capacity(points.len());
    if points.is_empty() {
        return (cum, 0.0);
    }
    cum.push(0.0);
    let mut total = 0.0;
    for pair in points.windows(2) {
        total += distance_3d(&pair[0], &pair[1]);
        cum.push(total);
    }
    (cum, total)
}

/// Interpolated position at `progress` (clamped 0..1) along the path by
/// arc length. Empty → origin; single point → that point.
fn position_at_progress(
    points: &[[f32; 3]],
    cumdist: &[f32],
    total: f32,
    progress: f32,
) -> [f32; 3] {
    match points.len() {
        0 => return [0.0, 0.0, 0.0],
        1 => return points[0],
        _ => {}
    }
    let target = progress.clamp(0.0, 1.0) * total;

    // Last cumulative distance <= target; clamp to a valid segment.
    let mut seg = cumdist
        .iter()
        .position(|&d| d > target)
        .unwrap_or(points.len() - 1)
        .saturating_sub(1);
    seg = seg.min(points.len() - 2);

    let seg_start = cumdist[seg];
    let seg_end = cumdist[seg + 1];
    let seg_len = seg_end - seg_start;
    let t = if seg_len > 1e-9 {
        (target - seg_start) / seg_len
    } else {
        0.0
    };

    let a = &points[seg];
    let b = &points[seg + 1];
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Hard cap on instances per stamp so a tiny spacing / huge count can't spawn
/// millions of scene entities and wedge the renderer. Sampling saturates here.
const MAX_STAMPS: usize = 4096;

/// The arc-length progress values (0..1) at which to place instances for a
/// descriptor. `FixedSpacing`: 0, spacing, 2·spacing, … clamped to total, where
/// `spacing_value` is real METERS converted to world units via `world_scale`
/// (FR-3). `FixedCount`: `count` values evenly across the path (scale-invariant).
/// Guards degenerate inputs (empty path, non-positive spacing, count 0/1)
/// without dividing by zero, and saturates at [`MAX_STAMPS`].
#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN spacing/total must take the degenerate branch
fn sample_progresses(
    points: &[[f32; 3]],
    descriptor: &PathAssetDescriptor,
    total: f32,
    world_scale: f32,
) -> Vec<f32> {
    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        return vec![0.0];
    }
    match descriptor.spacing_mode {
        SpacingMode::FixedSpacing => {
            // FR-3: spacing_value is in real meters; convert to world units.
            let spacing = descriptor.spacing_value * sanitize_world_scale(world_scale);
            if !(spacing > 0.0) || !(total > 0.0) {
                // No valid spacing → just stamp the endpoints (start + end).
                return vec![0.0, 1.0];
            }
            let n = (total / spacing).floor() as usize; // segments
            let count = (n + 1).min(MAX_STAMPS); // inclusive of the start point, capped
            (0..count)
                .map(|i| ((i as f32 * spacing) / total).clamp(0.0, 1.0))
                .collect()
        }
        SpacingMode::FixedCount => match descriptor.count as usize {
            0 => Vec::new(),
            1 => vec![0.0],
            c => {
                let c = c.min(MAX_STAMPS);
                (0..c).map(|i| i as f32 / (c - 1) as f32).collect()
            }
        },
    }
}

/// Yaw (radians) facing the local path direction at `progress`, for
/// `Quat::from_rotation_y`. Uses the tangent from `progress` toward a point
/// slightly ahead so the model's -Z forward (glTF convention) points down the
/// path. Returns 0.0 for degenerate paths.
#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN total must take the degenerate branch
fn tangent_yaw(points: &[[f32; 3]], cumdist: &[f32], total: f32, progress: f32) -> f32 {
    if points.len() < 2 || !(total > 0.0) {
        return 0.0;
    }
    // A small look-ahead in arc-length space; clamp so the end still has a
    // backward-looking tangent rather than a zero delta.
    let ahead = (progress + (0.01_f32).max(1.0 / total)).min(1.0);
    let (from_p, to_p) = if ahead > progress {
        (
            position_at_progress(points, cumdist, total, progress),
            position_at_progress(points, cumdist, total, ahead),
        )
    } else {
        // At the very end: look backward and keep the same heading.
        let behind = (progress - (0.01_f32).max(1.0 / total)).max(0.0);
        (
            position_at_progress(points, cumdist, total, behind),
            position_at_progress(points, cumdist, total, progress),
        )
    };
    let dx = to_p[0] - from_p[0];
    let dz = to_p[2] - from_p[2];
    if dx.abs() < 1e-9 && dz.abs() < 1e-9 {
        return 0.0;
    }
    // atan2(dx, dz): rotation about +Y that aims the model's -Z forward at (dx,dz).
    dx.atan2(dz)
}

/// The full set of `(position, yaw)` stamp transforms for a descriptor over a
/// path. `yaw` is meaningful only when `descriptor.tangent_align`. `world_scale`
/// (world units per meter) converts the descriptor's metric spacing to world
/// units (FR-3); pass `1.0` for human scale. Shared by `materialize_path_assets`.
pub(super) fn sample_transforms(
    points: &[[f32; 3]],
    descriptor: &PathAssetDescriptor,
    world_scale: f32,
) -> Vec<([f32; 3], f32)> {
    let (cumdist, total) = cumulative_distances(points);
    sample_progresses(points, descriptor, total, world_scale)
        .into_iter()
        .map(|progress| {
            let pos = position_at_progress(points, &cumdist, total, progress);
            let yaw = tangent_yaw(points, &cumdist, total, progress);
            (pos, yaw)
        })
        .collect()
}

/// Sanitize a `world_scale` (world units per meter) for the metric-spacing
/// conversion: any non-finite or non-positive value collapses to `1.0` (human
/// scale) so a bad petal config never zeroes or flips the spacing (FR-3).
#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN scale must take the degenerate branch
pub(super) fn sanitize_world_scale(world_scale: f32) -> f32 {
    if world_scale.is_finite() && world_scale > 0.0 {
        world_scale
    } else {
        1.0
    }
}

/// Euclidean distance between two 3D points.
fn distance_3d(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight_path() -> Vec<[f32; 3]> {
        // 3 points, total length 20 along X.
        vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]]
    }

    fn l_shaped_path() -> Vec<[f32; 3]> {
        // Along +X for 10, then along +Z for 10. total = 20.
        vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0]]
    }

    fn desc(mode: SpacingMode, spacing: f32, count: u32, tangent: bool) -> PathAssetDescriptor {
        PathAssetDescriptor {
            asset_path: "blob://x.glb".to_string(),
            spacing_mode: mode,
            spacing_value: spacing,
            count,
            tangent_align: tangent,
        }
    }

    #[test]
    fn cumulative_distances_straight() {
        let (cum, total) = cumulative_distances(&straight_path());
        assert_eq!(cum, vec![0.0, 10.0, 20.0]);
        assert_eq!(total, 20.0);
    }

    #[test]
    fn cumulative_distances_empty_and_single() {
        let (cum, total) = cumulative_distances(&[]);
        assert!(cum.is_empty());
        assert_eq!(total, 0.0);
        let (cum, total) = cumulative_distances(&[[1.0, 2.0, 3.0]]);
        assert_eq!(cum, vec![0.0]);
        assert_eq!(total, 0.0);
    }

    #[test]
    fn position_at_progress_endpoints_and_mid() {
        let p = straight_path();
        let (cum, total) = cumulative_distances(&p);
        assert_eq!(position_at_progress(&p, &cum, total, 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(position_at_progress(&p, &cum, total, 1.0), [20.0, 0.0, 0.0]);
        let mid = position_at_progress(&p, &cum, total, 0.5);
        assert!(
            (mid[0] - 10.0).abs() < 1e-4,
            "50% of 20 = 10 along X, got {mid:?}"
        );
    }

    #[test]
    fn position_at_progress_degenerate_inputs() {
        assert_eq!(position_at_progress(&[], &[], 0.0, 0.5), [0.0, 0.0, 0.0]);
        let single = [[5.0, 6.0, 7.0]];
        let (cum, total) = cumulative_distances(&single);
        assert_eq!(
            position_at_progress(&single, &cum, total, 0.3),
            [5.0, 6.0, 7.0]
        );
    }

    #[test]
    fn position_at_progress_clamps_out_of_range() {
        let p = straight_path();
        let (cum, total) = cumulative_distances(&p);
        assert_eq!(position_at_progress(&p, &cum, total, -1.0), [0.0, 0.0, 0.0]);
        assert_eq!(position_at_progress(&p, &cum, total, 2.0), [20.0, 0.0, 0.0]);
    }

    #[test]
    fn fixed_spacing_even_counts() {
        // total 20, spacing 5 → segments floor(20/5)=4, +1 = 5 instances.
        let d = desc(SpacingMode::FixedSpacing, 5.0, 0, false);
        let out = sample_transforms(&straight_path(), &d, 1.0);
        assert_eq!(out.len(), 5);
        let xs: Vec<f32> = out.iter().map(|(p, _)| p[0]).collect();
        for (i, x) in xs.iter().enumerate() {
            assert!((x - (i as f32 * 5.0)).abs() < 1e-3, "instance {i} at x={x}");
        }
    }

    #[test]
    fn fixed_spacing_zero_spacing_returns_endpoints() {
        let d = desc(SpacingMode::FixedSpacing, 0.0, 0, false);
        let out = sample_transforms(&straight_path(), &d, 1.0);
        assert_eq!(
            out.len(),
            2,
            "zero spacing must not divide-by-zero; endpoints only"
        );
        assert_eq!(out[0].0, [0.0, 0.0, 0.0]);
        assert_eq!(out[1].0, [20.0, 0.0, 0.0]);
    }

    #[test]
    fn fixed_spacing_negative_spacing_returns_endpoints() {
        let d = desc(SpacingMode::FixedSpacing, -3.0, 0, false);
        let out = sample_transforms(&straight_path(), &d, 1.0);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn fixed_count_distribution() {
        // 5 instances at progress 0, .25, .5, .75, 1 → x = 0,5,10,15,20.
        let d = desc(SpacingMode::FixedCount, 0.0, 5, false);
        let out = sample_transforms(&straight_path(), &d, 1.0);
        assert_eq!(out.len(), 5);
        let xs: Vec<f32> = out.iter().map(|(p, _)| p[0]).collect();
        let expected = [0.0, 5.0, 10.0, 15.0, 20.0];
        for (x, e) in xs.iter().zip(expected.iter()) {
            assert!((x - e).abs() < 1e-3, "got {x}, expected {e}");
        }
    }

    #[test]
    fn fixed_count_zero_is_empty() {
        let d = desc(SpacingMode::FixedCount, 0.0, 0, false);
        assert!(sample_transforms(&straight_path(), &d, 1.0).is_empty());
    }

    #[test]
    fn fixed_count_one_is_start_only() {
        let d = desc(SpacingMode::FixedCount, 0.0, 1, false);
        let out = sample_transforms(&straight_path(), &d, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn fixed_count_saturates_at_cap() {
        let d = desc(SpacingMode::FixedCount, 0.0, 1_000_000, false);
        assert_eq!(
            sample_transforms(&straight_path(), &d, 1.0).len(),
            MAX_STAMPS
        );
    }

    #[test]
    fn fixed_spacing_tiny_spacing_saturates_at_cap() {
        // total 20, spacing 0.0001 → ~200k requested; capped.
        let d = desc(SpacingMode::FixedSpacing, 0.0001, 0, false);
        assert_eq!(
            sample_transforms(&straight_path(), &d, 1.0).len(),
            MAX_STAMPS
        );
    }

    #[test]
    fn empty_path_yields_no_stamps() {
        let d = desc(SpacingMode::FixedCount, 0.0, 5, false);
        assert!(sample_transforms(&[], &d, 1.0).is_empty());
        let d2 = desc(SpacingMode::FixedSpacing, 2.0, 0, false);
        assert!(sample_transforms(&[], &d2, 1.0).is_empty());
    }

    #[test]
    fn single_point_path_yields_one_stamp() {
        let d = desc(SpacingMode::FixedSpacing, 2.0, 0, false);
        let out = sample_transforms(&[[3.0, 0.0, 4.0]], &d, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, [3.0, 0.0, 4.0]);
    }

    #[test]
    fn tangent_yaw_on_straight_x_path_faces_positive_x() {
        // Moving along +X: dir=(1,0), yaw = atan2(1, 0) = +PI/2.
        let p = straight_path();
        let (cum, total) = cumulative_distances(&p);
        let yaw = tangent_yaw(&p, &cum, total, 0.0);
        assert!(
            (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "got {yaw}"
        );
    }

    #[test]
    fn tangent_yaw_on_l_shaped_path_turns() {
        // First leg heads +X (yaw ~ +PI/2); after the corner heads +Z (yaw ~ 0).
        let p = l_shaped_path();
        let (cum, total) = cumulative_distances(&p);
        let yaw_start = tangent_yaw(&p, &cum, total, 0.0);
        let yaw_end = tangent_yaw(&p, &cum, total, 1.0);
        assert!(
            (yaw_start - std::f32::consts::FRAC_PI_2).abs() < 1e-2,
            "start heads +X, got {yaw_start}"
        );
        assert!(yaw_end.abs() < 1e-2, "end heads +Z (yaw~0), got {yaw_end}");
    }

    #[test]
    fn fingerprint_changes_with_points() {
        let a = points_fingerprint(&straight_path());
        let b = points_fingerprint(&l_shaped_path());
        assert_ne!(a, b);
        // Same points → same fingerprint (deterministic).
        assert_eq!(a, points_fingerprint(&straight_path()));
    }

    #[test]
    fn fingerprint_sensitive_to_order() {
        let mut reversed = straight_path();
        reversed.reverse();
        assert_ne!(
            points_fingerprint(&straight_path()),
            points_fingerprint(&reversed)
        );
    }

    #[test]
    fn sanitize_world_scale_guards_degenerate_values() {
        assert_eq!(sanitize_world_scale(0.001), 0.001);
        assert_eq!(sanitize_world_scale(1.0), 1.0);
        // Non-positive / non-finite collapse to human scale (never zero/flip).
        assert_eq!(sanitize_world_scale(0.0), 1.0);
        assert_eq!(sanitize_world_scale(-3.0), 1.0);
        assert_eq!(sanitize_world_scale(f32::NAN), 1.0);
        assert_eq!(sanitize_world_scale(f32::INFINITY), 1.0);
    }

    #[test]
    fn metric_spacing_converts_meters_to_world_units() {
        // FR-3: spacing_value is meters. At world_scale = 0.5 (0.5 world units
        // per meter), a 10 m spacing = 5 world units → floor(20/5)=4, +1 = 5.
        let d = desc(SpacingMode::FixedSpacing, 10.0, 0, false);
        let out = sample_transforms(&straight_path(), &d, 0.5);
        assert_eq!(out.len(), 5);
        let xs: Vec<f32> = out.iter().map(|(p, _)| p[0]).collect();
        for (i, x) in xs.iter().enumerate() {
            assert!((x - (i as f32 * 5.0)).abs() < 1e-3, "instance {i} at x={x}");
        }
    }

    #[test]
    fn metric_spacing_human_scale_is_identity() {
        // world_scale = 1.0 → spacing_m used directly (10 m over total 20 → 3).
        let d = desc(SpacingMode::FixedSpacing, 10.0, 0, false);
        let out = sample_transforms(&straight_path(), &d, 1.0);
        assert_eq!(out.len(), 3); // x = 0, 10, 20
    }

    #[test]
    fn metric_spacing_degenerate_scale_falls_back_to_human() {
        // A bad world_scale (≤0 / non-finite) must not zero or flip spacing:
        // it collapses to 1.0, so 5 m spacing over total 20 → 5 instances.
        let d = desc(SpacingMode::FixedSpacing, 5.0, 0, false);
        for bad in [0.0_f32, -2.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                sample_transforms(&straight_path(), &d, bad).len(),
                5,
                "degenerate world_scale {bad} must fall back to human scale"
            );
        }
    }

    #[test]
    fn metric_spacing_does_not_affect_fixed_count() {
        // FixedCount is scale-invariant: count instances regardless of scale.
        let d = desc(SpacingMode::FixedCount, 0.0, 4, false);
        assert_eq!(sample_transforms(&straight_path(), &d, 0.001).len(), 4);
        assert_eq!(sample_transforms(&straight_path(), &d, 1000.0).len(), 4);
    }
}
