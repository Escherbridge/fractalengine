//! Terrain height field shared between fe-terrain (writer) and the camera
//! ground clamp (reader); see `src/AGENTS.md` §terrain-height.

use bevy::prelude::*;
use std::collections::HashMap;

/// One resident terrain tile's height grid (world units, same grid its mesh was built from).
pub struct HeightTile {
    /// World-space SW-corner anchor (the chunk entity's translation).
    pub anchor: Vec3,
    /// Tile edge length in world units (east = +x, north = +z).
    pub tile_size: f32,
    /// Row-major heights relative to `anchor.y`; row maps to +z, col to +x.
    pub grid: Vec<f32>,
    /// Grid columns.
    pub width: u32,
    /// Grid rows.
    pub height: u32,
}

impl HeightTile {
    /// Bilinear height sample (world units) at world `(x, z)`; `None` outside the tile
    /// or for degenerate grids.
    pub fn sample(&self, x: f32, z: f32) -> Option<f32> {
        let tile_ok = self.tile_size.is_finite() && self.tile_size > 0.0;
        if self.width < 2 || self.height < 2 || !tile_ok {
            return None;
        }
        if self.grid.len() != (self.width as usize) * (self.height as usize) {
            return None;
        }
        let lx = x - self.anchor.x;
        let lz = z - self.anchor.z;
        if lx < 0.0 || lz < 0.0 || lx > self.tile_size || lz > self.tile_size {
            return None;
        }
        let w = self.width as usize;
        let u = (lx / self.tile_size) * (self.width - 1) as f32;
        let v = (lz / self.tile_size) * (self.height - 1) as f32;
        let c0 = (u.floor() as usize).min(w - 2);
        let r0 = (v.floor() as usize).min(self.height as usize - 2);
        let fu = (u - c0 as f32).clamp(0.0, 1.0);
        let fv = (v - r0 as f32).clamp(0.0, 1.0);
        let h00 = self.grid[r0 * w + c0];
        let h10 = self.grid[r0 * w + c0 + 1];
        let h01 = self.grid[(r0 + 1) * w + c0];
        let h11 = self.grid[(r0 + 1) * w + c0 + 1];
        let h = h00 * (1.0 - fu) * (1.0 - fv)
            + h10 * fu * (1.0 - fv)
            + h01 * (1.0 - fu) * fv
            + h11 * fu * fv;
        Some(self.anchor.y + h)
    }
}

/// Resident terrain heights keyed by tile `(zoom, x, y)`, kept in lockstep with
/// `TerrainChunk` spawns/despawns by fe-terrain; see `src/AGENTS.md` §terrain-height.
#[derive(Resource, Default)]
pub struct TerrainHeightField {
    tiles: HashMap<(u8, u32, u32), HeightTile>,
}

impl TerrainHeightField {
    /// Insert (or replace) the height tile for `key`.
    pub fn insert(&mut self, key: (u8, u32, u32), tile: HeightTile) {
        self.tiles.insert(key, tile);
    }

    /// Remove the height tile for `key` (no-op when absent).
    pub fn remove(&mut self, key: &(u8, u32, u32)) {
        self.tiles.remove(key);
    }

    /// Drop every resident tile (petal switch / assignment change).
    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    /// Number of resident tiles.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// True when no tiles are resident.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Terrain height (world units) under `(x, z)` from the highest-zoom covering tile;
    /// bounded scan over resident tiles (≤ terrain `max_chunks`), no allocation.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let mut best: Option<(u8, f32)> = None;
        for (key, tile) in &self.tiles {
            if best.is_some_and(|(zoom, _)| key.0 <= zoom) {
                continue;
            }
            if let Some(h) = tile.sample(x, z) {
                best = Some((key.0, h));
            }
        }
        best.map(|(_, h)| h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_tile(anchor: Vec3, tile_size: f32, height: f32) -> HeightTile {
        HeightTile {
            anchor,
            tile_size,
            grid: vec![height; 4],
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn sample_inside_flat_tile_returns_anchor_plus_height() {
        let t = flat_tile(Vec3::new(10.0, 5.0, 20.0), 100.0, 3.0);
        let h = t.sample(60.0, 70.0).unwrap();
        assert!((h - 8.0).abs() < 1e-4);
    }

    #[test]
    fn sample_outside_tile_is_none() {
        let t = flat_tile(Vec3::ZERO, 100.0, 0.0);
        assert!(t.sample(-0.1, 50.0).is_none());
        assert!(t.sample(50.0, 100.1).is_none());
    }

    #[test]
    fn sample_bilinear_interpolates_slope() {
        // 2x2 grid: east edge 10 units higher; midpoint interpolates to 5.
        let t = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 10.0,
            grid: vec![0.0, 10.0, 0.0, 10.0],
            width: 2,
            height: 2,
        };
        let h = t.sample(5.0, 5.0).unwrap();
        assert!((h - 5.0).abs() < 1e-4);
        // Corners are exact.
        assert!((t.sample(0.0, 0.0).unwrap() - 0.0).abs() < 1e-4);
        assert!((t.sample(10.0, 0.0).unwrap() - 10.0).abs() < 1e-4);
    }

    #[test]
    fn sample_row_maps_to_positive_z() {
        // Row 1 (north, +z) is 8 units higher than row 0.
        let t = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 10.0,
            grid: vec![0.0, 0.0, 8.0, 8.0],
            width: 2,
            height: 2,
        };
        assert!((t.sample(5.0, 0.0).unwrap() - 0.0).abs() < 1e-4);
        assert!((t.sample(5.0, 10.0).unwrap() - 8.0).abs() < 1e-4);
    }

    #[test]
    fn degenerate_grids_are_none() {
        let t = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 10.0,
            grid: vec![0.0, 0.0],
            width: 2,
            height: 1,
        };
        assert!(t.sample(5.0, 5.0).is_none());
        let mismatched = HeightTile {
            anchor: Vec3::ZERO,
            tile_size: 10.0,
            grid: vec![0.0; 3],
            width: 2,
            height: 2,
        };
        assert!(mismatched.sample(5.0, 5.0).is_none());
    }

    #[test]
    fn height_at_prefers_highest_zoom_cover() {
        let mut field = TerrainHeightField::default();
        field.insert((5, 0, 0), flat_tile(Vec3::ZERO, 100.0, 1.0));
        field.insert((6, 0, 0), flat_tile(Vec3::ZERO, 50.0, 2.0));
        // Point covered by both: zoom 6 wins.
        assert!((field.height_at(25.0, 25.0).unwrap() - 2.0).abs() < 1e-4);
        // Point covered only by the zoom-5 tile.
        assert!((field.height_at(75.0, 75.0).unwrap() - 1.0).abs() < 1e-4);
        // Point covered by neither.
        assert!(field.height_at(500.0, 500.0).is_none());
    }

    #[test]
    fn remove_and_clear_evict_tiles() {
        let mut field = TerrainHeightField::default();
        field.insert((5, 0, 0), flat_tile(Vec3::ZERO, 100.0, 1.0));
        field.insert((5, 1, 0), flat_tile(Vec3::new(100.0, 0.0, 0.0), 100.0, 1.0));
        field.remove(&(5, 0, 0));
        assert!(field.height_at(50.0, 50.0).is_none());
        assert_eq!(field.len(), 1);
        field.clear();
        assert!(field.is_empty());
    }
}
