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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(polygon_area_m2(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], 1.0), 0.0);
    }
}
