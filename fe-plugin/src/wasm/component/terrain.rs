//! Host side of the `terrain` import: read-only surface samples of the bound petal from a host-published
//! terrain snapshot; see src/wasm/AGENTS.md §terrain.

use std::sync::atomic::Ordering;
use std::time::Instant;

use super::bindings::fractalengine::plugin::terrain;
use super::bindings::fractalengine::plugin::types::HostError;
use super::imports::vec3;
use super::state::{denied, invalid, InstanceState};

/// Most points one `sample-many` call answers.
pub const MAX_TERRAIN_BATCH: usize = 1024;
/// Most points one hook call may sample across all `terrain` calls.
pub const MAX_TERRAIN_SAMPLES_PER_HOOK: u32 = 16_384;
/// Central-difference half-step for slope and normal, in real metres (multiplied by the world scale).
pub const SLOPE_STEP_M: f64 = 1.0;
/// Generation a fixed [`GridTerrain`] reports (it never changes).
pub const GRID_TERRAIN_GENERATION: u64 = 1;

/// One surface sample in host units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainPoint {
    /// World-frame surface height.
    pub surface_y: f64,
    /// Real elevation in metres above the terrain datum.
    pub elevation_m: f64,
    /// Steepest rise over run, percent.
    pub slope_pct: f64,
    /// Unit surface normal, +y up.
    pub normal: [f64; 3],
}

/// Host seam: the binary implements it over its published snapshot, the sandbox and tests over a fixture grid.
pub trait TerrainSource: Send + Sync {
    /// Clear `out` and fill it with one entry per point for (verse, petal); returns the terrain generation.
    /// `Err(reason)` means that petal's terrain is not resident on this host (`not-available`).
    fn sample(
        &self,
        verse_id: &str,
        petal_id: &str,
        points: &[(f64, f64)],
        out: &mut Vec<Option<TerrainPoint>>,
    ) -> Result<u64, String>;
}

/// Surface, slope and normal from a world-unit height function by central differences, one-sided where a
/// neighbour is uncovered; `step` is in world units. `None` when the centre is uncovered or any input is not finite.
pub fn surface_at(
    height: impl Fn(f64, f64) -> Option<f64>,
    x: f64,
    z: f64,
    step: f64,
    world_scale: f64,
    origin_ele: f64,
) -> Option<TerrainPoint> {
    let usable = |value: f64| value.is_finite() && value > 0.0;
    if !x.is_finite() || !z.is_finite() || !usable(step) || !usable(world_scale) || !origin_ele.is_finite() {
        return None;
    }
    let finite = |value: Option<f64>| value.filter(|height| height.is_finite());
    let center = finite(height(x, z))?;
    let gradient = |minus: Option<f64>, plus: Option<f64>| match (minus, plus) {
        (Some(low), Some(high)) => (high - low) / (2.0 * step),
        (None, Some(high)) => (high - center) / step,
        (Some(low), None) => (center - low) / step,
        (None, None) => 0.0,
    };
    let along_x = gradient(finite(height(x - step, z)), finite(height(x + step, z)));
    let along_z = gradient(finite(height(x, z - step)), finite(height(x, z + step)));
    let length = (along_x * along_x + 1.0 + along_z * along_z).sqrt();
    Some(TerrainPoint {
        surface_y: center,
        elevation_m: center / world_scale + origin_ele,
        slope_pct: 100.0 * along_x.hypot(along_z),
        normal: [-along_x / length, 1.0 / length, -along_z / length],
    })
}

/// Fixture and test grid: row-major world-unit heights (row = +z, column = +x), bilinear like fe-renderer
/// `HeightTile::sample`. It serves any verse and petal.
#[derive(Debug, Clone, PartialEq)]
pub struct GridTerrain {
    /// World `(x, z)` of the first sample.
    pub origin: [f64; 2],
    /// Spacing between samples, world units.
    pub cell: f64,
    /// Samples along +x.
    pub columns: u32,
    /// Samples along +z.
    pub rows: u32,
    /// `columns * rows` world-unit surface heights.
    pub heights: Vec<f64>,
    /// World units per real metre.
    pub world_scale: f64,
    /// Real elevation (metres) at world height 0.
    pub origin_ele: f64,
}

impl GridTerrain {
    /// Bilinear height at world `(x, z)`; `None` outside the grid or for a malformed grid.
    pub fn height(&self, x: f64, z: f64) -> Option<f64> {
        let (columns, rows) = (self.columns as usize, self.rows as usize);
        if columns < 2 || rows < 2 || self.heights.len() != columns * rows || !(self.cell.is_finite() && self.cell > 0.0) {
            return None;
        }
        let u = (x - self.origin[0]) / self.cell;
        let v = (z - self.origin[1]) / self.cell;
        if !(0.0..=(columns - 1) as f64).contains(&u) || !(0.0..=(rows - 1) as f64).contains(&v) {
            return None;
        }
        let column = (u.floor() as usize).min(columns - 2);
        let row = (v.floor() as usize).min(rows - 2);
        let (fu, fv) = (u - column as f64, v - row as f64);
        let at = |row: usize, column: usize| self.heights[row * columns + column];
        Some(
            at(row, column) * (1.0 - fu) * (1.0 - fv)
                + at(row, column + 1) * fu * (1.0 - fv)
                + at(row + 1, column) * (1.0 - fu) * fv
                + at(row + 1, column + 1) * fu * fv,
        )
    }
}

impl TerrainSource for GridTerrain {
    fn sample(
        &self,
        _verse_id: &str,
        _petal_id: &str,
        points: &[(f64, f64)],
        out: &mut Vec<Option<TerrainPoint>>,
    ) -> Result<u64, String> {
        let step = SLOPE_STEP_M * self.world_scale;
        out.clear();
        out.extend(points.iter().map(|&(x, z)| {
            surface_at(|x, z| self.height(x, z), x, z, step, self.world_scale, self.origin_ele)
        }));
        Ok(GRID_TERRAIN_GENERATION)
    }
}

fn wit_sample(point: TerrainPoint) -> terrain::TerrainSample {
    terrain::TerrainSample {
        surface_y: point.surface_y,
        elevation_m: point.elevation_m,
        slope_pct: point.slope_pct,
        normal: vec3(point.normal),
    }
}

impl InstanceState {
    /// Grant, input, deadline and per-hook budget checks, then one source call into the reused scratch buffer.
    fn sample_terrain(&mut self, points: &[(f64, f64)]) -> Result<u64, HostError> {
        self.lease_active()?;
        if !self.grants().is_some_and(|grants| grants.terrain) {
            return Err(denied("terrain"));
        }
        if points.len() > MAX_TERRAIN_BATCH {
            return Err(invalid(format!("at most {MAX_TERRAIN_BATCH} points per terrain call")));
        }
        if points.iter().any(|(x, z)| !x.is_finite() || !z.is_finite()) {
            return Err(invalid("terrain points must be finite"));
        }
        if self.hook_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(HostError::BudgetExceeded("the hook deadline passed; no further terrain samples".into()));
        }
        let requested = points.len() as u32;
        let used = self.terrain_samples_this_hook.load(Ordering::Relaxed);
        if used.saturating_add(requested) > MAX_TERRAIN_SAMPLES_PER_HOOK {
            return Err(HostError::BudgetExceeded(format!(
                "at most {MAX_TERRAIN_SAMPLES_PER_HOOK} terrain samples per hook call"
            )));
        }
        let source = self
            .terrain
            .clone()
            .ok_or_else(|| HostError::NotAvailable("this host provides no terrain".into()))?;
        let generation = source
            .sample(&self.binding.verse_id, &self.binding.petal_id, points, &mut self.terrain_scratch)
            .map_err(HostError::NotAvailable)?;
        // A refused call costs no budget.
        self.terrain_samples_this_hook.store(used + requested, Ordering::Relaxed);
        Ok(generation)
    }
}

impl terrain::Host for InstanceState {
    fn sample(&mut self, x: f64, z: f64) -> Result<Option<terrain::TerrainSample>, HostError> {
        self.sample_terrain(&[(x, z)])?;
        Ok(self.terrain_scratch.first().copied().flatten().map(wit_sample))
    }

    fn sample_many(&mut self, points: Vec<(f64, f64)>) -> Result<terrain::TerrainBatch, HostError> {
        let generation = self.sample_terrain(&points)?;
        Ok(terrain::TerrainBatch {
            generation,
            samples: self.terrain_scratch.iter().map(|point| point.map(wit_sample)).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 11 x 11 grid, 10 m cells from (0, 0): rises 1 m per 10 m east (10 %) and 2 m per 10 m north (20 %).
    fn ramp() -> GridTerrain {
        let heights = (0..11).flat_map(|row| (0..11).map(move |column| f64::from(column) + 2.0 * f64::from(row))).collect();
        GridTerrain { origin: [0.0, 0.0], cell: 10.0, columns: 11, rows: 11, heights, world_scale: 1.0, origin_ele: 100.0 }
    }

    #[test]
    fn surface_at_reads_height_slope_and_normal_from_a_grid() {
        let ramp_grid = ramp();
        let flat = GridTerrain { heights: vec![3.0; 121], ..ramp() };
        let east_only = GridTerrain { heights: (0..121).map(|index| f64::from(index % 11)).collect(), ..ramp() };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // (grid, x, z, expected (surface_y, elevation_m, slope_pct, normal x, normal z)), None = no sample.
        let rows: [(&GridTerrain, f64, f64, Option<(f64, f64, f64, f64, f64)>); 7] = [
            (&flat, 50.0, 50.0, Some((3.0, 103.0, 0.0, 0.0, 0.0))),
            (&east_only, 50.0, 50.0, Some((5.0, 105.0, 10.0, -0.1 / 1.01f64.sqrt(), 0.0))),
            (&ramp_grid, 50.0, 50.0, Some((15.0, 115.0, 100.0 * 0.05f64.sqrt(), -0.1 / 1.05f64.sqrt(), -0.2 / 1.05f64.sqrt()))),
            // The east edge: the +x neighbour is uncovered, so the slope is one-sided and still 10 %.
            (&east_only, 100.0, 50.0, Some((10.0, 110.0, 10.0, -0.1 / 1.01f64.sqrt(), 0.0))),
            (&flat, 100.5, 50.0, None),
            (&flat, -0.5, 50.0, None),
            (&flat, f64::NAN, 50.0, None),
        ];
        for (row, (grid, x, z, expected)) in rows.into_iter().enumerate() {
            let mut out = Vec::new();
            assert_eq!(grid.sample("v", "p", &[(x, z)], &mut out), Ok(GRID_TERRAIN_GENERATION));
            let got = out[0].map(|point| (point.surface_y, point.elevation_m, point.slope_pct, point.normal[0], point.normal[2]));
            match (got, expected) {
                (None, None) => {}
                (Some(got), Some(want)) => assert!(
                    close(got.0, want.0) && close(got.1, want.1) && close(got.2, want.2) && close(got.3, want.3) && close(got.4, want.4),
                    "row {row}: {got:?} != {want:?}"
                ),
                other => panic!("row {row}: {other:?}"),
            }
        }
        let unit = surface_at(|x, z| ramp_grid.height(x, z), 50.0, 50.0, 1.0, 1.0, 0.0).unwrap().normal;
        assert!(close(unit.iter().map(|c| c * c).sum::<f64>(), 1.0), "the normal is unit length");
    }
}
