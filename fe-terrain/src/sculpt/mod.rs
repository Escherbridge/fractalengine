//! Sculpt & earthwork region engine (sculpt_earthwork_regions_20260725, D-A8):
//! footprint area-selection (brush + defined shapes), sculpt operations, the
//! addressable modification-region record, and heightfield cut/fill volume in
//! real units. Bevy-free + always compiled (like `terrain_proposal.rs`/`ruler`/
//! `scale`) so the geometry/volume math is headless-testable and the fe-ui side
//! can mirror the record via JSON (NO fe-ui→fe-terrain dep). NEVER writes true
//! terrain: every fn takes the base surface as a read-only `&impl Fn(f32,f32)->
//! Option<f32>` (NFR-1, same contract as proposals). See `src/AGENTS.md` §sculpt.

use serde::{Deserialize, Serialize};

use crate::terrain_proposal::{footprint_bounds, point_in_polygon};

/// A sculpt operation over a region. Raise/Lower shift the existing surface by a
/// delta; Level flattens to an absolute target; Smooth relaxes toward the
/// region's mean height (planning-grade). Serde snake_case is the region JSON
/// contract; `from_snake` also accepts the terrain-proposal vocabulary
/// (`flatten` == `level`, `fill`/`pad` → raise, `cut` → lower) so a region
/// round-trips through the evolved `terrain.proposals` block without a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SculptOp {
    Raise,
    Lower,
    Level,
    Smooth,
}

impl SculptOp {
    /// Snake-case tag (region JSON `op` field + overlay tint parity).
    pub fn as_snake(&self) -> &'static str {
        match self {
            SculptOp::Raise => "raise",
            SculptOp::Lower => "lower",
            SculptOp::Level => "level",
            SculptOp::Smooth => "smooth",
        }
    }

    /// Lenient parse: the sculpt vocabulary plus the terrain-proposal op tags
    /// (so an evolved proposal record maps cleanly). Unknown → `None`.
    pub fn from_snake(s: &str) -> Option<Self> {
        match s {
            "raise" => Some(SculptOp::Raise),
            "lower" => Some(SculptOp::Lower),
            "level" | "flatten" => Some(SculptOp::Level),
            "smooth" => Some(SculptOp::Smooth),
            // Additive terrain ops fold onto their nearest sculpt intent so a
            // legacy proposal is still a well-formed region for volume math.
            "fill" | "pad" => Some(SculptOp::Raise),
            "cut" => Some(SculptOp::Lower),
            "ramp" | "slope" => Some(SculptOp::Level),
            _ => None,
        }
    }

    /// True when this op needs the read-only base sample to produce a height
    /// (Raise/Lower/Smooth are relative; Level is absolute).
    pub fn is_relative(&self) -> bool {
        !matches!(self, SculptOp::Level)
    }
}

/// An area-selection footprint in petal-local scene units (N-1: raw geometry;
/// `world_scale` enters only at the real-unit volume/report seam). `Brush` and
/// `Circle` are the same disc — kept distinct so the UI can label the tactile
/// (drag-painted) vs the defined (reportable) origin (D-A8 "an actual shape you
/// can report on"). All resolve to a closed XZ polygon via [`Footprint::to_polygon`].
#[derive(Debug, Clone, PartialEq)]
pub enum Footprint {
    /// Freeform brush dab: a disc of `radius` about `center` (tactile).
    Brush { center: [f32; 2], radius: f32 },
    /// Defined circle: a disc of `radius` about `center` (reportable).
    Circle { center: [f32; 2], radius: f32 },
    /// Axis-aligned rectangle between opposite corners `min`/`max`.
    Rect { min: [f32; 2], max: [f32; 2] },
    /// Arbitrary closed polygon (need not be explicitly closed).
    Polygon { verts: Vec<[f32; 2]> },
}

/// Segments used when a disc footprint is tessellated to a polygon.
const DISC_SEGMENTS: usize = 32;

impl Footprint {
    /// Closed XZ polygon `[x, z]` for this footprint (discs tessellated with
    /// [`DISC_SEGMENTS`]). Rect emits its 4 corners CCW. The result is what
    /// bakes, reports, and overlays — one shape the whole pipeline agrees on.
    pub fn to_polygon(&self) -> Vec<[f32; 2]> {
        match self {
            Footprint::Brush { center, radius } | Footprint::Circle { center, radius } => {
                disc_polygon(*center, *radius, DISC_SEGMENTS)
            }
            Footprint::Rect { min, max } => vec![
                [min[0], min[1]],
                [max[0], min[1]],
                [max[0], max[1]],
                [min[0], max[1]],
            ],
            Footprint::Polygon { verts } => verts.clone(),
        }
    }

    /// Whether world point `(x, z)` lies inside the footprint. Discs use an exact
    /// radial test (not the tessellated polygon) so brush gating is crisp.
    pub fn contains(&self, x: f32, z: f32) -> bool {
        match self {
            Footprint::Brush { center, radius } | Footprint::Circle { center, radius } => {
                let dx = x - center[0];
                let dz = z - center[1];
                dx * dx + dz * dz <= radius * radius
            }
            Footprint::Rect { min, max } => {
                let (lo_x, hi_x) = (min[0].min(max[0]), min[0].max(max[0]));
                let (lo_z, hi_z) = (min[1].min(max[1]), min[1].max(max[1]));
                x >= lo_x && x <= hi_x && z >= lo_z && z <= hi_z
            }
            Footprint::Polygon { verts } => point_in_polygon([x, z], verts),
        }
    }

    /// Axis-aligned bounds `(min_x, min_z, max_x, max_z)`; `None` when degenerate.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        match self {
            Footprint::Brush { center, radius } | Footprint::Circle { center, radius } => {
                let r = radius.abs();
                Some((center[0] - r, center[1] - r, center[0] + r, center[1] + r))
            }
            Footprint::Rect { min, max } => Some((
                min[0].min(max[0]),
                min[1].min(max[1]),
                min[0].max(max[0]),
                min[1].max(max[1]),
            )),
            Footprint::Polygon { verts } => footprint_bounds(verts),
        }
    }
}

/// A closed CCW polygon approximating a disc of `radius` about `center` (XZ).
fn disc_polygon(center: [f32; 2], radius: f32, segments: usize) -> Vec<[f32; 2]> {
    let n = segments.max(3);
    (0..n)
        .map(|i| {
            let t = (i as f32 / n as f32) * std::f32::consts::TAU;
            [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
        })
        .collect()
}

/// Mean of the read-only base surface over the footprint interior, sampled on a
/// regular `grid_step` lattice across the bounds; `None` when no inside sample
/// has a base height. Feeds Smooth (relax toward the mean) and analytic tests.
pub fn region_mean_height(
    base: &impl Fn(f32, f32) -> Option<f32>,
    footprint: &Footprint,
    grid_step: f32,
) -> Option<f32> {
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for_each_cell(footprint, grid_step, |x, z| {
        if let Some(h) = base(x, z) {
            sum += h as f64;
            count += 1;
        }
    });
    (count > 0).then(|| (sum / count as f64) as f32)
}

/// The PROPOSED height for one sculpt op at a point, from the read-only base
/// sample + params. Relative ops (Raise/Lower/Smooth) return `None` with no
/// base; Level is absolute. `strength` in `[0,1]` weights Smooth's pull toward
/// `region_mean`. See `src/AGENTS.md` §sculpt for the per-op table.
pub fn proposed_height(
    op: SculptOp,
    base: Option<f32>,
    target: f32,
    delta: f32,
    region_mean: Option<f32>,
    strength: f32,
) -> Option<f32> {
    match op {
        SculptOp::Raise => base.map(|b| b + delta),
        SculptOp::Lower => base.map(|b| b - delta),
        SculptOp::Level => Some(target),
        SculptOp::Smooth => match (base, region_mean) {
            (Some(b), Some(m)) => Some(b + (m - b) * strength.clamp(0.0, 1.0)),
            (b, _) => b,
        },
    }
}

/// Proposed heights at `sample_pts` for a sculpt op over `footprint`, WITHOUT
/// mutating `base` (NFR-1). Points outside the footprint gate to `None`; inside
/// points return the proposed height (or `None` for a relative op with no base).
/// Smooth uses the region mean sampled at `grid_step`.
#[allow(clippy::too_many_arguments)]
pub fn apply_sculpt(
    base: &impl Fn(f32, f32) -> Option<f32>,
    footprint: &Footprint,
    op: SculptOp,
    target: f32,
    delta: f32,
    strength: f32,
    grid_step: f32,
    sample_pts: &[[f32; 2]],
) -> Vec<Option<f32>> {
    let mean = if op == SculptOp::Smooth {
        region_mean_height(base, footprint, grid_step)
    } else {
        None
    };
    sample_pts
        .iter()
        .map(|&[x, z]| {
            if !footprint.contains(x, z) {
                return None;
            }
            proposed_height(op, base(x, z), target, delta, mean, strength)
        })
        .collect()
}

/// Cut vs fill volume of a modification region, in real units. Separated by
/// sign: `cut_m3` is net removed (proposed below base), `fill_m3` net added
/// (proposed above base). `scaled == false` when `world_scale` is unusable →
/// the figures are scene-unit³, never fabricated meters (NFR-4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutFill {
    pub cut_m3: f64,
    pub fill_m3: f64,
    pub scaled: bool,
}

impl CutFill {
    /// Net = fill − cut (positive when the region net-adds material).
    pub fn net_m3(&self) -> f64 {
        self.fill_m3 - self.cut_m3
    }
}

/// Heightfield prism/grid integration of cut/fill between the read-only `base`
/// and a `proposed` surface over `footprint`, in real units via the scale
/// authority (N-1; Q-4 planning-grade). For each `grid_step` cell centre inside
/// the footprint with both samples present, `dh = proposed − base` (scene units)
/// contributes `|dh_m| × cell_area_m²` to fill (`dh>0`) or cut (`dh<0`), where
/// `dh_m = dh/scale` and `cell_area_m² = grid_step²/scale²`. Documented
/// tolerance: O(grid_step) discretisation error — converges as `grid_step→0`;
/// pick a step ≤ ~¼ of the smallest relief feature for planning-grade results.
pub fn cut_fill_volume(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposed: &impl Fn(f32, f32) -> Option<f32>,
    footprint: &Footprint,
    grid_step: f32,
    world_scale: f64,
) -> CutFill {
    let scaled = world_scale.is_finite() && world_scale > 0.0;
    let s = crate::scale::sanitize_world_scale(world_scale);
    let cell_area_m2 = (grid_step as f64 / s) * (grid_step as f64 / s);
    let mut cut = 0.0f64;
    let mut fill = 0.0f64;
    for_each_cell(footprint, grid_step, |x, z| {
        if let (Some(b), Some(p)) = (base(x, z), proposed(x, z)) {
            let dh_m = (p - b) as f64 / s;
            if dh_m > 0.0 {
                fill += dh_m * cell_area_m2;
            } else if dh_m < 0.0 {
                cut += -dh_m * cell_area_m2;
            }
        }
    });
    CutFill {
        cut_m3: cut,
        fill_m3: fill,
        scaled,
    }
}

/// Convenience: cut/fill for a sculpt op applied over its footprint against the
/// read-only base (composes `region_mean` + `proposed_height` + integration).
#[allow(clippy::too_many_arguments)]
pub fn sculpt_cut_fill(
    base: &impl Fn(f32, f32) -> Option<f32>,
    footprint: &Footprint,
    op: SculptOp,
    target: f32,
    delta: f32,
    strength: f32,
    grid_step: f32,
    world_scale: f64,
) -> CutFill {
    let mean = if op == SculptOp::Smooth {
        region_mean_height(base, footprint, grid_step)
    } else {
        None
    };
    let proposed = |x: f32, z: f32| proposed_height(op, base(x, z), target, delta, mean, strength);
    cut_fill_volume(base, &proposed, footprint, grid_step, world_scale)
}

/// Visit every `grid_step` cell centre inside `footprint`'s bounds (degenerate
/// bounds / non-positive step → no visits). Shared by mean + volume so both
/// sample the SAME lattice (a region's mean and its cut/fill stay consistent).
fn for_each_cell(footprint: &Footprint, grid_step: f32, mut f: impl FnMut(f32, f32)) {
    let Some((min_x, min_z, max_x, max_z)) = footprint.bounds() else {
        return;
    };
    if !(grid_step.is_finite() && grid_step > 0.0) {
        return;
    }
    let nx = (((max_x - min_x) / grid_step).ceil() as i64).max(1);
    let nz = (((max_z - min_z) / grid_step).ceil() as i64).max(1);
    // Guard against a runaway lattice from a huge/degenerate footprint.
    if nx.saturating_mul(nz) > 4_000_000 {
        return;
    }
    let half = grid_step * 0.5;
    for iz in 0..nz {
        let z = min_z + iz as f32 * grid_step + half;
        for ix in 0..nx {
            let x = min_x + ix as f32 * grid_step + half;
            if footprint.contains(x, z) {
                f(x, z);
            }
        }
    }
}

/// A persistent, addressable modification-region (D-A8): footprint + op +
/// material tag + params. It is the SOURCE OF TRUTH for its baked contribution
/// (Q-2: delete reverts by dropping the record), reportable/queryable like any
/// node (N-10). Serde field names match the evolved `terrain.proposals` JSON so
/// fe-ui persists it via the existing `SetPetalTerrain` path (FR-6, no fork).
/// `material` defaults to `"earth"` (single-material this landing, Q-3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarthworkRegion {
    pub id: String,
    pub op: SculptOp,
    /// Footprint polygon as world `[x, z]` verts (the shape you report on).
    pub footprint: Vec<[f32; 2]>,
    #[serde(default = "default_material")]
    pub material: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f32>,
}

/// Default earthwork material tag (single-material this landing, Q-3).
fn default_material() -> String {
    "earth".to_string()
}

impl EarthworkRegion {
    /// The region's footprint as a [`Footprint::Polygon`] for the sculpt math.
    pub fn footprint_shape(&self) -> Footprint {
        Footprint::Polygon {
            verts: self.footprint.clone(),
        }
    }

    /// Cut/fill volume of this region against the read-only base surface.
    pub fn cut_fill(
        &self,
        base: &impl Fn(f32, f32) -> Option<f32>,
        grid_step: f32,
        world_scale: f64,
    ) -> CutFill {
        sculpt_cut_fill(
            base,
            &self.footprint_shape(),
            self.op,
            self.target_height.unwrap_or(0.0),
            self.delta.unwrap_or(0.0),
            1.0,
            grid_step,
            world_scale,
        )
    }
}

/// Parse the `terrain.proposals` array as earthwork regions; malformed records
/// are dropped (warn), never failing the set (mirrors `parse_proposals`).
pub fn parse_regions(value: &serde_json::Value) -> Vec<EarthworkRegion> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(
                |item| match serde_json::from_value::<EarthworkRegion>(item.clone()) {
                    Ok(r) => Some(r),
                    Err(err) => {
                        tracing::warn!(error = %err, "invalid earthwork region; skipping");
                        None
                    }
                },
            )
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f32) -> Footprint {
        Footprint::Rect {
            min: [0.0, 0.0],
            max: [size, size],
        }
    }

    #[test]
    fn footprint_contains_matches_shape() {
        let c = Footprint::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        };
        assert!(c.contains(3.0, 3.0)); // inside (dist ~4.24)
        assert!(!c.contains(4.0, 4.0)); // outside (dist ~5.66)
        let r = Footprint::Rect {
            min: [1.0, 1.0],
            max: [3.0, 5.0],
        };
        assert!(r.contains(2.0, 4.0));
        assert!(!r.contains(0.0, 4.0));
    }

    #[test]
    fn rect_bounds_normalize_swapped_corners() {
        let r = Footprint::Rect {
            min: [3.0, 5.0],
            max: [1.0, 1.0],
        };
        let (min_x, min_z, max_x, max_z) = r.bounds().unwrap();
        assert_eq!((min_x, min_z, max_x, max_z), (1.0, 1.0, 3.0, 5.0));
    }

    #[test]
    fn disc_to_polygon_is_closed_ring_within_radius() {
        let d = Footprint::Circle {
            center: [10.0, 0.0],
            radius: 2.0,
        };
        let poly = d.to_polygon();
        assert_eq!(poly.len(), DISC_SEGMENTS);
        for [x, z] in &poly {
            let r = ((x - 10.0).powi(2) + z.powi(2)).sqrt();
            assert!((r - 2.0).abs() < 1e-4, "vertex off the ring: r={r}");
        }
    }

    #[test]
    fn apply_gates_outside_and_applies_inside() {
        let base = |_x: f32, _z: f32| Some(1.0);
        let fp = square(10.0);
        let pts = [[5.0, 5.0], [50.0, 50.0]];
        let out = apply_sculpt(&base, &fp, SculptOp::Raise, 0.0, 3.0, 0.0, 1.0, &pts);
        assert_eq!(out, vec![Some(4.0), None]);
    }

    #[test]
    fn level_is_absolute_lower_is_relative() {
        let base = |_x: f32, _z: f32| Some(8.0);
        let fp = square(10.0);
        let level = apply_sculpt(
            &base,
            &fp,
            SculptOp::Level,
            2.0,
            0.0,
            0.0,
            1.0,
            &[[5.0, 5.0]],
        );
        assert_eq!(level, vec![Some(2.0)]);
        let lower = apply_sculpt(
            &base,
            &fp,
            SculptOp::Lower,
            0.0,
            3.0,
            0.0,
            1.0,
            &[[5.0, 5.0]],
        );
        assert_eq!(lower, vec![Some(5.0)]);
    }

    #[test]
    fn relative_op_without_base_yields_none() {
        let base = |_x: f32, _z: f32| None;
        let fp = square(10.0);
        let out = apply_sculpt(
            &base,
            &fp,
            SculptOp::Raise,
            0.0,
            1.0,
            0.0,
            1.0,
            &[[5.0, 5.0]],
        );
        assert_eq!(out, vec![None]);
    }

    #[test]
    fn smooth_relaxes_toward_region_mean() {
        // A ramp base h = x over a 10-wide square; mean ≈ 5. Full-strength smooth
        // pulls every point to the mean.
        let base = |x: f32, _z: f32| Some(x);
        let fp = square(10.0);
        let mean = region_mean_height(&base, &fp, 1.0).unwrap();
        assert!((mean - 5.0).abs() < 0.6, "mean ~5, got {mean}");
        let out = apply_sculpt(
            &base,
            &fp,
            SculptOp::Smooth,
            0.0,
            0.0,
            1.0,
            1.0,
            &[[2.0, 5.0]],
        );
        assert!((out[0].unwrap() - mean).abs() < 1e-4);
    }

    #[test]
    fn nfr1_base_sampler_unmutated_after_apply_and_volume() {
        // The closure borrows the grid immutably, so neither apply nor cut/fill
        // can write it. Byte-identity before/after is the NFR-1 proof.
        let grid: Vec<f32> = (0..100).map(|i| i as f32 * 0.25).collect();
        let snap = grid.clone();
        let base = |x: f32, z: f32| grid.get((z as usize) * 10 + (x as usize)).copied();
        let fp = square(9.0);
        let _ = apply_sculpt(
            &base,
            &fp,
            SculptOp::Raise,
            0.0,
            5.0,
            0.0,
            1.0,
            &[[4.0, 4.0]],
        );
        let _ = sculpt_cut_fill(&base, &fp, SculptOp::Level, 3.0, 0.0, 0.0, 1.0, 1.0);
        assert_eq!(grid, snap, "base heightfield mutated — NFR-1 violated");
    }

    #[test]
    fn cut_fill_flat_fill_matches_analytic_scaled() {
        // Flat base at 0, level a 10×10 scene-unit square up to target 2.
        // At 0.1 world-units/m the square is 100×100 m (10 000 m²) and the 2-unit
        // rise is 20 m → fill = 200 000 m³, cut = 0. Mirrors the proposal_report
        // convention exactly (evolve, don't fork).
        let base = |_x: f32, _z: f32| Some(0.0);
        let fp = square(10.0);
        let cf = sculpt_cut_fill(&base, &fp, SculptOp::Level, 2.0, 0.0, 0.0, 0.25, 0.1);
        assert!(cf.scaled);
        assert!(cf.cut_m3.abs() < 1.0, "no cut expected, got {}", cf.cut_m3);
        // O(step) discretisation: 0.25-step over a 10-unit square is tight.
        assert!(
            (cf.fill_m3 - 200_000.0).abs() / 200_000.0 < 0.02,
            "fill {} not within 2% of 200000",
            cf.fill_m3
        );
    }

    #[test]
    fn cut_fill_separates_cut_and_fill_over_relief() {
        // Base ramp h = x on [0,10]; level to target 5 → cut where x>5, fill where
        // x<5, symmetric → cut ≈ fill (both > 0). Analytic each ≈ ∫₀⁵(5−x)dx ·10
        // = 12.5·10 = 125 scene-unit³ (world_scale 1 → same in m³).
        let base = |x: f32, _z: f32| Some(x);
        let fp = square(10.0);
        let cf = sculpt_cut_fill(&base, &fp, SculptOp::Level, 5.0, 0.0, 0.0, 0.1, 1.0);
        assert!(cf.cut_m3 > 0.0 && cf.fill_m3 > 0.0);
        assert!(
            (cf.cut_m3 - cf.fill_m3).abs() < 5.0,
            "cut {} vs fill {} should be ~symmetric",
            cf.cut_m3,
            cf.fill_m3
        );
        assert!(
            (cf.fill_m3 - 125.0).abs() / 125.0 < 0.05,
            "fill {} not within 5% of analytic 125",
            cf.fill_m3
        );
    }

    #[test]
    fn cut_fill_unscaled_reports_scene_units_never_fake_meters() {
        let base = |_x: f32, _z: f32| Some(0.0);
        let fp = square(10.0);
        let cf = sculpt_cut_fill(&base, &fp, SculptOp::Level, 2.0, 0.0, 0.0, 0.25, f64::NAN);
        assert!(!cf.scaled, "bad scale must not claim meters");
        // scale sanitizes to 1.0 → raw scene units: 100 area × 2 rise = 200.
        assert!((cf.fill_m3 - 200.0).abs() / 200.0 < 0.02);
    }

    #[test]
    fn op_snake_round_trips_and_accepts_terrain_vocab() {
        for op in [
            SculptOp::Raise,
            SculptOp::Lower,
            SculptOp::Level,
            SculptOp::Smooth,
        ] {
            assert_eq!(SculptOp::from_snake(op.as_snake()), Some(op));
        }
        // Terrain-proposal vocabulary folds onto sculpt intents.
        assert_eq!(SculptOp::from_snake("flatten"), Some(SculptOp::Level));
        assert_eq!(SculptOp::from_snake("cut"), Some(SculptOp::Lower));
        assert_eq!(SculptOp::from_snake("fill"), Some(SculptOp::Raise));
        assert_eq!(SculptOp::from_snake("pad"), Some(SculptOp::Raise));
        assert_eq!(SculptOp::from_snake("nonsense"), None);
    }

    #[test]
    fn region_json_round_trips_with_material_default() {
        let region = EarthworkRegion {
            id: "r1".into(),
            op: SculptOp::Level,
            footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            material: "gravel".into(),
            target_height: Some(3.0),
            delta: None,
        };
        let json = serde_json::to_value(&region).unwrap();
        assert_eq!(json["op"], serde_json::json!("level"));
        assert_eq!(json["material"], serde_json::json!("gravel"));
        assert!(json.get("delta").is_none(), "None delta omitted");
        let back: EarthworkRegion = serde_json::from_value(json).unwrap();
        assert_eq!(back, region);
    }

    #[test]
    fn parse_regions_defaults_material_and_drops_invalid() {
        // A record with no `material` (e.g. a pre-sculpt proposal) defaults to
        // "earth"; an unknown op is dropped, never failing the set.
        let value = serde_json::json!([
            { "id": "r1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 2.0 },
            { "id": "bad", "op": "not_an_op", "footprint": [] }
        ]);
        let regions = parse_regions(&value);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].material, "earth");
        assert_eq!(regions[0].op, SculptOp::Raise);
    }

    #[test]
    fn region_cut_fill_uses_footprint_and_scale() {
        // A raise region: delta 2 over a 10×10 footprint at scale 1 → fill 200.
        let base = |_x: f32, _z: f32| Some(0.0);
        let region = EarthworkRegion {
            id: "r1".into(),
            op: SculptOp::Raise,
            footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            material: "earth".into(),
            target_height: None,
            delta: Some(2.0),
        };
        let cf = region.cut_fill(&base, 0.25, 1.0);
        assert!(cf.cut_m3.abs() < 1.0);
        assert!((cf.fill_m3 - 200.0).abs() / 200.0 < 0.02);
    }
}
