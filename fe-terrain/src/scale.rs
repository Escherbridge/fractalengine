//! Pure world-scale math for multi-scale terrain rendering; see `src/AGENTS.md` §scale.

/// Sanitize a world scale (world units per real meter): finite and `> 0`, else `1.0`.
pub fn sanitize_world_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// World-space edge length of a tile: real meters × scale.
pub fn scaled_tile_size(tile_world_size_m: f64, scale: f64) -> f64 {
    tile_world_size_m * scale
}

/// Convert a world-space height back to real meters (inverse of scale) for zoom selection.
pub fn world_to_real_height(world_y: f64, scale: f64) -> f64 {
    world_y / sanitize_world_scale(scale)
}

/// Scale a local coordinate / anchor by the world scale, component-wise.
pub fn scale_local(coord: [f64; 3], scale: f64) -> [f64; 3] {
    [coord[0] * scale, coord[1] * scale, coord[2] * scale]
}

/// Scale a grid of elevation samples (absolute meters) into world units.
pub fn scale_elevations(elevations: &[f32], scale: f32) -> Vec<f32> {
    elevations.iter().map(|e| e * scale).collect()
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
    fn scaled_tile_size_is_linear_in_scale() {
        let real = 156_000.0; // ~zoom-8 tile at the equator
        assert_eq!(scaled_tile_size(real, 1.0), real);
        assert!((scaled_tile_size(real, 0.001) - 156.0).abs() < 1e-9);
    }

    #[test]
    fn world_to_real_height_inverts_scale() {
        // A camera 200 world units up at 0.001 scale sees 200 km of real height.
        assert!((world_to_real_height(200.0, 0.001) - 200_000.0).abs() < 1e-6);
        // At 1:1 the world height is the real height.
        assert_eq!(world_to_real_height(200.0, 1.0), 200.0);
        // Bad scale falls back to 1.0 (no divide-by-zero).
        assert_eq!(world_to_real_height(200.0, 0.0), 200.0);
    }

    #[test]
    fn scale_local_scales_each_component() {
        assert_eq!(scale_local([10.0, -100.0, 30.0], 0.001), [0.01, -0.1, 0.03]);
        assert_eq!(scale_local([1.0, 2.0, 3.0], 1.0), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn scale_elevations_scales_each_sample() {
        let out = scale_elevations(&[0.0, 100.0, 150.0], 0.001);
        assert_eq!(out, vec![0.0, 0.1, 0.15]);
    }

    #[test]
    fn composed_world_y_matches_ele_minus_origin_times_scale() {
        // spawn_chunk composes anchor.y (= -origin_ele scaled) with mesh y (= ele scaled).
        let scale = 0.001;
        let origin_ele = 100.0_f64;
        let ele = 150.0_f32;
        let anchor = scale_local([0.0, -origin_ele, 0.0], scale);
        let mesh_y = scale_elevations(&[ele], scale as f32)[0] as f64;
        let world_y = anchor[1] + mesh_y;
        // f32 mesh heights (matching terrain_mesh) carry ~1e-7 relative error.
        assert!((world_y - (ele as f64 - origin_ele) * scale).abs() < 1e-5);
    }
}
