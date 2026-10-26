//! Pure selected-volume report math (spec FR-6/7/8, P2): true cut/fill for an
//! arbitrary selection footprint against the composite proposed surface, plus
//! a per-item standalone breakdown with overlap detection (D-DV-6). Bevy-free
//! + always compiled (same NFR-1 contract as `terrain_proposal.rs`/`sculpt`/
//! `delta_viz.rs`): every sampler is a read-only `&impl Fn(f32,f32)->
//! Option<f32>`, and every volume figure is computed by the ONE tested
//! integrator (`sculpt::cut_fill_volume`) — never re-derived (FR-5 extended
//! to the report). See `src/AGENTS.md` §volume-report (added by S5, which
//! wires this module into `lib.rs` and answers `VolumeReportRequest`).

use crate::sculpt::{
    cut_fill_volume, for_each_cell, proposed_height, region_mean_height, EarthworkRegion,
    Footprint, SculptOp,
};
use crate::terrain_proposal::{apply_proposal_over_base, point_in_polygon, TerrainProposal};

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
    let region_footprints: Vec<Footprint> = regions.iter().map(|r| r.footprint_shape()).collect();
    let region_means: Vec<Option<f32>> = regions
        .iter()
        .zip(&region_footprints)
        .map(|(r, fp)| {
            (r.op == SculptOp::Smooth)
                .then(|| region_mean_height(base, fp, step))
                .flatten()
        })
        .collect();

    let area_m2 = {
        let poly: Vec<[f64; 3]> = selection
            .to_polygon()
            .iter()
            .map(|&[x, z]| [x as f64, 0.0, z as f64])
            .collect();
        crate::ruler::polygon_area_m2(&poly, world_scale)
    };

    // -- composite total: one combined surface, record order (see doc above) --
    let composite_proposed = |x: f32, z: f32| -> Option<f32> {
        let mut h = base(x, z)?;
        for p in proposals {
            if point_in_polygon([x, z], &p.footprint) {
                let current = h;
                if let Some(next) = apply_proposal_over_base(&|_, _| Some(current), p, &[[x, z]])
                    .into_iter()
                    .next()
                    .flatten()
                {
                    h = next;
                }
            }
        }
        for (j, r) in regions.iter().enumerate() {
            if region_footprints[j].contains(x, z) {
                if let Some(next) = proposed_height(
                    r.op,
                    Some(h),
                    r.target_height.unwrap_or(0.0),
                    region_effective_delta(r),
                    region_means[j],
                    r.effective_strength(),
                ) {
                    h = next;
                }
            }
        }
        Some(h)
    };
    let total = cut_fill_volume(base, &composite_proposed, selection, step, world_scale);

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
            for (j, fp) in region_footprints.iter().enumerate() {
                if fp.contains(x, z) {
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
        let proposed =
            |x: f32, z: f32| apply_proposal_over_base(base, p, &[[x, z]]).into_iter().next().flatten();
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
    for (j, r) in regions.iter().enumerate() {
        let fp = &region_footprints[j];
        let mean = region_means[j];
        let proposed = |x: f32, z: f32| {
            if !fp.contains(x, z) {
                return None;
            }
            let b = base(x, z)?;
            proposed_height(
                r.op,
                Some(b),
                r.target_height.unwrap_or(0.0),
                region_effective_delta(r),
                mean,
                r.effective_strength(),
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

/// Mirrors `EarthworkRegion`'s private `effective_delta` (Level/Smooth store
/// blend STRENGTH in the `delta` slot, not a height delta, so it reads as 0
/// here); duplicated as a 3-line field read because the real method is
/// private to `sculpt` — see `EarthworkRegion::effective_strength` (pub) for
/// the strength half of the same mapping.
fn region_effective_delta(region: &EarthworkRegion) -> f32 {
    if matches!(region.op, SculptOp::Level | SculptOp::Smooth) {
        0.0
    } else {
        region.delta.unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_proposal::TerrainOp;

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

        assert!((report.area_m2 - 50.0).abs() / 50.0 < 0.02, "area {}", report.area_m2);
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
        let report =
            selection_volume_report(&base, &[proposal], &[], &selection, 0.25, f64::NAN);

        assert!(!report.scaled, "bad scale must not claim meters");
        assert!(!report.items[0].cut_m3.is_nan() && !report.items[0].fill_m3.is_nan());
        // scale sanitizes to 1.0 -> raw scene units: 100 area * 2 rise = 200.
        assert!((report.fill_m3 - 200.0).abs() / 200.0 < 0.03, "got {}", report.fill_m3);
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
        assert!(!disjoint.overlap, "disjoint footprints must not flag overlap");
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
        assert!((report.area_m2 - 10_000.0).abs() / 10_000.0 < 0.01, "area {}", report.area_m2);
        assert!(report.cut_m3.abs() < 1.0, "flat level should have no cut, got {}", report.cut_m3);
        assert!(
            (report.fill_m3 - 200_000.0).abs() / 200_000.0 < 0.02,
            "fill {} not within 2% of analytic 200000",
            report.fill_m3
        );
        assert!((report.net_m3 - report.fill_m3).abs() < 1e-6);
        assert!((report.items[0].fill_m3 - report.fill_m3).abs() < 1.0);
    }
}
