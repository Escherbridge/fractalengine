//! Relief-following delta lattice: the proposed-surface ghost + cut/fill
//! heatmap drape geometry for one footprint (spec FR-1, FR-2). Bevy-free +
//! always compiled (same NFR-1 contract as `terrain_proposal.rs`/`sculpt`):
//! every sampler is a read-only `&impl Fn(f32,f32)->Option<f32>`. See
//! `src/AGENTS.md` §delta-viz.

use crate::sculpt::{cut_fill_volume, CutFill, Footprint};

/// Per-footprint vertex cap (NFR-2): bounds one lattice's ghost/drape mesh so
/// a huge or degenerate footprint can't blow the frame/residency budget.
/// [`DeltaLattice::build`] auto-coarsens its step to respect this (never
/// silently drops the footprint) and warns when it does.
pub const DELTA_LATTICE_VERTEX_CAP: usize = 16_384;

/// Tiny world-unit offset lifting the heatmap drape clear of both the ghost
/// and the terrain mesh so the three surfaces never z-fight (FR-2 "+lift").
const DRAPE_LIFT: f32 = 0.02;

/// Relief-following ghost + cut/fill heatmap drape for one footprint, plus
/// the volume those same samples integrate to. See `src/AGENTS.md` §delta-viz.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaLattice {
    /// Proposed-surface vertices `[x, proposed_y, z]` (interior lattice, not a lid).
    pub ghost_positions: Vec<[f32; 3]>,
    /// Heatmap-drape vertices `[x, max(base, proposed) + lift, z]`; index-aligned with `ghost_positions`.
    pub drape_positions: Vec<[f32; 3]>,
    /// Per-vertex `proposed - base` in SCENE units (fill > 0, cut < 0); index-aligned with the positions.
    pub dh: Vec<f32>,
    /// Shared triangle index buffer for both position arrays.
    pub indices: Vec<u32>,
    /// Real-unit cut/fill for this exact lattice, via [`cut_fill_volume`] (FR-5 one truth).
    pub cut_fill: CutFill,
}

impl DeltaLattice {
    /// Build the ghost + drape lattice over `footprint` from read-only `base`/`proposed`
    /// samplers (NFR-1); auto-coarsens `grid_step` to the vertex cap (see `effective_step`).
    /// `None` for a degenerate footprint or non-finite/non-positive `grid_step`.
    pub fn build(
        base: &impl Fn(f32, f32) -> Option<f32>,
        proposed: &impl Fn(f32, f32) -> Option<f32>,
        footprint: &Footprint,
        grid_step: f32,
        world_scale: f64,
    ) -> Option<DeltaLattice> {
        let (min_x, min_z, max_x, max_z) = footprint.bounds()?;
        if !(grid_step.is_finite() && grid_step > 0.0) {
            return None;
        }

        let step = effective_step(min_x, min_z, max_x, max_z, grid_step);
        let nx = (((max_x - min_x) / step).ceil() as i64).max(1);
        let nz = (((max_z - min_z) / step).ceil() as i64).max(1);
        let corners_x = nx + 1;
        let corners_z = nz + 1;

        // Compact vertex slot per (ix, iz) grid corner; only qualifying
        // corners get one, so the position/dh arrays carry no holes/NaNs.
        let mut slot: Vec<Option<u32>> = vec![None; (corners_x * corners_z) as usize];
        let mut ghost_positions = Vec::new();
        let mut drape_positions = Vec::new();
        let mut dh = Vec::new();

        for iz in 0..corners_z {
            let z = min_z + iz as f32 * step;
            for ix in 0..corners_x {
                let x = min_x + ix as f32 * step;
                if !footprint.contains(x, z) {
                    continue;
                }
                let (Some(b), Some(p)) = (base(x, z), proposed(x, z)) else {
                    continue;
                };
                slot[(iz * corners_x + ix) as usize] = Some(ghost_positions.len() as u32);
                ghost_positions.push([x, p, z]);
                drape_positions.push([x, b.max(p) + DRAPE_LIFT, z]);
                dh.push(p - b);
            }
        }

        let slot_at = |ix: i64, iz: i64| -> Option<u32> {
            slot.get((iz * corners_x + ix) as usize).copied().flatten()
        };
        let mut indices = Vec::new();
        for iz in 0..nz {
            for ix in 0..nx {
                let (Some(a), Some(b), Some(c), Some(d)) = (
                    slot_at(ix, iz),
                    slot_at(ix + 1, iz),
                    slot_at(ix + 1, iz + 1),
                    slot_at(ix, iz + 1),
                ) else {
                    continue; // a corner is outside/uncovered — leave the hole
                };
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }

        let cut_fill = cut_fill_volume(base, proposed, footprint, step, world_scale);

        Some(DeltaLattice {
            ghost_positions,
            drape_positions,
            dh,
            indices,
            cut_fill,
        })
    }
}

/// Coarsen `grid_step` so the grid-corner count fits [`DELTA_LATTICE_VERTEX_CAP`]
/// (warns when it does, NFR-2); unchanged if it already fits. Bounded iteration
/// count — see `src/AGENTS.md` §delta-viz for why this can't stall.
fn effective_step(min_x: f32, min_z: f32, max_x: f32, max_z: f32, grid_step: f32) -> f32 {
    let corners_for = |step: f32| -> i64 {
        let nx = (((max_x - min_x) / step).ceil() as i64).max(1);
        let nz = (((max_z - min_z) / step).ceil() as i64).max(1);
        (nx + 1).saturating_mul(nz + 1)
    };
    if corners_for(grid_step) <= DELTA_LATTICE_VERTEX_CAP as i64 {
        return grid_step;
    }
    let mut step = grid_step;
    for _ in 0..8 {
        let corners = corners_for(step);
        if corners <= DELTA_LATTICE_VERTEX_CAP as i64 {
            break;
        }
        let scale = ((corners as f64) / (DELTA_LATTICE_VERTEX_CAP as f64))
            .sqrt()
            .max(1.001) as f32;
        step *= scale;
    }
    tracing::warn!(
        requested_step = grid_step as f64,
        coarsened_step = step as f64,
        cap = DELTA_LATTICE_VERTEX_CAP,
        "delta lattice exceeded the per-footprint vertex cap; coarsening step (NFR-2)"
    );
    step
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(min: [f32; 2], max: [f32; 2]) -> Footprint {
        Footprint::Rect { min, max }
    }

    #[test]
    fn lattice_ghost_follows_relief_inside_footprint() {
        // Sloped base (NOT flat), Raise by 2. Every ghost vertex must sit at
        // base+2 exactly, and there must be more vertices than the 4 corners
        // of the rect footprint — proving an interior lattice, not a lid
        // (the R-1 regression: a perimeter-only triangulation).
        let base = |x: f32, _z: f32| Some(x * 0.3);
        let proposed = |x: f32, z: f32| base(x, z).map(|b| b + 2.0);
        let fp = rect([0.0, 0.0], [10.0, 10.0]);
        let lattice = DeltaLattice::build(&base, &proposed, &fp, 2.0, 1.0).expect("lattice built");

        assert!(
            lattice.ghost_positions.len() > 4,
            "expected interior samples beyond the 4 footprint corners, got {}",
            lattice.ghost_positions.len()
        );
        for &[x, y, z] in &lattice.ghost_positions {
            let expected = base(x, z).unwrap() + 2.0;
            assert!(
                (y - expected).abs() < 1e-4,
                "ghost vertex at ({x},{z}) is y={y}, expected base+2={expected}"
            );
        }
    }

    #[test]
    fn heatmap_dh_sign_matches_cut_and_fill() {
        // Base descends left->right (h = 10-x); "Level" to the midline value
        // 5. Left of the midline the ground is HIGH (>5) so leveling removes
        // material -> dh<0 (cut). Right of the midline the ground is LOW
        // (<5) so leveling adds material -> dh>0 (fill).
        let base = |x: f32, _z: f32| Some(10.0 - x);
        let proposed = |_x: f32, _z: f32| Some(5.0);
        let fp = rect([0.0, 0.0], [10.0, 10.0]);
        let lattice = DeltaLattice::build(&base, &proposed, &fp, 1.0, 1.0).expect("lattice built");

        let dh_at = |target_x: f32| -> f32 {
            let (idx, _) = lattice
                .ghost_positions
                .iter()
                .enumerate()
                .find(|(_, p)| (p[0] - target_x).abs() < 1e-4 && (p[2] - 5.0).abs() < 1e-4)
                .expect("sample vertex present");
            lattice.dh[idx]
        };
        assert!(dh_at(2.0) < 0.0, "left of midline should be cut (dh<0)");
        assert!(dh_at(8.0) > 0.0, "right of midline should be fill (dh>0)");
    }

    #[test]
    fn lattice_integrated_volume_equals_cut_fill_volume() {
        // FR-5 one-truth invariant: the lattice's own `cut_fill` must be
        // exactly what an independent `cut_fill_volume` call on the SAME
        // inputs produces — not a second, drifting implementation.
        let base = |x: f32, z: f32| Some((x * 0.2).sin() * 3.0 + z * 0.1);
        let proposed = |x: f32, z: f32| base(x, z).map(|b| b + 1.5);
        let fp = rect([-4.0, -4.0], [6.0, 7.0]);
        let step = 0.5;
        let world_scale = 0.5;

        let lattice =
            DeltaLattice::build(&base, &proposed, &fp, step, world_scale).expect("lattice built");
        let direct = cut_fill_volume(&base, &proposed, &fp, step, world_scale);

        assert_eq!(lattice.cut_fill, direct);
    }

    #[test]
    fn lattice_volume_matches_composite_proposed_surface() {
        // Spec FR-5's shared-composite expectation at the delta_viz seam: a
        // lattice built from a `sculpt::composite_proposed_height`-backed
        // `proposed` sampler integrates the SAME volume `cut_fill_volume`
        // would for that composite surface — the lattice has no special-cased
        // path for multi-region input (see `composite_applies_regions_in_record_order`
        // in `sculpt/mod.rs` for the composite function's own migrated test).
        use crate::sculpt::{prepare_composite, EarthworkRegion, SculptOp};

        let base = |_x: f32, _z: f32| Some(1.0);
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let regions = vec![
            EarthworkRegion {
                id: "r1".into(),
                op: SculptOp::Raise,
                footprint: square.clone(),
                material: "earth".into(),
                target_height: None,
                delta: Some(2.0),
            },
            EarthworkRegion {
                id: "r2".into(),
                op: SculptOp::Level,
                footprint: square.clone(),
                material: "earth".into(),
                target_height: Some(0.5),
                delta: None,
            },
        ];
        let prepared = prepare_composite(&regions, &base, 1.0);
        let proposed = |x: f32, z: f32| {
            base(x, z).map(|b| crate::sculpt::composite_proposed_height(&prepared, b, x, z))
        };

        let fp = Footprint::Polygon { verts: square };
        let lattice = DeltaLattice::build(&base, &proposed, &fp, 1.0, 1.0).expect("lattice built");
        let direct = cut_fill_volume(&base, &proposed, &fp, 1.0, 1.0);
        assert_eq!(lattice.cut_fill, direct);
        // raise 1->3 then level to 0.5: net is a cut (3 -> 0.5), so dh<0 everywhere inside.
        assert!(!lattice.dh.is_empty(), "expected interior samples");
        assert!(lattice.dh.iter().all(|&d| d < 0.0));
    }

    #[test]
    fn lattice_respects_vertex_cap_and_reports_truncation() {
        // A huge footprint at a fine step would ask for far more than the
        // cap; `effective_step` must coarsen (observable: returned step is
        // larger than requested) and the built lattice must stay bounded.
        let (min_x, min_z, max_x, max_z) = (0.0f32, 0.0f32, 10_000.0f32, 10_000.0f32);
        let requested_step = 1.0f32;
        let coarsened = effective_step(min_x, min_z, max_x, max_z, requested_step);
        assert!(
            coarsened > requested_step,
            "expected coarsening, got step unchanged at {coarsened}"
        );

        let base = |_x: f32, _z: f32| Some(0.0);
        let proposed = |_x: f32, _z: f32| Some(1.0);
        let fp = rect([min_x, min_z], [max_x, max_z]);
        let lattice = DeltaLattice::build(&base, &proposed, &fp, requested_step, 1.0)
            .expect("lattice built despite truncation");
        assert!(
            lattice.ghost_positions.len() <= DELTA_LATTICE_VERTEX_CAP,
            "lattice exceeded the documented vertex cap: {}",
            lattice.ghost_positions.len()
        );
        assert!(
            !lattice.ghost_positions.is_empty(),
            "truncation must still yield a usable (coarser) lattice, not nothing"
        );
    }

    #[test]
    fn nfr1_base_untouched_by_delta_lattice() {
        // The base closure borrows a backing grid immutably; building a
        // lattice (ghost + drape + volume) cannot mutate it. Byte-identity
        // before/after is the NFR-1 proof (same pattern as sculpt/mod.rs and
        // terrain_proposal.rs).
        let grid: Vec<f32> = (0..100).map(|i| i as f32 * 0.5).collect();
        let snapshot = grid.clone();
        let base = |x: f32, z: f32| {
            let idx = (z as usize) * 10 + (x as usize);
            grid.get(idx).copied()
        };
        let proposed = |x: f32, z: f32| base(x, z).map(|b| b + 1.0);
        let fp = rect([0.0, 0.0], [9.0, 9.0]);

        let _ = DeltaLattice::build(&base, &proposed, &fp, 1.0, 1.0);
        assert_eq!(grid, snapshot, "base heightfield mutated — NFR-1 violated");
    }
}
