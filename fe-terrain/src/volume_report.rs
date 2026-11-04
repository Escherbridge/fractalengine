//! Pure selected volumes and changed-space overlaps; see src/AGENTS.md.

use crate::sculpt::{
    composite_proposed_height, cut_fill_volume, for_each_cell, prepare_composite, proposed_height,
    EarthworkRegion, Footprint,
};
use crate::terrain_proposal::{apply_proposal_over_base, point_in_polygon, TerrainProposal};

/// Sample changed terrain space against bounded object prisms; see src/AGENTS.md.
pub fn changed_space_overlaps(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposals: &[TerrainProposal],
    regions: &[EarthworkRegion],
    selection: &Footprint,
    step: f32,
    world_scale: f64,
    boxes: &[[[f32; 3]; 2]],
) -> Vec<Option<f64>> {
    let composite = prepare_composite(regions, base);
    let mut overlaps: Vec<Option<f64>> = boxes
        .iter()
        .map(|bounds| {
            (bounds.iter().flatten().all(|value| value.is_finite())
                && (0..3).all(|axis| bounds[0][axis] <= bounds[1][axis]))
            .then_some(0.0)
        })
        .collect();
    let Some((min_x, min_z, max_x, max_z)) = selection.bounds() else {
        return vec![None; boxes.len()];
    };
    if !step.is_finite()
        || step <= 0.0
        || [min_x, min_z, max_x, max_z]
            .into_iter()
            .any(|value| !value.is_finite())
    {
        return vec![None; boxes.len()];
    }
    let scale = crate::scale::sanitize_world_scale(world_scale);
    let half = f64::from(step) * 0.5;
    let mut visited = false;
    for_each_cell(selection, step, |x, z| {
        visited = true;
        let original = base(x, z).filter(|value| value.is_finite());
        let changed =
            combined_height(base, proposals, &composite, x, z).filter(|value| value.is_finite());
        for (bounds, overlap) in boxes.iter().zip(&mut overlaps) {
            let width = (f64::from(x) + half)
                .min(f64::from(max_x))
                .min(f64::from(bounds[1][0]))
                - (f64::from(x) - half)
                    .max(f64::from(min_x))
                    .max(f64::from(bounds[0][0]));
            let depth = (f64::from(z) + half)
                .min(f64::from(max_z))
                .min(f64::from(bounds[1][2]))
                - (f64::from(z) - half)
                    .max(f64::from(min_z))
                    .max(f64::from(bounds[0][2]));
            if width <= 0.0 || depth <= 0.0 || overlap.is_none() {
                continue;
            }
            let (Some(original), Some(changed)) = (original, changed) else {
                *overlap = None;
                continue;
            };
            let bottom = f64::from(original.min(changed)).max(f64::from(bounds[0][1]));
            let top = f64::from(original.max(changed)).min(f64::from(bounds[1][1]));
            let contribution = width * depth * (top - bottom).max(0.0) / scale.powi(3);
            if let Some(total) = overlap {
                *total += contribution;
                if !total.is_finite() {
                    *overlap = None;
                }
            }
        }
    });
    if visited {
        overlaps
    } else {
        vec![None; boxes.len()]
    }
}

fn combined_height(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposals: &[TerrainProposal],
    regions: &[crate::sculpt::CompositeRegion],
    x: f32,
    z: f32,
) -> Option<f32> {
    let mut height = base(x, z)?;
    for proposal in proposals {
        if point_in_polygon([x, z], &proposal.footprint) {
            let current = height;
            if let Some(next) = apply_proposal_over_base(&|_, _| Some(current), proposal, &[[x, z]])
                .into_iter()
                .next()
                .flatten()
            {
                height = next;
            }
        }
    }
    Some(composite_proposed_height(regions, height, x, z))
}

/// Which record kind a [`VolumeItem`] came from (fe-ui/S5 label proposals and
/// baked regions differently; see `fe-renderer::terrain_overlay::VolumeItem::kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeItemKind {
    /// A palette `TerrainProposal` (ghost-only, never baked).
    Proposal,
    /// A baked `EarthworkRegion`.
    Region,
}

/// One proposal/region overlapping the selection: standalone cut/fill clipped
/// to the selection (computed against the ORIGINAL base, not the composite —
/// "as if this were the only item"), plus how much of its clipped footprint
/// is shared with another item (FR-8).
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeItem {
    pub id: String,
    pub kind: VolumeItemKind,
    /// Snake-case op tag (`TerrainProposal::op_snake` / `SculptOp::as_snake`).
    pub op_snake: &'static str,
    /// Region material tag, or the proposal's material marker (`None` for a
    /// plain ghost proposal that is not also a bakeable earthwork record).
    pub material: Option<String>,
    pub cut_m3: f64,
    pub fill_m3: f64,
    /// Area (m², or world units² if unscaled) of this item's clipped
    /// footprint that is ALSO covered by at least one other item.
    pub overlap_area_m2: f64,
}

/// Selected-volume report for one footprint (FR-7/8): true geometry + the
/// composite total + a per-item breakdown. `overlap` is a GEOMETRIC flag (two
/// or more items share area inside the selection) — the practical reason the
/// per-item sum can diverge from the composite total, not a claim that it does
/// (additive ops like Raise/Lower on disjoint footprints keep them equal even
/// without overlap; non-additive ops like Level/Smooth under real overlap are
/// where they diverge). `scaled == false` (NFR-4) means every *_m3/*_m2 figure
/// above is in raw scene units, never fabricated meters.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionReport {
    /// Required cell centers on the selected integration lattice.
    pub expected_samples: u64,
    /// Cell centers with finite base and proposed samples.
    pub sampled_pairs: u64,
    pub area_m2: f64,
    /// Composite (combined-surface) cut volume clipped to the selection.
    pub cut_m3: f64,
    /// Composite (combined-surface) fill volume clipped to the selection.
    pub fill_m3: f64,
    pub net_m3: f64,
    pub items: Vec<VolumeItem>,
    pub overlap: bool,
    /// Lattice step actually integrated with, in the same units as the
    /// volume figures (real meters when `scaled`, else scene/world units).
    pub step_m: f64,
    pub scaled: bool,
}

/// True selected-volume report (FR-7/8): clips every `proposals`/`regions`
/// entry AND the combined composite surface to `selection`, integrating with
/// [`cut_fill_volume`] (never `area × delta` — that approximation is exactly
/// what this report replaces, R-4). `step` is the lattice cell size in scene
/// units; `world_scale` is the map-scale authority (N-1, NFR-4).
///
/// Composite order (undocumented elsewhere because this is the first seam to
/// combine both record kinds): every `proposals` entry is applied in array
/// order, THEN every `regions` entry in array order, each seeing the prior
/// result as its own base — the same "each containing record folds over the
/// running height" shape as `sculpt::composite_proposed_height`, just spanning
/// both vocabularies. Callers (S5) that want to avoid double-counting a record
/// that is both a ghost proposal and a bakeable region must pass it in only
/// one of the two slices.
///
/// Clipping trick (reused, not re-derived): [`cut_fill_volume`] always walks
/// `selection`'s own lattice, so handing it a `proposed` sampler that simply
/// returns the base height outside an item's footprint makes that cell
/// contribute zero — i.e. "item ∩ selection" falls out of "selection's walk +
/// a no-op outside the item", with no second integrator and no new
/// `Footprint` variant.
pub fn selection_volume_report(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposals: &[TerrainProposal],
    regions: &[EarthworkRegion],
    selection: &Footprint,
    step: f32,
    world_scale: f64,
) -> SelectionReport {
    // The SAME prepared composite the bake + baked heatmap use (FR-5): each
    // Smooth mean at its region's own lattice, never this selection's step.
    let composite = prepare_composite(regions, base);

    let area_m2 = {
        let poly: Vec<[f64; 3]> = selection
            .to_polygon()
            .iter()
            .map(|&[x, z]| [x as f64, 0.0, z as f64])
            .collect();
        crate::ruler::polygon_area_m2(&poly, world_scale)
    };

    // -- composite total: one combined surface, record order (see doc above) --
    let composite_proposed = |x: f32, z: f32| combined_height(base, proposals, &composite, x, z);
    let total = cut_fill_volume(base, &composite_proposed, selection, step, world_scale);
    let expected_samples = total.expected_cells;
    let sampled_pairs = total.sampled_cells;

    // -- geometric overlap pass: purely footprint containment, no heights --
    let item_count = proposals.len() + regions.len();
    let mut overlap_area_m2 = vec![0.0f64; item_count];
    let mut overlap = false;
    let s = crate::scale::sanitize_world_scale(world_scale);
    let cell_area_m2 = (step as f64 / s) * (step as f64 / s);
    {
        let mut touching: Vec<usize> = Vec::new();
        for_each_cell(selection, step, |x, z| {
            if base(x, z).is_none() {
                return;
            }
            touching.clear();
            for (i, p) in proposals.iter().enumerate() {
                if point_in_polygon([x, z], &p.footprint) {
                    touching.push(i);
                }
            }
            for (j, c) in composite.iter().enumerate() {
                if c.footprint.contains(x, z) {
                    touching.push(proposals.len() + j);
                }
            }
            if touching.len() > 1 {
                overlap = true;
                for &idx in &touching {
                    overlap_area_m2[idx] += cell_area_m2;
                }
            }
        });
    }

    // -- per-item standalone, clipped to the selection via the no-op trick --
    let mut items = Vec::with_capacity(item_count);
    for (i, p) in proposals.iter().enumerate() {
        let proposed = |x: f32, z: f32| {
            apply_proposal_over_base(base, p, &[[x, z]])
                .into_iter()
                .next()
                .flatten()
        };
        let cf = cut_fill_volume(base, &proposed, selection, step, world_scale);
        items.push(VolumeItem {
            id: p.id.clone(),
            kind: VolumeItemKind::Proposal,
            op_snake: p.op_snake(),
            material: p.material.clone(),
            cut_m3: cf.cut_m3,
            fill_m3: cf.fill_m3,
            overlap_area_m2: overlap_area_m2[i],
        });
    }
    for (j, (r, c)) in regions.iter().zip(&composite).enumerate() {
        let proposed = |x: f32, z: f32| {
            if !c.footprint.contains(x, z) {
                return None;
            }
            proposed_height(
                c.op,
                Some(base(x, z)?),
                c.target,
                c.delta,
                c.mean,
                c.strength,
            )
        };
        let cf = cut_fill_volume(base, &proposed, selection, step, world_scale);
        items.push(VolumeItem {
            id: r.id.clone(),
            kind: VolumeItemKind::Region,
            op_snake: r.op.as_snake(),
            material: Some(r.material.clone()),
            cut_m3: cf.cut_m3,
            fill_m3: cf.fill_m3,
            overlap_area_m2: overlap_area_m2[proposals.len() + j],
        });
    }

    SelectionReport {
        expected_samples,
        sampled_pairs,
        area_m2,
        cut_m3: total.cut_m3,
        fill_m3: total.fill_m3,
        net_m3: total.net_m3(),
        items,
        overlap,
        step_m: step as f64 / s,
        scaled: total.scaled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::{region_grid_step, SculptOp};
    use crate::terrain_proposal::TerrainOp;

    #[test]
    fn selection_total_and_items_share_the_bake_composite() {
        // FR-5 extended to the report (S7 fix): the total must be exactly
        // `cut_fill_volume` over the SAME `prepare_composite` surface the bake
        // uses, and a region's per-item figure (selection == its footprint, at
        // its own lattice step — what S5 sends) must equal the standalone
        // volume persisted on its node. Smooth over relief is the op whose
        // mean made the old per-selection-step copy drift.
        let base = |x: f32, z: f32| Some((x * 0.37).sin() * 4.0 + z * 0.3);
        let smooth = EarthworkRegion {
            id: "s".into(),
            op: SculptOp::Smooth,
            footprint: rect_verts([0.0, 0.0], [10.0, 10.0]),
            material: "earth".into(),
            target_height: None,
            delta: Some(0.8),
        };
        let raise = EarthworkRegion {
            id: "r".into(),
            op: SculptOp::Raise,
            footprint: rect_verts([4.0, 4.0], [30.0, 30.0]),
            material: "earth".into(),
            target_height: None,
            delta: Some(1.0),
        };
        let regions = vec![smooth.clone(), raise];
        let selection = Footprint::Polygon {
            verts: smooth.footprint.clone(),
        };
        let step = region_grid_step(selection.bounds());
        let report = selection_volume_report(&base, &[], &regions, &selection, step, 1.0);

        let prepared = prepare_composite(&regions, &base);
        let surface =
            |x: f32, z: f32| base(x, z).map(|b| composite_proposed_height(&prepared, b, x, z));
        let expected = cut_fill_volume(&base, &surface, &selection, step, 1.0);
        assert_eq!(
            (report.cut_m3, report.fill_m3),
            (expected.cut_m3, expected.fill_m3)
        );

        let item = report
            .items
            .iter()
            .find(|i| i.id == "s")
            .expect("smooth item");
        let persisted = smooth.cut_fill(&base, step, 1.0);
        assert_eq!(
            (item.cut_m3, item.fill_m3),
            (persisted.cut_m3, persisted.fill_m3)
        );
        assert!(report.overlap, "the raise overlaps the smooth region");
        assert_eq!(report.expected_samples, 128 * 128);
        assert_eq!(report.sampled_pairs, report.expected_samples);
        let missing = selection_volume_report(&|_, _| None, &[], &regions, &selection, step, 1.0);
        assert_eq!(missing.expected_samples, 128 * 128);
        assert_eq!(missing.sampled_pairs, 0);
        let partial = selection_volume_report(
            &|x, _| (x < 5.0).then_some(0.0),
            &[],
            &regions,
            &selection,
            step,
            1.0,
        );
        assert_eq!(partial.expected_samples, 128 * 128);
        assert_eq!(partial.sampled_pairs, 128 * 64);
    }

    fn square(min: [f32; 2], max: [f32; 2]) -> Footprint {
        Footprint::Rect { min, max }
    }

    fn raise_proposal(id: &str, footprint: Vec<[f32; 2]>, delta: f32) -> TerrainProposal {
        TerrainProposal {
            id: id.into(),
            op: TerrainOp::Raise,
            footprint,
            target_height: None,
            delta: Some(delta),
            material: None,
        }
    }

    fn rect_verts(min: [f32; 2], max: [f32; 2]) -> Vec<[f32; 2]> {
        vec![
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
        ]
    }

    #[test]
    fn selection_report_clips_overlapping_regions_to_selection() {
        // Region spans the FULL 10x10 square (unclipped fill would be
        // 10*10*2=200); the selection only covers the left half (5x10). The
        // regression this guards: using the region's own footprint instead
        // of clipping to the selection would report ~200, not ~100.
        let base = |_x: f32, _z: f32| Some(0.0);
        let region = EarthworkRegion {
            id: "r1".into(),
            op: SculptOp::Raise,
            footprint: rect_verts([0.0, 0.0], [10.0, 10.0]),
            material: "earth".into(),
            target_height: None,
            delta: Some(2.0),
        };
        let selection = square([0.0, 0.0], [5.0, 10.0]);
        let report = selection_volume_report(&base, &[], &[region], &selection, 0.25, 1.0);

        assert!(
            (report.area_m2 - 50.0).abs() / 50.0 < 0.02,
            "area {}",
            report.area_m2
        );
        assert_eq!(report.items.len(), 1);
        assert!(
            (report.items[0].fill_m3 - 100.0).abs() / 100.0 < 0.03,
            "clipped fill should be ~half the unclipped 200, got {}",
            report.items[0].fill_m3
        );
        assert!(
            (report.fill_m3 - report.items[0].fill_m3).abs() < 1.0,
            "single-item total should match the item"
        );
        assert!(!report.overlap);
    }

    #[test]
    fn selection_report_unscaled_never_fabricates_meters() {
        let base = |_x: f32, _z: f32| Some(0.0);
        let selection = square([0.0, 0.0], [10.0, 10.0]);
        let proposal = raise_proposal("p1", rect_verts([0.0, 0.0], [10.0, 10.0]), 2.0);
        let report = selection_volume_report(&base, &[proposal], &[], &selection, 0.25, f64::NAN);

        assert!(!report.scaled, "bad scale must not claim meters");
        assert!(!report.items[0].cut_m3.is_nan() && !report.items[0].fill_m3.is_nan());
        // scale sanitizes to 1.0 -> raw scene units: 100 area * 2 rise = 200.
        assert!(
            (report.fill_m3 - 200.0).abs() / 200.0 < 0.03,
            "got {}",
            report.fill_m3
        );
    }

    #[test]
    fn per_item_sum_equals_total_when_items_disjoint_and_flags_overlap_otherwise() {
        let base = |_x: f32, _z: f32| Some(0.0);
        let selection = square([0.0, 0.0], [20.0, 10.0]);

        // Disjoint: side-by-side, non-overlapping footprints.
        let left = raise_proposal("left", rect_verts([0.0, 0.0], [10.0, 10.0]), 2.0);
        let right = raise_proposal("right", rect_verts([10.0, 0.0], [20.0, 10.0]), 3.0);
        let disjoint = selection_volume_report(&base, &[left, right], &[], &selection, 0.25, 1.0);
        let item_fill_sum: f64 = disjoint.items.iter().map(|i| i.fill_m3).sum();
        let item_cut_sum: f64 = disjoint.items.iter().map(|i| i.cut_m3).sum();
        assert!(
            (item_fill_sum - disjoint.fill_m3).abs() < 1.0,
            "disjoint fill sum {} should equal total {}",
            item_fill_sum,
            disjoint.fill_m3
        );
        assert!((item_cut_sum - disjoint.cut_m3).abs() < 1.0);
        assert!(
            !disjoint.overlap,
            "disjoint footprints must not flag overlap"
        );
        assert!(disjoint.items.iter().all(|i| i.overlap_area_m2 == 0.0));

        // Overlapping: both proposals cover the same 10x10 region.
        let a = raise_proposal("a", rect_verts([0.0, 0.0], [10.0, 10.0]), 2.0);
        let b = raise_proposal("b", rect_verts([0.0, 0.0], [10.0, 10.0]), 3.0);
        let overlapping = selection_volume_report(&base, &[a, b], &[], &selection, 0.25, 1.0);
        assert!(overlapping.overlap, "shared footprints must flag overlap");
        assert!(overlapping.items.iter().all(|i| i.overlap_area_m2 > 0.0));
    }

    #[test]
    fn selection_report_matches_analytic_flat_fill() {
        // Mirrors `sculpt::cut_fill_flat_fill_matches_analytic_scaled`: flat
        // base at 0, a Level region to target 2 over a 10x10 scene-unit
        // square, 0.1 world-units/m -> 100x100 m, 20 m rise -> fill 200000 m3.
        let base = |_x: f32, _z: f32| Some(0.0);
        let region = EarthworkRegion {
            id: "r1".into(),
            op: SculptOp::Level,
            footprint: rect_verts([0.0, 0.0], [10.0, 10.0]),
            material: "earth".into(),
            target_height: Some(2.0),
            delta: None,
        };
        let selection = square([0.0, 0.0], [10.0, 10.0]);
        let report = selection_volume_report(&base, &[], &[region], &selection, 0.25, 0.1);

        assert!(report.scaled);
        assert!(
            (report.area_m2 - 10_000.0).abs() / 10_000.0 < 0.01,
            "area {}",
            report.area_m2
        );
        assert!(
            report.cut_m3.abs() < 1.0,
            "flat level should have no cut, got {}",
            report.cut_m3
        );
        assert!(
            (report.fill_m3 - 200_000.0).abs() / 200_000.0 < 0.02,
            "fill {} not within 2% of analytic 200000",
            report.fill_m3
        );
        assert!((report.net_m3 - report.fill_m3).abs() < 1e-6);
        assert!((report.items[0].fill_m3 - report.fill_m3).abs() < 1.0);
        let raised = EarthworkRegion {
            id: "raised".into(),
            op: SculptOp::Raise,
            footprint: rect_verts([-5.0, -5.0], [5.0, 5.0]),
            material: "clay".into(),
            target_height: None,
            delta: Some(2.0),
        };
        let footprint = square([-5.0, -5.0], [5.0, 5.0]);
        let boxes = [
            [[-1.0, 0.5, -1.0], [1.0, 1.5, 1.0]],
            [[-1.0, 3.0, -1.0], [1.0, 4.0, 1.0]],
            [[4.0, 0.5, -1.0], [7.0, 1.5, 1.0]],
            [[20.0, 0.5, -1.0], [22.0, 1.5, 1.0]],
            [[f32::NAN, 0.5, -1.0], [1.0, 1.5, 1.0]],
        ];
        let overlaps = changed_space_overlaps(
            &base,
            &[],
            std::slice::from_ref(&raised),
            &footprint,
            10.0 / 128.0,
            1.0,
            &boxes,
        );
        for (actual, expected) in overlaps[..4].iter().zip([4.0, 0.0, 2.0, 0.0]) {
            assert!(
                (actual.unwrap() - expected).abs() < 1e-9,
                "changed-space overlap {actual:?} versus {expected}"
            );
        }
        assert!(overlaps[4].is_none());
        assert!(changed_space_overlaps(
            &|_, _| None,
            &[],
            std::slice::from_ref(&raised),
            &footprint,
            10.0 / 128.0,
            1.0,
            &boxes
        )[0]
        .is_none());
        let lower = EarthworkRegion {
            op: SculptOp::Lower,
            delta: Some(1.0),
            ..raised.clone()
        };
        let buried = [[[-1.0, -0.75, -1.0], [1.0, -0.25, 1.0]]];
        assert!(
            (changed_space_overlaps(&base, &[], &[lower], &footprint, 10.0 / 128.0, 1.0, &buried)
                [0]
            .unwrap()
                - 2.0)
                .abs()
                < 1e-9
        );
        assert!(
            (changed_space_overlaps(&base, &[], &[raised], &footprint, 10.0 / 128.0, 0.5, &boxes)
                [0]
            .unwrap()
                - 32.0)
                .abs()
                < 1e-9
        );
    }
}
