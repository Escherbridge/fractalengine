//! Pure, `f64`, bevy-free pose math for agent vision captures: `tile`/`geo`
//! pose resolution and tileset coverage checks. See `src/AGENTS.md`
//! §camera-framing for the axis convention and why this stays decoupled from
//! `fe-renderer` (which owns the bevy-side `Transform` construction).

use crate::projection::Projection;
use crate::scale::scale_local;
use crate::tiles::source::TileCoord;

/// A camera pose expressed purely in world units (post-`world_scale`), ready
/// for `fe-renderer::pose_transform` to turn into a bevy `Transform`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramedPose {
    pub position: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
}

/// Whether a tile's geographic extent overlaps a tileset's served bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Overlaps at least one tileset's bounds.
    Inside,
    /// Disjoint from every tileset's bounds.
    Outside,
    /// No tilesets to check against (online/demo mode); the capture proceeds.
    Unbounded,
}

/// Geographic bounds `[min_lat, min_lon, max_lat, max_lon]` (WGS84) a slippy
/// tile covers — its NW corner to the NW corner of the `(x+1, y+1)` tile, the
/// same "full extent, not just the NW point" rule as
/// `tiles::hexon_source::HexonTileSource::covers`.
pub fn tile_bounds(zoom: u8, x: u32, y: u32) -> [f64; 4] {
    let (nw_lat, nw_lon) = TileCoord::new(x, y, zoom).to_lat_lon();
    let (se_lat, se_lon) =
        TileCoord::new(x.saturating_add(1), y.saturating_add(1), zoom).to_lat_lon();
    [
        se_lat.min(nw_lat),
        nw_lon.min(se_lon),
        se_lat.max(nw_lat),
        nw_lon.max(se_lon),
    ]
}

/// Rectangle intersection between two `[min_lat, min_lon, max_lat, max_lon]`
/// bounds boxes.
pub fn bounds_intersect(a: [f64; 4], b: [f64; 4]) -> bool {
    let [a_min_lat, a_min_lon, a_max_lat, a_max_lon] = a;
    let [b_min_lat, b_min_lon, b_max_lat, b_max_lon] = b;
    a_min_lat <= b_max_lat
        && a_max_lat >= b_min_lat
        && a_min_lon <= b_max_lon
        && a_max_lon >= b_min_lon
}

/// Checks a tile's bounds against every known tileset's bounds. Takes plain
/// bounds arrays (not `fe_format::manifest::TilesetMeta`) so this module
/// never depends on fe-format — the caller passes
/// `CompositeTileSource::source_metas().iter().map(|m| m.bounds)`.
pub fn coverage_check(tile_bounds: [f64; 4], tileset_bounds: &[[f64; 4]]) -> Coverage {
    if tileset_bounds.is_empty() {
        return Coverage::Unbounded;
    }
    if tileset_bounds
        .iter()
        .any(|&b| bounds_intersect(tile_bounds, b))
    {
        Coverage::Inside
    } else {
        Coverage::Outside
    }
}

/// `true` when `(lat, lon)` is within the flat projection's accurate range
/// (~500 km from origin, `projection.rs`'s own documented bound). `false` on
/// non-finite input or a projection failure (out-of-range WGS84 coordinates).
pub fn within_projection_range(projection: &Projection, lat: f64, lon: f64) -> bool {
    if !lat.is_finite() || !lon.is_finite() {
        return false;
    }
    match projection.wgs84_to_local(lat, lon, 0.0) {
        Ok([x, _, z]) => x.hypot(z) <= 500_000.0,
        Err(_) => false,
    }
}

/// Floor so a degenerate (point-sized) bounds box never yields a
/// zero-distance (position == target) camera.
const MIN_FRAME_DISTANCE_M: f64 = 1e-3;

/// Frames a geographic bounds box top-down, north-up (`up = +z`, looking
/// `-Y`), at a distance that fits BOTH the lat and lon extents for the given
/// output `aspect` (width / height). `fov_v_rad` is the vertical field of
/// view in radians; the horizontal half-angle is derived from it via
/// `tan(h/2) = tan(v/2) * aspect` (the standard aspect/FOV relationship — no
/// separate horizontal FOV input needed).
///
/// `None` on non-finite/non-positive input or a projection failure. Reuses
/// `Projection::wgs84_to_local` + `scale_local` — the exact pipeline
/// `terrain_plugin::spawn_chunk` uses for its tile anchor — so a framed
/// tile's centre lands in the same world-space frame as the chunk itself.
pub fn frame_bounds_top_down(
    projection: &Projection,
    world_scale: f64,
    bounds: [f64; 4],
    ground_y: f64,
    fov_v_rad: f64,
    aspect: f64,
) -> Option<FramedPose> {
    let [min_lat, min_lon, max_lat, max_lon] = bounds;
    if ![min_lat, min_lon, max_lat, max_lon, world_scale, ground_y]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    if world_scale <= 0.0 {
        return None;
    }
    if !(fov_v_rad.is_finite() && fov_v_rad > 0.0 && fov_v_rad < std::f64::consts::PI) {
        return None;
    }
    if !(aspect.is_finite() && aspect > 0.0) {
        return None;
    }

    let corners = [
        (min_lat, min_lon),
        (min_lat, max_lon),
        (max_lat, min_lon),
        (max_lat, max_lon),
    ];
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    for (lat, lon) in corners {
        let local = projection.wgs84_to_local(lat, lon, 0.0).ok()?;
        let world = scale_local(local, world_scale);
        min_x = min_x.min(world[0]);
        max_x = max_x.max(world[0]);
        min_z = min_z.min(world[2]);
        max_z = max_z.max(world[2]);
    }

    let center_lat = (min_lat + max_lat) / 2.0;
    let center_lon = (min_lon + max_lon) / 2.0;
    let center_local = projection.wgs84_to_local(center_lat, center_lon, 0.0).ok()?;
    let center_world = scale_local(center_local, world_scale);

    let half_w = (max_x - min_x) / 2.0;
    let half_h = (max_z - min_z) / 2.0;
    let tan_half_v = (fov_v_rad / 2.0).tan();
    let tan_half_h = tan_half_v * aspect;
    let dist_for_v = half_h / tan_half_v;
    let dist_for_h = half_w / tan_half_h;
    let distance = (dist_for_v.max(dist_for_h) * 1.1).max(MIN_FRAME_DISTANCE_M * world_scale);

    Some(FramedPose {
        position: [center_world[0], ground_y + distance, center_world[2]],
        target: [center_world[0], ground_y, center_world[2]],
        up: [0.0, 0.0, 1.0],
    })
}

/// Resolves a `geo` pose spec to world-space position/target. `heading_deg`
/// 0 = north (+z), clockwise toward +x (east); `pitch_deg` is the look angle
/// (negative looks down). `ground_or_origin_m` is the absolute WGS84
/// elevation to add `altitude_m` to — the caller resolves whether that is
/// resident terrain height ("terrain" reference) or the projection's
/// `origin_ele` ("origin" reference); this function only does the geometry.
///
/// `None` on non-finite input, a non-positive `world_scale`, or a projection
/// failure (lat/lon outside WGS84 bounds).
#[allow(clippy::too_many_arguments)]
pub fn geo_pose(
    projection: &Projection,
    world_scale: f64,
    lat: f64,
    lon: f64,
    ground_or_origin_m: f64,
    altitude_m: f64,
    heading_deg: f64,
    pitch_deg: f64,
) -> Option<FramedPose> {
    if ![
        lat,
        lon,
        ground_or_origin_m,
        altitude_m,
        heading_deg,
        pitch_deg,
        world_scale,
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return None;
    }
    if world_scale <= 0.0 {
        return None;
    }

    let ele = ground_or_origin_m + altitude_m;
    let local = projection.wgs84_to_local(lat, lon, ele).ok()?;
    let position = scale_local(local, world_scale);

    let heading_rad = heading_deg.to_radians();
    let pitch_rad = pitch_deg.to_radians();
    let cos_pitch = pitch_rad.cos();
    // "One unit along the view direction" (spec §5.3): the target is not
    // framed distance-wise, only direction-wise — `pose_transform` only needs
    // a point ahead of `position`, not a specific look-at range.
    let direction = [
        cos_pitch * heading_rad.sin(),
        pitch_rad.sin(),
        cos_pitch * heading_rad.cos(),
    ];
    let target = [
        position[0] + direction[0],
        position[1] + direction[1],
        position[2] + direction[2],
    ];

    Some(FramedPose {
        position,
        target,
        up: [0.0, 1.0, 0.0],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-6;

    #[test]
    fn tile_bounds_match_the_slippy_formula() {
        // z0: the whole world, Web-Mercator-capped at ±85.0511.
        let whole_world = tile_bounds(0, 0, 0);
        assert!((whole_world[0] - (-85.0511)).abs() < 1e-3, "{whole_world:?}");
        assert!((whole_world[1] - (-180.0)).abs() < 1e-6, "{whole_world:?}");
        assert!((whole_world[2] - 85.0511).abs() < 1e-3, "{whole_world:?}");
        assert!((whole_world[3] - 180.0).abs() < 1e-6, "{whole_world:?}");

        // z1 (0,0): the NW quadrant — lat [0, 85.0511], lon [-180, 0].
        let nw_quadrant = tile_bounds(1, 0, 0);
        assert!((nw_quadrant[0] - 0.0).abs() < 1e-6, "{nw_quadrant:?}");
        assert!((nw_quadrant[1] - (-180.0)).abs() < 1e-6, "{nw_quadrant:?}");
        assert!((nw_quadrant[2] - 85.0511).abs() < 1e-3, "{nw_quadrant:?}");
        assert!((nw_quadrant[3] - 0.0).abs() < 1e-6, "{nw_quadrant:?}");

        // A known z11 tile: internally consistent (min <= max on both axes,
        // and matches the NW/(x+1,y+1) corner formula directly).
        let (z, x, y) = (11, 327, 791);
        let got = tile_bounds(z, x, y);
        let (nw_lat, nw_lon) = TileCoord::new(x, y, z).to_lat_lon();
        let (se_lat, se_lon) = TileCoord::new(x + 1, y + 1, z).to_lat_lon();
        assert!(got[0] <= got[2] && got[1] <= got[3], "{got:?}");
        assert!((got[0] - se_lat.min(nw_lat)).abs() < EPS);
        assert!((got[2] - se_lat.max(nw_lat)).abs() < EPS);
        assert!((got[1] - nw_lon.min(se_lon)).abs() < EPS);
        assert!((got[3] - nw_lon.max(se_lon)).abs() < EPS);
    }

    #[test]
    fn framed_tile_centre_is_the_chunk_anchor_space_point() {
        let projection = Projection::new(37.0, -122.0, 0.0);
        let scale = 0.01;
        let bounds = tile_bounds(14, 2621, 6333);
        let [min_lat, min_lon, max_lat, max_lon] = bounds;
        let center_lat = (min_lat + max_lat) / 2.0;
        let center_lon = (min_lon + max_lon) / 2.0;

        let pose =
            frame_bounds_top_down(&projection, scale, bounds, 0.0, 45.0_f64.to_radians(), 16.0 / 9.0)
                .expect("valid frame");

        // Same pipeline terrain_plugin::spawn_chunk uses for its tile anchor:
        // Projection::wgs84_to_local × scale_local.
        let expected_local = projection.wgs84_to_local(center_lat, center_lon, 0.0).unwrap();
        let expected_world = scale_local(expected_local, scale);

        assert!((pose.target[0] - expected_world[0]).abs() < EPS);
        assert!((pose.target[2] - expected_world[2]).abs() < EPS);
        assert_eq!(pose.target[1], 0.0, "target sits at the requested ground_y");
    }

    #[test]
    fn frame_fits_both_axes_for_wide_and_tall_aspects() {
        let projection = Projection::new(0.0, 0.0, 0.0);
        let bounds = [-1.0_f64, -1.0, 1.0, 1.0];
        let fov_v = 45.0_f64.to_radians();

        for aspect in [16.0 / 9.0, 9.0 / 16.0, 1.0] {
            let pose = frame_bounds_top_down(&projection, 1.0, bounds, 0.0, fov_v, aspect)
                .expect("valid frame");
            let distance = pose.position[1] - pose.target[1];

            // Recompute the half-extents independently and assert the chosen
            // distance is large enough to fit both — the frustum half-width
            // at `distance` must cover `half_w`, half-height must cover
            // `half_h` (within the documented 1.1x margin).
            let local_corners = [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)]
                .map(|(lat, lon)| projection.wgs84_to_local(lat, lon, 0.0).unwrap());
            let (min_x, max_x) = local_corners
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), c| {
                    (mn.min(c[0]), mx.max(c[0]))
                });
            let (min_z, max_z) = local_corners
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), c| {
                    (mn.min(c[2]), mx.max(c[2]))
                });
            let half_w = (max_x - min_x) / 2.0;
            let half_h = (max_z - min_z) / 2.0;
            let tan_half_v = (fov_v / 2.0).tan();
            let tan_half_h = tan_half_v * aspect;

            let frustum_half_w_at_distance = distance * tan_half_h;
            let frustum_half_h_at_distance = distance * tan_half_v;
            assert!(
                frustum_half_w_at_distance >= half_w - 1e-6,
                "aspect {aspect}: frustum half-width {frustum_half_w_at_distance} must cover {half_w}"
            );
            assert!(
                frustum_half_h_at_distance >= half_h - 1e-6,
                "aspect {aspect}: frustum half-height {frustum_half_h_at_distance} must cover {half_h}"
            );
        }
    }

    #[test]
    fn coverage_outside_inside_unbounded_table() {
        let tile = tile_bounds(10, 100, 200);

        assert_eq!(coverage_check(tile, &[]), Coverage::Unbounded);
        assert_eq!(coverage_check(tile, &[tile]), Coverage::Inside);

        // A tileset far from the tile's bounds.
        let disjoint = [60.0, 60.0, 61.0, 61.0];
        assert_eq!(coverage_check(tile, &[disjoint]), Coverage::Outside);

        // One disjoint, one overlapping: Inside wins.
        assert_eq!(coverage_check(tile, &[disjoint, tile]), Coverage::Inside);
    }

    #[test]
    fn geo_heading_zero_looks_north_and_ninety_looks_east() {
        let projection = Projection::new(0.0, 0.0, 0.0);

        let north = geo_pose(&projection, 1.0, 0.0, 0.0, 0.0, 300.0, 0.0, -45.0).unwrap();
        assert!(
            north.target[2] > north.position[2],
            "heading 0 must look north (+z): {north:?}"
        );

        let east = geo_pose(&projection, 1.0, 0.0, 0.0, 0.0, 300.0, 90.0, -45.0).unwrap();
        assert!(
            east.target[0] > east.position[0],
            "heading 90 must look east (+x): {east:?}"
        );
    }

    #[test]
    fn geo_position_round_trips_through_local_to_wgs84() {
        let projection = Projection::new(37.5, -122.3, 10.0);
        let (lat, lon, altitude_m) = (37.52, -122.28, 300.0);
        let ground_or_origin_m = 10.0;

        let pose = geo_pose(&projection, 1.0, lat, lon, ground_or_origin_m, altitude_m, 15.0, -30.0)
            .unwrap();
        let (got_lat, got_lon, got_ele) =
            projection.local_to_wgs84(pose.position[0], pose.position[1], pose.position[2]);

        assert!((got_lat - lat).abs() < 1e-9, "lat round-trip: {got_lat} vs {lat}");
        assert!((got_lon - lon).abs() < 1e-9, "lon round-trip: {got_lon} vs {lon}");
        assert!(
            (got_ele - (ground_or_origin_m + altitude_m)).abs() < 1e-9,
            "ele round-trip: {got_ele} vs {}",
            ground_or_origin_m + altitude_m
        );
    }

    #[test]
    fn geo_beyond_500km_is_out_of_range() {
        let projection = Projection::new(0.0, 0.0, 0.0);
        // ~1500 km away.
        assert!(!within_projection_range(&projection, 10.0, 10.0));
        // A few hundred meters away.
        assert!(within_projection_range(&projection, 0.001, 0.001));
        assert!(!within_projection_range(&projection, f64::NAN, 0.0));
    }

    #[test]
    fn frame_and_geo_pose_reject_non_finite_and_degenerate_input() {
        let projection = Projection::new(0.0, 0.0, 0.0);
        let bounds = [-1.0_f64, -1.0, 1.0, 1.0];
        let fov_v = 45.0_f64.to_radians();

        assert!(frame_bounds_top_down(&projection, f64::NAN, bounds, 0.0, fov_v, 1.0).is_none());
        assert!(frame_bounds_top_down(&projection, 0.0, bounds, 0.0, fov_v, 1.0).is_none());
        assert!(frame_bounds_top_down(&projection, 1.0, bounds, 0.0, 0.0, 1.0).is_none());
        assert!(frame_bounds_top_down(&projection, 1.0, bounds, 0.0, fov_v, -1.0).is_none());
        assert!(frame_bounds_top_down(&projection, 1.0, [f64::NAN, -1.0, 1.0, 1.0], 0.0, fov_v, 1.0)
            .is_none());

        assert!(geo_pose(&projection, f64::NAN, 0.0, 0.0, 0.0, 300.0, 0.0, -45.0).is_none());
        assert!(geo_pose(&projection, 1.0, f64::NAN, 0.0, 0.0, 300.0, 0.0, -45.0).is_none());
        assert!(geo_pose(&projection, 0.0, 0.0, 0.0, 0.0, 300.0, 0.0, -45.0).is_none());
    }
}
