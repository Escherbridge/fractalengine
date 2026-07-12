//! Pure LOD ring, despawn-hysteresis, and close-range upsample math; see `src/AGENTS.md` §terrain_plugin.
//!
//! No `bevy` dependency — all helpers are unit-tested without the render feature.

use std::collections::HashSet;

use crate::tiles::TileCoord;

/// Multiplier from camera world-height to the visible ground radius (world units).
pub const VIEW_RADIUS_FACTOR: f64 = 3.0;
/// Despawn radius as a multiple of the spawn radius (strictly > 1 → no flicker).
pub const DESPAWN_HYSTERESIS: f64 = 1.5;
/// Cap on close-range elevation upsampling (power of two).
pub const MAX_UPSAMPLE: u32 = 4;
/// Height (real meters) at/above which the camera wants the served max zoom; mirrors `desired_zoom`.
pub const ZOOM_BASE_HEIGHT_M: f64 = 200.0;
/// Max child-zoom span for the hole-free wrong-zoom despawn coverage check.
pub const MAX_COVER_DZ: u8 = 2;

/// World-unit ground radius the camera can see: height × factor, capped by the far plane.
pub fn view_radius_world(cam_height_world: f64, far_world: f64, factor: f64) -> f64 {
    let h = cam_height_world.abs();
    (h * factor.max(0.0)).min(far_world.max(0.0))
}

/// Spawn/despawn world radii with hysteresis; floored to ~2 tiles so big low-zoom
/// tiles don't churn. Despawn is strictly greater than spawn.
pub fn spawn_despawn_radii(view_radius_world: f64, tile_world: f64, hysteresis: f64) -> (f64, f64) {
    let spawn = view_radius_world.max(2.0 * tile_world.max(0.0));
    let despawn = spawn * hysteresis.max(1.0 + f64::EPSILON);
    (spawn, despawn)
}

/// Ring radius in tiles covering `view_radius_world`, clamped to `[1, max_radius]`.
pub fn ring_radius_tiles(view_radius_world: f64, scaled_tile_size: f64, max_radius: u32) -> u32 {
    if !(scaled_tile_size > 0.0) {
        return 1;
    }
    let r = (view_radius_world / scaled_tile_size).ceil();
    let r = if r.is_finite() && r >= 1.0 { r as u32 } else { 1 };
    r.clamp(1, max_radius.max(1))
}

/// Largest ring radius whose `(2r+1)^2` tile count still fits the chunk budget (≥1).
pub fn max_ring_radius_for_budget(max_chunks: usize) -> u32 {
    let mut r = 1u32;
    loop {
        let next = r + 1;
        let count = ((2 * next + 1) as usize).pow(2);
        if count <= max_chunks {
            r = next;
        } else {
            return r;
        }
    }
}

/// Ring offset coordinates within `radius`, nearest-first (ascending squared distance).
pub fn ring_offsets(radius: u32) -> Vec<(i64, i64)> {
    let r = radius as i64;
    let mut v = Vec::with_capacity(((2 * radius + 1) as usize).pow(2));
    for dy in -r..=r {
        for dx in -r..=r {
            v.push((dx, dy));
        }
    }
    v.sort_by_key(|(dx, dy)| dx * dx + dy * dy);
    v
}

/// Extra zoom steps a close camera may over-fetch splats beyond the mesh zoom.
pub const SPLAT_CLOSE_ZOOM_BONUS: u8 = 2;

/// Splat-specific desired zoom: like `desired_zoom` but may exceed `mesh_zoom` by up to
/// [`SPLAT_CLOSE_ZOOM_BONUS`] when the camera is close, and never exceeds it when far.
/// Clamped into `[min_zoom, max_zoom]` (FR-5 graceful ceiling — no error past the tileset max).
pub fn splat_desired_zoom(cam_height_m: f32, mesh_zoom: u8, min_zoom: u8, max_zoom: u8) -> u8 {
    let (min_zoom, max_zoom) = if min_zoom <= max_zoom {
        (min_zoom, max_zoom)
    } else {
        (max_zoom, min_zoom)
    };
    let h = f64::from(cam_height_m.max(1.0));
    // Extra zoom-in steps as the camera descends below the base height (each halving → +1).
    let bonus = (ZOOM_BASE_HEIGHT_M / h).max(1.0).log2().floor() as i64;
    let bonus = bonus.clamp(0, SPLAT_CLOSE_ZOOM_BONUS as i64);
    let z = mesh_zoom as i64 + bonus;
    z.clamp(min_zoom as i64, max_zoom as i64) as u8
}

/// Tiles at `target_zoom` that fully cover `coord`'s footprint (self, children, or parent).
pub fn covering_tiles(coord: TileCoord, target_zoom: u8) -> Vec<(u8, u32, u32)> {
    use std::cmp::Ordering;
    match target_zoom.cmp(&coord.zoom) {
        Ordering::Equal => vec![(coord.zoom, coord.x, coord.y)],
        Ordering::Greater => {
            let dz = (target_zoom - coord.zoom) as u32;
            let f = 1u32 << dz;
            let x0 = coord.x.saturating_mul(f);
            let y0 = coord.y.saturating_mul(f);
            let mut out = Vec::with_capacity((f * f) as usize);
            for yy in 0..f {
                for xx in 0..f {
                    out.push((target_zoom, x0 + xx, y0 + yy));
                }
            }
            out
        }
        Ordering::Less => {
            let dz = (coord.zoom - target_zoom) as u32;
            vec![(target_zoom, coord.x >> dz, coord.y >> dz)]
        }
    }
}

/// True when `coord` is at the wrong zoom AND its desired-zoom replacement coverage
/// already exists — the condition for a hole-free despawn. A large child span
/// (`> MAX_COVER_DZ`) reports `false` so the old chunk lingers (brief overlap, no gap).
pub fn wrong_zoom_replacement_present(
    coord: TileCoord,
    desired_zoom: u8,
    existing: &HashSet<(u8, u32, u32)>,
) -> bool {
    if coord.zoom == desired_zoom {
        return false;
    }
    if desired_zoom > coord.zoom.saturating_add(MAX_COVER_DZ) {
        return false;
    }
    covering_tiles(coord, desired_zoom)
        .into_iter()
        .all(|t| existing.contains(&t))
}

/// Elevation upsample factor for how far below the max-zoom "natural height" the
/// camera has descended (each halving below `base_height_m` doubles density).
/// Returns a power of two ≤ `cap`; `1` when no upsampling is needed.
pub fn upsample_factor_for_height(cam_height_m: f64, base_height_m: f64, cap: u32) -> u32 {
    let h = cam_height_m.abs().max(1.0);
    let base = base_height_m.max(1.0);
    if h >= base || cap <= 1 {
        return 1;
    }
    let steps = (base / h).log2().floor().max(0.0) as u32;
    let f = 1u32 << steps.min(16);
    f.min(largest_pow2_le(cap))
}

/// Largest power of two ≤ `cap` (≥1).
fn largest_pow2_le(cap: u32) -> u32 {
    let mut p = 1u32;
    while p.saturating_mul(2) <= cap {
        p *= 2;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_radius_capped_by_far_plane() {
        assert_eq!(view_radius_world(100.0, 1000.0, 3.0), 300.0);
        // Far plane caps the reach.
        assert_eq!(view_radius_world(100.0, 200.0, 3.0), 200.0);
        // Negative height handled via abs.
        assert_eq!(view_radius_world(-100.0, 1000.0, 3.0), 300.0);
    }

    #[test]
    fn hysteresis_despawn_strictly_greater_than_spawn() {
        let (spawn, despawn) = spawn_despawn_radii(300.0, 50.0, DESPAWN_HYSTERESIS);
        assert_eq!(spawn, 300.0);
        assert!(despawn > spawn, "despawn {despawn} must exceed spawn {spawn}");
        // Two-tile floor applies when the view radius is tiny.
        let (spawn, despawn) = spawn_despawn_radii(10.0, 50.0, DESPAWN_HYSTERESIS);
        assert_eq!(spawn, 100.0);
        assert!(despawn > spawn);
    }

    #[test]
    fn ring_radius_grows_with_view_and_clamps() {
        // Tile size 100 world units.
        assert_eq!(ring_radius_tiles(50.0, 100.0, 8), 1); // < 1 tile → floor 1
        assert_eq!(ring_radius_tiles(250.0, 100.0, 8), 3); // ceil(2.5)
        assert_eq!(ring_radius_tiles(5000.0, 100.0, 7), 7); // clamped to budget max
        // Bad tile size falls back to 1.
        assert_eq!(ring_radius_tiles(500.0, 0.0, 8), 1);
    }

    #[test]
    fn budget_radius_fits_chunk_cap() {
        // (2*7+1)^2 = 225 <= 256, (2*8+1)^2 = 289 > 256.
        assert_eq!(max_ring_radius_for_budget(256), 7);
        assert_eq!(max_ring_radius_for_budget(9), 1);
        assert_eq!(max_ring_radius_for_budget(25), 2);
    }

    #[test]
    fn ring_offsets_are_nearest_first_and_complete() {
        let offs = ring_offsets(1);
        assert_eq!(offs.len(), 9);
        assert_eq!(offs[0], (0, 0)); // center first
        // First ring (dist 1) before corners (dist 2).
        let center_idx = offs.iter().position(|&o| o == (0, 0)).unwrap();
        let corner_idx = offs.iter().position(|&o| o == (1, 1)).unwrap();
        let edge_idx = offs.iter().position(|&o| o == (1, 0)).unwrap();
        assert!(center_idx < edge_idx && edge_idx < corner_idx);
    }

    #[test]
    fn splat_desired_zoom_monotonic_in_height() {
        // Lower camera (closer) must never request a lower zoom than a higher camera.
        let mut prev = 0u8;
        let mut h = 1.0f32;
        let mut heights = vec![];
        while h <= 100_000.0 {
            heights.push(h);
            h *= 2.0;
        }
        // Iterate from far (high) to near (low): zoom must be non-decreasing.
        for &hh in heights.iter().rev() {
            let z = splat_desired_zoom(hh, 12, 0, 20);
            assert!(z >= prev, "closer camera at {hh}m gave lower zoom {z} < {prev}");
            prev = z;
        }
    }

    #[test]
    fn splat_desired_zoom_clamped_to_range() {
        // Very close camera would exceed max_zoom → clamped (FR-5 ceiling).
        assert_eq!(splat_desired_zoom(1.0, 14, 0, 14), 14);
        // Far camera never exceeds mesh_zoom.
        assert_eq!(splat_desired_zoom(100_000.0, 10, 0, 20), 10);
        // Close camera adds the bonus, capped by SPLAT_CLOSE_ZOOM_BONUS.
        assert_eq!(splat_desired_zoom(1.0, 10, 0, 20), 10 + SPLAT_CLOSE_ZOOM_BONUS);
        // Result never drops below min_zoom.
        assert!(splat_desired_zoom(1.0, 2, 5, 20) >= 5);
        // Inverted min/max is normalized, not panicked.
        assert_eq!(splat_desired_zoom(100_000.0, 10, 20, 0), 10);
    }

    #[test]
    fn covering_tiles_self_children_parent() {
        let c = TileCoord::new(4, 6, 10);
        assert_eq!(covering_tiles(c, 10), vec![(10, 4, 6)]);
        // One level down → 4 children.
        let kids = covering_tiles(c, 11);
        assert_eq!(kids.len(), 4);
        assert!(kids.contains(&(11, 8, 12)));
        assert!(kids.contains(&(11, 9, 13)));
        // Parent.
        assert_eq!(covering_tiles(c, 9), vec![(9, 2, 3)]);
    }

    #[test]
    fn wrong_zoom_replacement_gate() {
        let mut existing = HashSet::new();
        let chunk = TileCoord::new(2, 3, 9); // coarse tile
        // Same zoom is never "wrong".
        assert!(!wrong_zoom_replacement_present(chunk, 9, &existing));
        // Desired one level finer → needs all 4 children present.
        assert!(!wrong_zoom_replacement_present(chunk, 10, &existing));
        for t in covering_tiles(chunk, 10) {
            existing.insert(t);
        }
        assert!(wrong_zoom_replacement_present(chunk, 10, &existing));
        // Excessive child span is not attempted (returns false → linger, no hole).
        assert!(!wrong_zoom_replacement_present(chunk, 9 + MAX_COVER_DZ + 1, &existing));
    }

    #[test]
    fn wrong_zoom_replacement_parent_case() {
        let mut existing = HashSet::new();
        let chunk = TileCoord::new(8, 12, 11); // fine tile
        assert!(!wrong_zoom_replacement_present(chunk, 9, &existing));
        existing.insert((9, 2, 3)); // the covering parent
        assert!(wrong_zoom_replacement_present(chunk, 9, &existing));
    }

    #[test]
    fn upsample_factor_only_when_closer_than_base() {
        // At or above the base height → no upsample.
        assert_eq!(upsample_factor_for_height(200.0, 200.0, 4), 1);
        assert_eq!(upsample_factor_for_height(500.0, 200.0, 4), 1);
        // Half the base → 2×; quarter → 4× (capped).
        assert_eq!(upsample_factor_for_height(100.0, 200.0, 4), 2);
        assert_eq!(upsample_factor_for_height(50.0, 200.0, 4), 4);
        assert_eq!(upsample_factor_for_height(10.0, 200.0, 4), 4); // cap holds
        // Cap of 1 disables upsampling.
        assert_eq!(upsample_factor_for_height(10.0, 200.0, 1), 1);
    }

    #[test]
    fn largest_pow2_le_bounds() {
        assert_eq!(largest_pow2_le(4), 4);
        assert_eq!(largest_pow2_le(5), 4);
        assert_eq!(largest_pow2_le(1), 1);
        assert_eq!(largest_pow2_le(0), 1);
    }
}
