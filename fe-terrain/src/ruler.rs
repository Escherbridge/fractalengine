//! Pure ruler/measurement math (nice-number, distance, bearing, area); see `src/AGENTS.md` §ruler.

/// Snap a real-meter span to a "nice" round value: `1`, `2`, or `5` × `10ⁿ`.
///
/// Used to pick scale-bar lengths and grid spacings that read cleanly
/// (e.g. `100 m`, `200 m`, `500 m`) rather than an arbitrary camera-derived span.
pub fn nice_number(span: f64) -> f64 {
    if !span.is_finite() || span <= 0.0 {
        return 1.0;
    }
    let exponent = span.log10().floor();
    let magnitude = 10f64.powf(exponent);
    let fraction = span / magnitude;
    let nice_fraction = if fraction < 1.5 {
        1.0
    } else if fraction < 3.5 {
        2.0
    } else if fraction < 7.5 {
        5.0
    } else {
        10.0
    };
    nice_fraction * magnitude
}

/// Real-meter ground-plane (XZ) distance between two world-space points, given
/// world units per real meter (`scale`, same convention as `TerrainConfig::world_scale`).
/// Y (height) is ignored, matching `bearing_deg`/`polygon_area_m2`.
pub fn world_to_real_distance(a: [f64; 3], b: [f64; 3], scale: f64) -> f64 {
    let sanitized = crate::scale::sanitize_world_scale(scale);
    let dx = (b[0] - a[0]) / sanitized;
    let dz = (b[2] - a[2]) / sanitized;
    (dx * dx + dz * dz).sqrt()
}

/// Compass bearing in degrees `[0, 360)` from point `a` to `b` on the XZ
/// ground plane (Bevy convention: X = east, Z = south; north = -Z).
/// Y (height) is ignored — bearing is a ground-plane heading.
pub fn bearing_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    // atan2(east, north) with north = -Z gives a compass bearing (0 = north, CW positive).
    let raw = dx.atan2(-dz).to_degrees();
    (raw + 360.0) % 360.0
}

/// Planar area (m²) of a closed polygon on the XZ ground plane via the
/// shoelace formula. `vertices` need not be explicitly closed (first ≠ last
/// is fine); fewer than 3 vertices returns `0.0`.
pub fn polygon_area_m2(vertices: &[[f64; 3]], scale: f64) -> f64 {
    if vertices.len() < 3 {
        return 0.0;
    }
    let sanitized = crate::scale::sanitize_world_scale(scale);
    let mut sum = 0.0;
    for i in 0..vertices.len() {
        let [x1, _, z1] = vertices[i];
        let [x2, _, z2] = vertices[(i + 1) % vertices.len()];
        sum += x1 * z2 - x2 * z1;
    }
    (sum.abs() / 2.0) / (sanitized * sanitized)
}

/// A chosen scale bar: nice-number real length and its on-screen pixel width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleBarSpec {
    /// Real-world length the bar represents (meters).
    pub meters: f64,
    /// On-screen bar width (pixels).
    pub pixels: f64,
}

/// Ground meters per screen pixel for a down-looking perspective camera
/// (`cam_height_m` real meters, vertical FOV radians, viewport height px);
/// `0.0` on degenerate input.
pub fn meters_per_pixel(cam_height_m: f64, fov_y_rad: f64, viewport_h_px: f64) -> f64 {
    let valid_height = cam_height_m.is_finite() && cam_height_m > 0.0;
    let valid_fov = fov_y_rad.is_finite() && fov_y_rad > 0.0 && fov_y_rad < std::f64::consts::PI;
    let valid_vp = viewport_h_px.is_finite() && viewport_h_px > 0.0;
    if !(valid_height && valid_fov && valid_vp) {
        return 0.0;
    }
    2.0 * cam_height_m * (fov_y_rad / 2.0).tan() / viewport_h_px
}

/// Pick the nice-number scale bar nearest a `target_px` on-screen width;
/// `None` on degenerate input.
pub fn scale_bar_spec(meters_per_px: f64, target_px: f64) -> Option<ScaleBarSpec> {
    let valid_mpp = meters_per_px.is_finite() && meters_per_px > 0.0;
    let valid_px = target_px.is_finite() && target_px > 0.0;
    if !(valid_mpp && valid_px) {
        return None;
    }
    let meters = nice_number(meters_per_px * target_px);
    Some(ScaleBarSpec {
        meters,
        pixels: meters / meters_per_px,
    })
}

/// Human-readable distance label: m below 1 km, km above, sensible precision.
pub fn format_distance_m(meters: f64) -> String {
    if !meters.is_finite() || meters <= 0.0 {
        return "0 m".to_string();
    }
    if meters >= 1000.0 {
        let km = meters / 1000.0;
        if km >= 10.0 || km.fract().abs() < 1e-9 {
            format!("{km:.0} km")
        } else {
            format!("{km:.1} km")
        }
    } else if meters >= 1.0 {
        if meters.fract().abs() < 1e-9 {
            format!("{meters:.0} m")
        } else {
            format!("{meters:.1} m")
        }
    } else {
        format!("{meters:.2} m")
    }
}

/// Adaptive world-grid spacing: a major/minor pair on the 1/2/5×10ⁿ ladder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpacing {
    /// Major line spacing (meters).
    pub major_m: f64,
    /// Minor line spacing (meters): the ladder step below `major_m`.
    pub minor_m: f64,
    /// Minor-line opacity `[0, 1]`: fades out as the major spacing nears its
    /// next ladder step, so re-subdivision on zoom never pops.
    pub minor_alpha: f32,
}

/// The 1/2/5×10ⁿ ladder value strictly below `x` (positive, finite `x`).
fn next_nice_down(x: f64) -> f64 {
    let magnitude = 10f64.powf(x.log10().floor());
    [5.0, 2.0, 1.0, 0.5]
        .into_iter()
        .map(|f| f * magnitude)
        .find(|candidate| *candidate < x * (1.0 - 1e-9))
        .unwrap_or(magnitude * 0.5)
}

/// The raw-span band `[lo, hi)` that [`nice_number`] snaps onto `major`.
fn nice_band(major: f64) -> (f64, f64) {
    let magnitude = 10f64.powf(major.log10().floor());
    let fraction = major / magnitude;
    let (lo, hi) = if fraction < 1.5 {
        (0.75, 1.5)
    } else if fraction < 3.5 {
        (1.5, 3.5)
    } else {
        (3.5, 7.5)
    };
    (lo * magnitude, hi * magnitude)
}

/// Grid spacing for a camera `cam_height_m` (real meters above the ground
/// plane) aiming for about `target_major_lines` majors across the view
/// (visible span ≈ `2·h`, the 90° down-looking approximation). `None` on
/// degenerate input. See `src/AGENTS.md` §ruler.
pub fn grid_spacing_for_height(cam_height_m: f64, target_major_lines: f64) -> Option<GridSpacing> {
    let valid_height = cam_height_m.is_finite() && cam_height_m > 0.0;
    let valid_target = target_major_lines.is_finite() && target_major_lines > 0.0;
    if !(valid_height && valid_target) {
        return None;
    }
    let raw = 2.0 * cam_height_m / target_major_lines;
    if !(raw.is_finite() && raw > 0.0) {
        return None;
    }
    let major_m = nice_number(raw);
    let minor_m = next_nice_down(major_m);
    // Log-space position of `raw` inside the band that snaps to `major_m`:
    // 0 at the band floor (minor fully shown) → 1 at the next ladder step.
    let (lo, hi) = nice_band(major_m);
    let t = ((raw / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0);
    Some(GridSpacing {
        major_m,
        minor_m,
        minor_alpha: (1.0 - t) as f32,
    })
}

/// Whole-grid opacity by camera height: `1` at or below `fade_start_m`, `0`
/// at or above `fade_end_m`, linear between; `0` on non-finite input.
pub fn grid_fade_alpha(cam_height_m: f64, fade_start_m: f64, fade_end_m: f64) -> f32 {
    if !(cam_height_m.is_finite() && fade_start_m.is_finite() && fade_end_m.is_finite()) {
        return 0.0;
    }
    if cam_height_m <= fade_start_m {
        return 1.0;
    }
    if cam_height_m >= fade_end_m {
        return 0.0;
    }
    (1.0 - (cam_height_m - fade_start_m) / (fade_end_m - fade_start_m)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `v` is 1, 2, or 5 × 10ⁿ.
    fn on_ladder(v: f64) -> bool {
        let fraction = v / 10f64.powf(v.log10().floor());
        [1.0, 2.0, 5.0, 10.0]
            .iter()
            .any(|f| (fraction - f).abs() < 1e-6)
    }

    #[test]
    fn grid_spacing_majors_sit_on_the_ladder_at_site_to_region_heights() {
        // (camera height m, expected major m) with ~10 majors across 2·h.
        for (height, want) in [
            (10.0, 2.0),
            (150.0, 20.0),
            (2_000.0, 500.0),
            (40_000.0, 10_000.0),
        ] {
            let spacing = grid_spacing_for_height(height, 10.0).expect("valid input");
            assert_eq!(spacing.major_m, want, "height {height}");
            assert!(on_ladder(spacing.minor_m), "minor {}", spacing.minor_m);
            assert!(spacing.minor_m < spacing.major_m);
            assert!((0.0..=1.0).contains(&spacing.minor_alpha));
        }
    }

    #[test]
    fn grid_minor_is_the_next_ladder_step_down() {
        for (major, minor) in [
            (1.0, 0.5),
            (2.0, 1.0),
            (5.0, 2.0),
            (10.0, 5.0),
            (200.0, 100.0),
        ] {
            assert_eq!(next_nice_down(major), minor, "major {major}");
        }
    }

    #[test]
    fn grid_spacing_is_monotone_in_height_and_minor_fades_before_resubdivision() {
        let mut previous_major = 0.0;
        let mut height = 1.0;
        while height < 1.0e6 {
            let spacing = grid_spacing_for_height(height, 10.0).unwrap();
            assert!(on_ladder(spacing.major_m), "major {}", spacing.major_m);
            assert!(
                spacing.major_m >= previous_major,
                "major shrank at height {height}"
            );
            previous_major = spacing.major_m;
            height *= 1.07;
        }
        // Just below a re-subdivision (raw 1.49 → major 1) the minor lines
        // are almost gone; just above it (raw 1.5 → major 2) they are full.
        let before = grid_spacing_for_height(1.49 * 5.0, 10.0).unwrap();
        let after = grid_spacing_for_height(1.5 * 5.0, 10.0).unwrap();
        assert_eq!((before.major_m, after.major_m), (1.0, 2.0));
        assert!(before.minor_alpha < 0.05, "{}", before.minor_alpha);
        assert!(after.minor_alpha > 0.95, "{}", after.minor_alpha);
    }

    #[test]
    fn grid_spacing_rejects_degenerate_input() {
        for (height, target) in [
            (0.0, 10.0),
            (-5.0, 10.0),
            (f64::NAN, 10.0),
            (f64::INFINITY, 10.0),
            (100.0, 0.0),
            (100.0, f64::NAN),
        ] {
            assert!(grid_spacing_for_height(height, target).is_none());
        }
    }

    #[test]
    fn grid_fade_alpha_table() {
        for (height, want) in [
            (10.0, 1.0),
            (1_000.0, 1.0),
            (1_500.0, 0.5),
            (2_000.0, 0.0),
            (9_999.0, 0.0),
            (f64::NAN, 0.0),
        ] {
            assert!(
                (grid_fade_alpha(height, 1_000.0, 2_000.0) - want).abs() < 1e-6,
                "height {height}"
            );
        }
        assert_eq!(grid_fade_alpha(10.0, f64::NAN, 2_000.0), 0.0);
    }

    #[test]
    fn nice_number_snaps_to_1_2_5_sequence() {
        assert_eq!(nice_number(1.2), 1.0);
        assert_eq!(nice_number(1.8), 2.0);
        assert_eq!(nice_number(3.0), 2.0);
        assert_eq!(nice_number(4.0), 5.0);
        assert_eq!(nice_number(8.0), 10.0);
        assert_eq!(nice_number(120.0), 100.0);
        assert_eq!(nice_number(180.0), 200.0);
        assert_eq!(nice_number(450.0), 500.0);
    }

    #[test]
    fn nice_number_boundary_cases() {
        assert_eq!(nice_number(1.5), 2.0);
        assert_eq!(nice_number(3.5), 5.0);
        assert_eq!(nice_number(7.5), 10.0);
    }

    #[test]
    fn nice_number_falls_back_on_bad_input() {
        assert_eq!(nice_number(0.0), 1.0);
        assert_eq!(nice_number(-5.0), 1.0);
        assert_eq!(nice_number(f64::NAN), 1.0);
        assert_eq!(nice_number(f64::INFINITY), 1.0);
    }

    #[test]
    fn world_to_real_distance_known_3_4_5_triangle() {
        // Ground-plane (XZ): a 3-on-X, 4-on-Z offset is a 5-unit distance.
        let d = world_to_real_distance([0.0, 0.0, 0.0], [3.0, 0.0, 4.0], 1.0);
        assert!((d - 5.0).abs() < 1e-9);
    }

    #[test]
    fn world_to_real_distance_ignores_height() {
        // Two points differing only in Y (height) are the same ground-plane point.
        let d = world_to_real_distance([1.0, 0.0, 2.0], [1.0, 100.0, 2.0], 1.0);
        assert_eq!(d, 0.0);
    }

    #[test]
    fn world_to_real_distance_scales_inversely() {
        // Ground-plane (XZ) at 0.001 world units per meter: 5 world units = 5000 real meters.
        let d = world_to_real_distance([0.0, 0.0, 0.0], [3.0, 0.0, 4.0], 0.001);
        assert!((d - 5000.0).abs() < 1e-6);
    }

    #[test]
    fn bearing_deg_cardinal_directions() {
        // North: b is directly -Z from a.
        assert!((bearing_deg([0.0, 0.0, 0.0], [0.0, 0.0, -10.0]) - 0.0).abs() < 1e-9);
        // East: b is directly +X from a.
        assert!((bearing_deg([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]) - 90.0).abs() < 1e-9);
        // South: b is directly +Z from a.
        assert!((bearing_deg([0.0, 0.0, 0.0], [0.0, 0.0, 10.0]) - 180.0).abs() < 1e-9);
        // West: b is directly -X from a.
        assert!((bearing_deg([0.0, 0.0, 0.0], [-10.0, 0.0, 0.0]) - 270.0).abs() < 1e-9);
    }

    #[test]
    fn bearing_deg_stays_in_0_360_range() {
        for &(a, b) in &[
            ([0.0, 0.0, 0.0], [1.0, 0.0, 1.0]),
            ([0.0, 0.0, 0.0], [-1.0, 0.0, 1.0]),
            ([0.0, 0.0, 0.0], [-1.0, 0.0, -1.0]),
            ([0.0, 0.0, 0.0], [1.0, 0.0, -1.0]),
        ] {
            let deg = bearing_deg(a, b);
            assert!((0.0..360.0).contains(&deg), "bearing {deg} out of range");
        }
    }

    #[test]
    fn polygon_area_known_square() {
        // A 10x10 square on the XZ plane at 1:1 scale has area 100 m^2.
        let square = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 10.0],
            [0.0, 0.0, 10.0],
        ];
        let area = polygon_area_m2(&square, 1.0);
        assert!((area - 100.0).abs() < 1e-9);
    }

    #[test]
    fn polygon_area_scales_with_world_scale_squared() {
        let square = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 10.0],
            [0.0, 0.0, 10.0],
        ];
        // At 0.1 world units per meter, a 10x10 world square is 100x100 real
        // meters -> area 10000 m^2.
        let area = polygon_area_m2(&square, 0.1);
        assert!((area - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn polygon_area_degenerate_returns_zero() {
        assert_eq!(polygon_area_m2(&[], 1.0), 0.0);
        assert_eq!(polygon_area_m2(&[[0.0, 0.0, 0.0]], 1.0), 0.0);
        assert_eq!(
            polygon_area_m2(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], 1.0),
            0.0
        );
    }

    #[test]
    fn meters_per_pixel_known_geometry() {
        // fov = 90° → tan(fov/2) = 1 → visible span = 2h; 1000 px viewport.
        let mpp = meters_per_pixel(1000.0, std::f64::consts::FRAC_PI_2, 1000.0);
        assert!((mpp - 2.0).abs() < 1e-9);
    }

    #[test]
    fn meters_per_pixel_degenerate_inputs_return_zero() {
        assert_eq!(meters_per_pixel(0.0, 1.0, 1000.0), 0.0);
        assert_eq!(meters_per_pixel(-10.0, 1.0, 1000.0), 0.0);
        assert_eq!(meters_per_pixel(f64::NAN, 1.0, 1000.0), 0.0);
        assert_eq!(meters_per_pixel(100.0, 0.0, 1000.0), 0.0);
        assert_eq!(meters_per_pixel(100.0, std::f64::consts::PI, 1000.0), 0.0);
        assert_eq!(meters_per_pixel(100.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn scale_bar_spec_snaps_to_nice_number() {
        // 2 m/px × 120 px target = 240 m → nice 200 m → 100 px wide.
        let spec = scale_bar_spec(2.0, 120.0).unwrap();
        assert_eq!(spec.meters, 200.0);
        assert!((spec.pixels - 100.0).abs() < 1e-9);
    }

    #[test]
    fn scale_bar_spec_none_on_degenerate_input() {
        assert!(scale_bar_spec(0.0, 120.0).is_none());
        assert!(scale_bar_spec(f64::NAN, 120.0).is_none());
        assert!(scale_bar_spec(2.0, 0.0).is_none());
    }

    #[test]
    fn scale_bar_from_height_and_world_scale_end_to_end() {
        // Camera at world-Y 500 under 0.001 scale → 500 km real height (FR-8 path:
        // world_to_real_height → meters_per_pixel → scale_bar_spec).
        let real_h = crate::scale::world_to_real_height(500.0, 0.001);
        let mpp = meters_per_pixel(real_h, std::f64::consts::FRAC_PI_2, 1000.0);
        assert!((mpp - 1000.0).abs() < 1e-9);
        let spec = scale_bar_spec(mpp, 120.0).unwrap();
        assert_eq!(spec.meters, 100_000.0); // nice(120 km) = 100 km
        assert!((spec.pixels - 100.0).abs() < 1e-9);
        assert_eq!(format_distance_m(spec.meters), "100 km");
    }

    #[test]
    fn format_distance_meters_range() {
        assert_eq!(format_distance_m(500.0), "500 m");
        assert_eq!(format_distance_m(2.0), "2 m");
        assert_eq!(format_distance_m(2.5), "2.5 m");
        assert_eq!(format_distance_m(0.5), "0.50 m");
    }

    #[test]
    fn format_distance_kilometers_range() {
        assert_eq!(format_distance_m(1000.0), "1 km");
        assert_eq!(format_distance_m(1500.0), "1.5 km");
        assert_eq!(format_distance_m(2000.0), "2 km");
        assert_eq!(format_distance_m(50_000.0), "50 km");
    }

    #[test]
    fn format_distance_bad_input_is_zero() {
        assert_eq!(format_distance_m(0.0), "0 m");
        assert_eq!(format_distance_m(-5.0), "0 m");
        assert_eq!(format_distance_m(f64::NAN), "0 m");
    }
}
