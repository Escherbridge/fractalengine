//! fe-ui-LOCAL pure mirror of fe-terrain's ruler geometry. fe-ui must NOT
//! depend on fe-terrain, so the pure formulas are copied verbatim (not
//! imported). Callers add the "no map scale" chip when `world_scale` is unset —
//! this module is honest about it: an unusable scale sanitizes to `1.0`, so the
//! returned numbers are world units, not meters. See `fe-ui/src/AGENTS.md`
//! §geometry-mirror.

/// Sanitize a world scale (world units per real meter): finite and `> 0`, else
/// `1.0`. Mirror of `fe_terrain::scale::sanitize_world_scale` (do NOT import).
fn sanitize_world_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Real-meter ground-plane (XZ) distance between two world-space points, given
/// world units per real meter (`scale`). Y (height) is ignored, matching
/// `bearing_deg`/`polygon_area_m2`. Mirror of `fe_terrain::ruler::world_to_real_distance`.
pub fn world_to_real_distance(a: [f64; 3], b: [f64; 3], scale: f64) -> f64 {
    let sanitized = sanitize_world_scale(scale);
    let dx = (b[0] - a[0]) / sanitized;
    let dz = (b[2] - a[2]) / sanitized;
    (dx * dx + dz * dz).sqrt()
}

/// Compass bearing in degrees `[0, 360)` from point `a` to `b` on the XZ ground
/// plane (Bevy convention: X = east, Z = south; north = -Z). Y (height) is
/// ignored. Mirror of `fe_terrain::ruler::bearing_deg`.
pub fn bearing_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    let raw = dx.atan2(-dz).to_degrees();
    (raw + 360.0) % 360.0
}

/// Planar area (m²) of a closed polygon on the XZ ground plane via the shoelace
/// formula. `vertices` need not be explicitly closed; fewer than 3 vertices
/// returns `0.0`. Mirror of `fe_terrain::ruler::polygon_area_m2`.
pub fn polygon_area_m2(vertices: &[[f64; 3]], scale: f64) -> f64 {
    if vertices.len() < 3 {
        return 0.0;
    }
    let sanitized = sanitize_world_scale(scale);
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
    fn sanitize_rejects_non_positive_and_non_finite() {
        assert_eq!(sanitize_world_scale(0.5), 0.5);
        assert_eq!(sanitize_world_scale(0.0), 1.0);
        assert_eq!(sanitize_world_scale(-2.0), 1.0);
        assert_eq!(sanitize_world_scale(f64::NAN), 1.0);
        assert_eq!(sanitize_world_scale(f64::INFINITY), 1.0);
    }

    #[test]
    fn world_to_real_distance_known_3_4_5_triangle() {
        // Ground-plane (XZ): a 3-on-X, 4-on-Z offset is a 5-unit distance.
        let d = world_to_real_distance([0.0, 0.0, 0.0], [3.0, 0.0, 4.0], 1.0);
        assert!((d - 5.0).abs() < 1e-9);
    }

    #[test]
    fn world_to_real_distance_ignores_height() {
        let d = world_to_real_distance([1.0, 0.0, 2.0], [1.0, 100.0, 2.0], 1.0);
        assert_eq!(d, 0.0);
    }

    #[test]
    fn world_to_real_distance_scales_inversely() {
        // 0.001 world units per meter: 5 world units = 5000 real meters.
        let d = world_to_real_distance([0.0, 0.0, 0.0], [3.0, 0.0, 4.0], 0.001);
        assert!((d - 5000.0).abs() < 1e-6);
    }

    #[test]
    fn world_to_real_distance_unset_scale_returns_world_units() {
        // Honest: scale <= 0 sanitizes to 1.0, so the number is world units.
        let d = world_to_real_distance([0.0, 0.0, 0.0], [3.0, 0.0, 4.0], 0.0);
        assert!((d - 5.0).abs() < 1e-9);
    }

    #[test]
    fn bearing_deg_cardinal_directions() {
        assert!((bearing_deg([0.0, 0.0, 0.0], [0.0, 0.0, -10.0]) - 0.0).abs() < 1e-9);
        assert!((bearing_deg([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]) - 90.0).abs() < 1e-9);
        assert!((bearing_deg([0.0, 0.0, 0.0], [0.0, 0.0, 10.0]) - 180.0).abs() < 1e-9);
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
        assert!((polygon_area_m2(&square, 1.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn polygon_area_scales_with_world_scale_squared() {
        let square = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 10.0],
            [0.0, 0.0, 10.0],
        ];
        // 0.1 world units per meter: 10x10 world square is 100x100 real meters
        // -> area 10000 m^2.
        assert!((polygon_area_m2(&square, 0.1) - 10_000.0).abs() < 1e-6);
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
}
