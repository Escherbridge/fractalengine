//! Terrain helpers over the `terrain` import (manifest `terrain: true`); see tools/fe-ext/AGENTS.md §guest-sdk.
//! A component imports `terrain` only when something in it calls these functions.

use crate::wit_terrain::{self, TerrainSample};
use crate::HostError;

/// Most points the host answers in one `sample-many` call.
pub const MAX_BATCH: usize = 1024;

/// Why a terrain read did not complete.
#[derive(Debug, Clone)]
pub enum TerrainError {
    /// The host refused the call (`not-available` when the bound petal's terrain is not loaded on this host).
    Host(HostError),
    /// Resident terrain changed between two chunks of one [`sample_many`]; sample again.
    Changed { first: u64, later: u64 },
}

impl From<HostError> for TerrainError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// Every point answered from one terrain generation.
#[derive(Debug, Clone)]
pub struct Samples {
    pub generation: u64,
    /// One entry per point, in order; `None` where no resident terrain covers it.
    pub samples: Vec<Option<TerrainSample>>,
}

/// The surface under `(x, z)` of the bound petal; `None` where no resident terrain covers it.
pub fn sample(x: f64, z: f64) -> Result<Option<TerrainSample>, HostError> {
    wit_terrain::sample(x, z)
}

/// Sample any number of points in chunks of [`MAX_BATCH`]; refuses a result that mixes terrain generations.
pub fn sample_many(points: &[(f64, f64)]) -> Result<Samples, TerrainError> {
    let mut answered = Samples { generation: 0, samples: Vec::with_capacity(points.len()) };
    for (index, chunk) in points.chunks(MAX_BATCH).enumerate() {
        let batch = wit_terrain::sample_many(chunk)?;
        if index == 0 {
            answered.generation = batch.generation;
        } else if batch.generation != answered.generation {
            return Err(TerrainError::Changed { first: answered.generation, later: batch.generation });
        }
        answered.samples.extend(batch.samples);
    }
    Ok(answered)
}

/// Height range and steepest slope over a footprint's samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FootprintStats {
    /// Lowest surface (world y); `None` when no point was covered.
    pub min_y: Option<f64>,
    pub max_y: Option<f64>,
    pub max_slope_pct: Option<f64>,
    /// Points with a sample.
    pub covered: usize,
}

/// Pure summary of [`sample_many`] output for site checks (level enough, fully on terrain).
pub fn footprint_stats(samples: &[Option<TerrainSample>]) -> FootprintStats {
    let mut stats = FootprintStats { min_y: None, max_y: None, max_slope_pct: None, covered: 0 };
    for sample in samples.iter().flatten() {
        stats.covered += 1;
        stats.min_y = Some(stats.min_y.map_or(sample.surface_y, |low: f64| low.min(sample.surface_y)));
        stats.max_y = Some(stats.max_y.map_or(sample.surface_y, |high: f64| high.max(sample.surface_y)));
        stats.max_slope_pct = Some(stats.max_slope_pct.map_or(sample.slope_pct, |steep: f64| steep.max(sample.slope_pct)));
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    fn at(surface_y: f64, slope_pct: f64) -> Option<TerrainSample> {
        Some(TerrainSample { surface_y, elevation_m: surface_y, slope_pct, normal: Vec3 { x: 0.0, y: 1.0, z: 0.0 } })
    }

    #[test]
    fn footprint_stats_table() {
        let empty = FootprintStats { min_y: None, max_y: None, max_slope_pct: None, covered: 0 };
        let rows: [(&[Option<TerrainSample>], FootprintStats); 3] = [
            (&[], empty),
            (&[None, None], empty),
            (
                &[at(2.0, 5.0), None, at(-1.0, 12.5), at(4.0, 0.0)],
                FootprintStats { min_y: Some(-1.0), max_y: Some(4.0), max_slope_pct: Some(12.5), covered: 3 },
            ),
        ];
        for (samples, expected) in rows {
            assert_eq!(footprint_stats(samples), expected);
        }
    }
}
