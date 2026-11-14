//! Terrain-aware ground point (D-38): snap to terrain when present, Y=0 otherwise.

use bevy::prelude::*;
use fe_renderer::terrain_height::TerrainHeightField;

/// Maximum fixed-point parallax-correction iterations.
pub(crate) const GROUND_SNAP_ITERATIONS: usize = 4;

/// Height agreement (world units) at which the iteration is considered converged.
const CONVERGENCE_TOLERANCE: f32 = 1e-3;

/// Intersect `ray` with the horizontal plane `y = plane_y`; `None` if parallel, behind, or non-finite.
fn intersect_plane_y(ray: &Ray3d, plane_y: f32) -> Option<Vec3> {
    let direction_y = ray.direction.y;
    if direction_y.abs() <= f32::EPSILON {
        return None;
    }
    let distance = (plane_y - ray.origin.y) / direction_y;
    if !distance.is_finite() || distance < 0.0 {
        return None;
    }
    let hit = ray.origin + *ray.direction * distance;
    hit.is_finite().then_some(hit)
}

/// World point under `ray`: terrain surface when `height_field` covers it, else the `y = 0` plane.
pub(crate) fn ground_point(
    ray: &Ray3d,
    height_field: Option<&TerrainHeightField>,
) -> Option<[f32; 3]> {
    let Some(field) = height_field else {
        let hit = intersect_plane_y(ray, 0.0)?;
        return Some([hit.x, 0.0, hit.z]);
    };

    // Fixed-point parallax correction: the visible surface point is displaced along the ray,
    // so re-intersect at the sampled height until height and plane agree.
    let mut plane_y = 0.0_f32;
    let mut last_hit: Option<Vec3> = None;
    for iteration in 0..GROUND_SNAP_ITERATIONS {
        let Some(hit) = intersect_plane_y(ray, plane_y) else {
            // Terrain above the ray's reach: keep the last XZ on the sampled surface, not under it.
            return last_hit.map(|point| [point.x, plane_y, point.z]);
        };
        last_hit = Some(hit);
        let sampled = field.height_at(hit.x, hit.z).filter(|h| h.is_finite());
        let Some(height) = sampled else {
            // No terrain under the cursor: today's behaviour on the first pass.
            return Some(if iteration == 0 {
                [hit.x, 0.0, hit.z]
            } else {
                [hit.x, hit.y, hit.z]
            });
        };
        if (height - plane_y).abs() < CONVERGENCE_TOLERANCE {
            return Some([hit.x, height, hit.z]);
        }
        plane_y = height;
    }
    last_hit.map(|hit| [hit.x, plane_y, hit.z])
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_renderer::terrain_height::HeightTile;

    fn field_with(grid: Vec<f32>) -> TerrainHeightField {
        let mut field = TerrainHeightField::default();
        field.insert(
            (0, 0, 0),
            HeightTile {
                anchor: Vec3::ZERO,
                tile_size: 100.0,
                grid,
                width: 2,
                height: 2,
            },
        );
        field
    }

    fn ray(origin: Vec3, direction: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(direction).unwrap())
    }

    #[test]
    fn ground_point_table() {
        let flat = field_with(vec![5.0; 4]);
        // Ramp: height = 0.1 * z.
        let ramp = field_with(vec![0.0, 0.0, 10.0, 10.0]);
        let down = ray(Vec3::new(30.0, 50.0, 40.0), Vec3::NEG_Y);
        let oblique = ray(Vec3::new(50.0, 50.0, 0.0), Vec3::new(0.0, -1.0, 1.0));
        let outside = ray(Vec3::new(500.0, 50.0, 500.0), Vec3::NEG_Y);
        let parallel = ray(Vec3::new(0.0, 5.0, 0.0), Vec3::X);

        let flat_hit = ground_point(&down, Some(&flat)).unwrap();
        assert_eq!(flat_hit[0], 30.0);
        assert_eq!(flat_hit[2], 40.0);
        assert!((flat_hit[1] - 5.0).abs() < 1e-3);

        let ramp_hit = ground_point(&oblique, Some(&ramp)).unwrap();
        let surface = ramp.height_at(ramp_hit[0], ramp_hit[2]).unwrap();
        assert!((ramp_hit[1] - surface).abs() < 1e-2);

        let outside_hit = ground_point(&outside, Some(&flat)).unwrap();
        assert_eq!(outside_hit, [500.0, 0.0, 500.0]);

        // A ray starting below a raised plateau never reaches its height: stay on the surface, not under it.
        let plateau = field_with(vec![20.0; 4]);
        let low = ray(Vec3::new(0.0, 10.0, 10.0), Vec3::new(1.0, -1.0, 0.0));
        let low_hit = ground_point(&low, Some(&plateau)).unwrap();
        assert!((low_hit[0] - 10.0).abs() < 1e-3 && (low_hit[2] - 10.0).abs() < 1e-3);
        assert!((low_hit[1] - 20.0).abs() < 1e-3, "on the plateau surface, got {low_hit:?}");

        assert_eq!(ground_point(&down, None).unwrap(), [30.0, 0.0, 40.0]);
        assert!(ground_point(&parallel, Some(&flat)).is_none());
        assert!(ground_point(&parallel, None).is_none());
    }
}
