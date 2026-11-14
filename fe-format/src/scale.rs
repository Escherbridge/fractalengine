//! Canonical world-scale accessor (world units per real meter); Bevy-free.

use serde_json::Value;

/// Smallest accepted world scale (1:1,000,000; below a zoom-0 tileset's native scale).
pub const MIN_WORLD_SCALE: f64 = 1e-6;

/// Largest accepted world scale (10,000 world units per real meter).
pub const MAX_WORLD_SCALE: f64 = 10_000.0;

/// Sanitize a world scale: inside `[MIN_WORLD_SCALE, MAX_WORLD_SCALE]`, else `1.0` (NaN and infinities included).
pub fn sanitize_world_scale(scale: f64) -> f64 {
    if is_world_scale_in_range(scale) {
        scale
    } else {
        1.0
    }
}

/// Whether a world scale is inside the global range (write-side validation for terrain documents).
pub fn is_world_scale_in_range(scale: f64) -> bool {
    (MIN_WORLD_SCALE..=MAX_WORLD_SCALE).contains(&scale)
}

/// Sanitize, then clamp into valid `[min, max]` bounds narrowed to the global range; malformed bounds are ignored.
pub fn clamp_world_scale_to_bounds(scale: f64, bounds: Option<[f64; 2]>) -> f64 {
    let sanitized = sanitize_world_scale(scale);
    match bounds {
        Some([min, max]) if min.is_finite() && max.is_finite() && min > 0.0 && max >= min => {
            let (low, high) = (min.max(MIN_WORLD_SCALE), max.min(MAX_WORLD_SCALE));
            if low <= high {
                sanitized.clamp(low, high)
            } else {
                sanitized
            }
        }
        _ => sanitized,
    }
}

/// Real meters to world units.
pub fn meters_to_world(meters: f64, scale: f64) -> f64 {
    meters * sanitize_world_scale(scale)
}

/// World units to real meters.
pub fn world_to_meters(world: f64, scale: f64) -> f64 {
    world / sanitize_world_scale(scale)
}

/// Effective world scale from a petal terrain JSON blob; ignores the `enabled` flag (spec FR-1).
pub fn world_scale_from_terrain_json(terrain: Option<&Value>) -> f64 {
    let Some(object) = terrain.and_then(Value::as_object) else {
        return 1.0;
    };
    let Some(world_scale) = object.get("world_scale").and_then(Value::as_f64) else {
        return 1.0;
    };
    let bounds = object
        .get("scale_bounds")
        .and_then(|value| serde_json::from_value::<[f64; 2]>(value.clone()).ok());
    clamp_world_scale_to_bounds(world_scale, bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_rejects_non_positive_and_non_finite() {
        assert_eq!(sanitize_world_scale(0.5), 0.5);
        assert_eq!(sanitize_world_scale(0.0), 1.0);
        assert_eq!(sanitize_world_scale(-2.0), 1.0);
        assert_eq!(sanitize_world_scale(f64::NAN), 1.0);
        assert_eq!(sanitize_world_scale(f64::INFINITY), 1.0);
        // Finite f64 values that overflow or vanish after the `as f32` placement cast.
        assert_eq!(sanitize_world_scale(1e300), 1.0);
        assert_eq!(sanitize_world_scale(1e-300), 1.0);
        assert_eq!(sanitize_world_scale(f64::MAX), 1.0);
        assert_eq!(sanitize_world_scale(MIN_WORLD_SCALE), MIN_WORLD_SCALE);
        assert_eq!(sanitize_world_scale(MAX_WORLD_SCALE), MAX_WORLD_SCALE);
        assert!(!is_world_scale_in_range(1e300));
        assert!(is_world_scale_in_range(0.001));
    }

    #[test]
    fn clamp_vectors_match_terrain_parity() {
        assert_eq!(clamp_world_scale_to_bounds(0.5, Some([0.001, 0.1])), 0.1);
        assert_eq!(clamp_world_scale_to_bounds(0.00001, Some([0.001, 0.1])), 0.001);
        assert_eq!(clamp_world_scale_to_bounds(0.05, Some([0.001, 0.1])), 0.05);
        assert_eq!(clamp_world_scale_to_bounds(0.5, None), 0.5);
        assert_eq!(clamp_world_scale_to_bounds(-1.0, None), 1.0);
        assert_eq!(clamp_world_scale_to_bounds(0.5, Some([0.5, 0.1])), 0.5);
        assert_eq!(clamp_world_scale_to_bounds(1e300, Some([0.001, 0.1])), 0.1);
        assert_eq!(clamp_world_scale_to_bounds(f64::MAX, None), 1.0);
        assert_eq!(clamp_world_scale_to_bounds(1e-300, None), 1.0);
        // Bounds narrow the global range, never widen it.
        assert_eq!(clamp_world_scale_to_bounds(0.5, Some([1e-300, 1e300])), 0.5);
        assert_eq!(clamp_world_scale_to_bounds(5_000.0, Some([20_000.0, 1e9])), 5_000.0);
    }

    #[test]
    fn accessor_table() {
        let cases: Vec<(Option<Value>, f64)> = vec![
            (None, 1.0),
            (Some(Value::Null), 1.0),
            (Some(json!({})), 1.0),
            (Some(json!({"world_scale": 0})), 1.0),
            (Some(json!({"world_scale": -2})), 1.0),
            (Some(json!({"world_scale": "x"})), 1.0),
            (Some(json!({"world_scale": 0.01})), 0.01),
            (Some(json!({"world_scale": 1e300})), 1.0),
            (Some(json!({"world_scale": 1e-300})), 1.0),
            (Some(json!({"world_scale": f64::MAX})), 1.0),
            (Some(json!({"enabled": false, "world_scale": 0.01})), 0.01),
            (
                Some(json!({"world_scale": 0.5, "scale_bounds": [0.001, 0.1]})),
                0.1,
            ),
            (
                Some(json!({"world_scale": 0.5, "scale_bounds": [0.5, 0.1]})),
                0.5,
            ),
            (
                Some(json!({"world_scale": 0.5, "scale_bounds": "bad"})),
                0.5,
            ),
        ];
        for (terrain, expected) in cases {
            assert_eq!(world_scale_from_terrain_json(terrain.as_ref()), expected, "{terrain:?}");
        }
    }

    #[test]
    fn meters_world_round_trip() {
        let world = meters_to_world(1000.0, 0.01);
        assert!((world - 10.0).abs() < 1e-9);
        assert!((world_to_meters(world, 0.01) - 1000.0).abs() < 1e-9);
        assert_eq!(world_to_meters(5.0, 0.0), 5.0);
    }
}
