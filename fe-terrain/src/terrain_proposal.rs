//! Non-destructive terrain proposal records + pure apply/report geometry;
//! see `src/AGENTS.md` §terrain-proposals. NEVER writes true terrain (NFR-1).

use serde::{Deserialize, Serialize};

use crate::sculpt::{CutFill, Footprint};

/// A terrain edit operation kind. A proposal is an ANALYTICS overlay, never a
/// destructive heightfield/tileset write (NFR-1); serde is snake_case to match
/// the persisted `terrain.proposals` JSON contract the fe-ui side mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainOp {
    Raise,
    Lower,
    Flatten,
    Ramp,
    Slope,
    Pad,
    Cut,
    Fill,
    /// Sculpt-region vocabulary (T3): absolute flatten-to-target. Present so a
    /// `terrain.proposals` block holding earthwork regions still parses as a
    /// `TerrainConfig` — see `src/AGENTS.md` §sculpt.
    Level,
    /// Sculpt-region vocabulary (T3): relax toward the region mean. Identity at
    /// this seam (the real math lives in `sculpt`; regions never ghost).
    Smooth,
}

impl TerrainOp {
    /// Snake-case tag (matches the JSON `op` field); used for per-op overlay tint.
    pub fn as_snake(&self) -> &'static str {
        match self {
            TerrainOp::Raise => "raise",
            TerrainOp::Lower => "lower",
            TerrainOp::Flatten => "flatten",
            TerrainOp::Ramp => "ramp",
            TerrainOp::Slope => "slope",
            TerrainOp::Pad => "pad",
            TerrainOp::Cut => "cut",
            TerrainOp::Fill => "fill",
            TerrainOp::Level => "level",
            TerrainOp::Smooth => "smooth",
        }
    }
}

/// A single proposed terrain edit over a closed footprint polygon (XZ world
/// units). `target_height` is absolute (flatten/pad/…); `delta` is relative
/// (raise/lower/…). Fields map 1:1 to the persisted JSON contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainProposal {
    pub id: String,
    pub op: TerrainOp,
    /// Footprint polygon as world-space `[x, z]` vertices (need not be closed).
    pub footprint: Vec<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f32>,
    /// Earthwork-region marker (T3): the sculpt commit path always writes a
    /// `material` tag; the plain proposal path never does. Presence is THE
    /// region-vs-proposal predicate — see `src/AGENTS.md` §sculpt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
}

impl TerrainProposal {
    /// Snake-case op tag for overlay tint / JSON parity.
    pub fn op_snake(&self) -> &'static str {
        self.op.as_snake()
    }

    /// Ghost-suppression predicate (T3): a record carrying a `material` tag is
    /// a baked earthwork region, not a ghosted proposal.
    pub fn is_earthwork_region(&self) -> bool {
        self.material.is_some()
    }
}

/// Axis-aligned footprint bounds `(min_x, min_z, max_x, max_z)`; `None` when empty.
/// `pub(crate)` so the `sculpt` module reuses one tested bounds helper (N-7).
pub(crate) fn footprint_bounds(footprint: &[[f32; 2]]) -> Option<(f32, f32, f32, f32)> {
    let mut it = footprint.iter();
    let &[fx, fz] = it.next()?;
    let (mut min_x, mut min_z, mut max_x, mut max_z) = (fx, fz, fx, fz);
    for &[x, z] in it {
        min_x = min_x.min(x);
        min_z = min_z.min(z);
        max_x = max_x.max(x);
        max_z = max_z.max(z);
    }
    Some((min_x, min_z, max_x, max_z))
}

/// Ramp/slope gradient params at world-x: normalized fraction across the
/// footprint X extent `[0,1]` and the raw world-unit run from the min-X edge.
fn ramp_params(x: f32, bounds: (f32, f32, f32, f32)) -> (f32, f32) {
    let (min_x, _, max_x, _) = bounds;
    let span = max_x - min_x;
    let run = x - min_x;
    let frac = if span > 0.0 {
        (run / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (frac, run)
}

/// Even-odd (ray-cast) point-in-polygon test on the XZ plane. A point exactly
/// on a vertex/edge may fall either way — acceptable for footprint gating.
/// `pub(crate)` so the `sculpt` module reuses one tested gate (N-7).
pub(crate) fn point_in_polygon(p: [f32; 2], polygon: &[[f32; 2]]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let (px, pz) = (p[0], p[1]);
    let mut inside = false;
    let n = polygon.len();
    let mut j = n - 1;
    for i in 0..n {
        let [xi, zi] = polygon[i];
        let [xj, zj] = polygon[j];
        let intersects = ((zi > pz) != (zj > pz)) && (px < (xj - xi) * (pz - zi) / (zj - zi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// PROPOSED height at one point from the op, the READ-ONLY base sample, and the
/// proposal params. Relative ops (raise/lower/slope/ramp) need a base and return
/// `None` when it is absent; absolute ops (flatten/pad/cut/fill w/ target) do not.
/// See `src/AGENTS.md` §terrain-proposals for the per-op semantics table.
fn proposed_height_at(
    op: TerrainOp,
    base: Option<f32>,
    target: Option<f32>,
    delta: f32,
    frac: f32,
    run: f32,
) -> Option<f32> {
    match op {
        TerrainOp::Raise => base.map(|b| b + delta),
        TerrainOp::Lower => base.map(|b| b - delta),
        TerrainOp::Flatten => target.or(base),
        // Pad: fill-only platform — never below existing ground.
        TerrainOp::Pad => match (base, target) {
            (Some(b), Some(t)) => Some(b.max(t)),
            (b, t) => t.or(b),
        },
        // Cut: remove-only — never above existing ground.
        TerrainOp::Cut => match (base, target) {
            (Some(b), Some(t)) => Some(b.min(t)),
            (Some(b), None) => Some(b - delta),
            (None, t) => t,
        },
        // Fill: add-only — never below existing ground.
        TerrainOp::Fill => match (base, target) {
            (Some(b), Some(t)) => Some(b.max(t)),
            (Some(b), None) => Some(b + delta),
            (None, t) => t,
        },
        // Ramp: lerp base→target across the footprint X extent (or base + delta·frac).
        TerrainOp::Ramp => match target {
            Some(tgt) => base.map(|b| b + (tgt - b) * frac),
            None => base.map(|b| b + delta * frac),
        },
        // Slope: constant grade `delta` (rise per world-unit run) along +X.
        TerrainOp::Slope => base.map(|b| b + delta * run),
        // Level: absolute flatten-to-target (sculpt vocabulary, == Flatten here).
        TerrainOp::Level => target.or(base),
        // Smooth: identity at this seam — the mean-relax math lives in `sculpt`
        // and regions never render as ghosts (see `src/AGENTS.md` §sculpt).
        TerrainOp::Smooth => base,
    }
}

/// Compute PROPOSED heights at `sample_pts` WITHOUT mutating `base`
/// (NFR-1: base is `&impl Fn`, structurally read-only). Points outside the
/// footprint return `None` (the proposal does not cover them); inside points
/// return the proposed height, or `None` for a relative op with no base sample.
pub fn apply_proposal_over_base(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposal: &TerrainProposal,
    sample_pts: &[[f32; 2]],
) -> Vec<Option<f32>> {
    let bounds = footprint_bounds(&proposal.footprint);
    let delta = proposal.delta.unwrap_or(0.0);
    sample_pts
        .iter()
        .map(|&[x, z]| {
            if !point_in_polygon([x, z], &proposal.footprint) {
                return None;
            }
            let (frac, run) = bounds.map(|b| ramp_params(x, b)).unwrap_or((0.0, 0.0));
            proposed_height_at(
                proposal.op,
                base(x, z),
                proposal.target_height,
                delta,
                frac,
                run,
            )
        })
        .collect()
}

/// Triangulated ghost-overlay geometry (`positions`, `indices`) for a proposal's
/// footprint lifted to PROPOSED heights over the READ-ONLY `base`. `None` for a
/// degenerate footprint (<3 verts) or a triangulation failure. A relative op with
/// no base sample falls back to `target_height` then `0.0` so the ghost still
/// renders (it is intent, not ground truth).
pub fn proposal_overlay_mesh_data(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposal: &TerrainProposal,
) -> Option<(Vec<[f32; 3]>, Vec<u32>)> {
    let fp = &proposal.footprint;
    if fp.len() < 3 {
        return None;
    }
    let flat: Vec<f64> = fp.iter().flat_map(|&[x, z]| [x as f64, z as f64]).collect();
    let tris = earcutr::earcut(&flat, &[], 2).ok()?;
    if tris.is_empty() {
        return None;
    }
    let bounds = footprint_bounds(fp)?;
    let delta = proposal.delta.unwrap_or(0.0);
    let positions: Vec<[f32; 3]> = fp
        .iter()
        .map(|&[x, z]| {
            let (frac, run) = ramp_params(x, bounds);
            let y = proposed_height_at(
                proposal.op,
                base(x, z),
                proposal.target_height,
                delta,
                frac,
                run,
            )
            .or(proposal.target_height)
            .unwrap_or(0.0);
            [x, y, z]
        })
        .collect();
    let indices: Vec<u32> = tris.iter().map(|&i| i as u32).collect();
    Some((positions, indices))
}

/// Parse the persisted `terrain.proposals` array; invalid records are dropped
/// (warn) rather than failing the whole set. Non-array JSON yields an empty set.
pub fn parse_proposals(value: &serde_json::Value) -> Vec<TerrainProposal> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(
                |item| match serde_json::from_value::<TerrainProposal>(item.clone()) {
                    Ok(p) => Some(p),
                    Err(err) => {
                        tracing::warn!(error = %err, "invalid terrain proposal; skipping");
                        None
                    }
                },
            )
            .collect(),
        _ => Vec::new(),
    }
}

/// Serialize a proposal set back to the `terrain.proposals` JSON array.
pub fn to_json(proposals: &[TerrainProposal]) -> serde_json::Value {
    serde_json::to_value(proposals).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

/// Metric report for a proposal (FR-6). Real meters when `world_scale` is a
/// finite `> 0` map scale; otherwise world units with `scaled == false` (NFR-4:
/// never fabricate meters). Reuses the tested `ruler` math.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProposalReport {
    /// Footprint bounding extent `[x, z]` (real meters when `scaled`, else world units).
    pub extent_m: [f64; 2],
    /// Footprint planar area (m² when `scaled`, else world units²).
    pub area_m2: f64,
    /// Representative cut/fill volume = area × |delta| (m³ when `scaled`).
    pub volume_m3: f64,
    /// Representative grade `100 × |delta| / max horizontal extent` (%); scale-invariant.
    pub slope_pct: f64,
    /// Compass bearing of the first footprint edge (degrees, `[0, 360)`).
    pub bearing_deg: f64,
    /// `false` when no map scale is set — values are world units, not meters.
    pub scaled: bool,
}

/// Compute a metric [`ProposalReport`]; see `src/AGENTS.md` §terrain-proposals.
pub fn proposal_report(proposal: &TerrainProposal, world_scale: f64) -> ProposalReport {
    let scaled = world_scale.is_finite() && world_scale > 0.0;
    let s = crate::scale::sanitize_world_scale(world_scale);
    let (ext_x_world, ext_z_world) = match footprint_bounds(&proposal.footprint) {
        Some((min_x, min_z, max_x, max_z)) => (max_x - min_x, max_z - min_z),
        None => (0.0, 0.0),
    };
    let extent_m = [(ext_x_world as f64) / s, (ext_z_world as f64) / s];

    // Area via the tested shoelace (footprint is XZ → lift to [x, 0, z]).
    let poly3: Vec<[f64; 3]> = proposal
        .footprint
        .iter()
        .map(|&[x, z]| [x as f64, 0.0, z as f64])
        .collect();
    let area_m2 = crate::ruler::polygon_area_m2(&poly3, world_scale);

    let delta_abs = proposal.delta.unwrap_or(0.0).abs() as f64;
    // delta and area both scale by 1/s and 1/s² respectively → real volume.
    let volume_m3 = area_m2 * (delta_abs / s);

    // Slope is scale-invariant: numerator and denominator are both world units.
    let max_run_world = ext_x_world.max(ext_z_world) as f64;
    let slope_pct = if max_run_world > 0.0 {
        100.0 * delta_abs / max_run_world
    } else {
        0.0
    };

    let bearing_deg = if proposal.footprint.len() >= 2 {
        let a = proposal.footprint[0];
        let b = proposal.footprint[1];
        crate::ruler::bearing_deg(
            [a[0] as f64, 0.0, a[1] as f64],
            [b[0] as f64, 0.0, b[1] as f64],
        )
    } else {
        0.0
    };

    ProposalReport {
        extent_m,
        area_m2,
        volume_m3,
        slope_pct,
        bearing_deg,
        scaled,
    }
}

/// True cut/fill volume of a proposal against the READ-ONLY base surface,
/// integrated on a `grid_step` lattice in real units (FR-4, Q-4 planning-grade).
/// Reuses the proposal's own per-op proposed height ([`apply_proposal_over_base`])
/// as the target surface, so every existing op (raise…fill) yields SEPARATED
/// cut/fill without forking the sculpt engine (FR-6). Points where the proposal
/// leaves the surface unchanged (outside the footprint, or a relative op with no
/// base) contribute nothing. See `src/AGENTS.md` §sculpt.
pub fn proposal_cut_fill(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposal: &TerrainProposal,
    grid_step: f32,
    world_scale: f64,
) -> CutFill {
    let footprint = Footprint::Polygon {
        verts: proposal.footprint.clone(),
    };
    let proposed = |x: f32, z: f32| {
        apply_proposal_over_base(base, proposal, &[[x, z]])
            .into_iter()
            .next()
            .flatten()
    };
    crate::sculpt::cut_fill_volume(base, &proposed, &footprint, grid_step, world_scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10×10 square footprint at the XZ origin.
    fn unit_square() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    fn raise(delta: f32) -> TerrainProposal {
        TerrainProposal {
            id: "p1".into(),
            op: TerrainOp::Raise,
            footprint: unit_square(),
            target_height: None,
            delta: Some(delta),
            material: None,
        }
    }

    #[test]
    fn apply_raise_adds_delta_to_base() {
        let base = |_x: f32, _z: f32| Some(5.0);
        let out = apply_proposal_over_base(&base, &raise(3.0), &[[5.0, 5.0]]);
        assert_eq!(out, vec![Some(8.0)]);
    }

    #[test]
    fn apply_lower_subtracts_delta_from_base() {
        let base = |_x: f32, _z: f32| Some(5.0);
        let p = TerrainProposal {
            op: TerrainOp::Lower,
            ..raise(2.0)
        };
        let out = apply_proposal_over_base(&base, &p, &[[5.0, 5.0]]);
        assert_eq!(out, vec![Some(3.0)]);
    }

    #[test]
    fn apply_flatten_uses_absolute_target_ignoring_base() {
        let base = |_x: f32, _z: f32| Some(5.0);
        let p = TerrainProposal {
            op: TerrainOp::Flatten,
            target_height: Some(12.5),
            delta: None,
            ..raise(0.0)
        };
        let out = apply_proposal_over_base(&base, &p, &[[2.0, 2.0]]);
        assert_eq!(out, vec![Some(12.5)]);
    }

    #[test]
    fn footprint_gates_points_outside_polygon_to_none() {
        let base = |_x: f32, _z: f32| Some(1.0);
        let pts = [[5.0, 5.0], [50.0, 50.0], [-1.0, 5.0]];
        let out = apply_proposal_over_base(&base, &raise(1.0), &pts);
        assert_eq!(out, vec![Some(2.0), None, None]);
    }

    #[test]
    fn relative_op_without_base_yields_none() {
        let base = |_x: f32, _z: f32| None;
        let out = apply_proposal_over_base(&base, &raise(3.0), &[[5.0, 5.0]]);
        assert_eq!(out, vec![None]);
    }

    #[test]
    fn nfr1_base_sampler_is_unmutated_after_apply() {
        // Backing grid owned here; the closure borrows it immutably, so apply
        // cannot mutate it. Assert byte-identity before/after as the NFR-1 proof.
        let grid: Vec<f32> = (0..100).map(|i| i as f32 * 0.5).collect();
        let snapshot = grid.clone();
        let base = |x: f32, z: f32| {
            let idx = (z as usize) * 10 + (x as usize);
            grid.get(idx).copied()
        };
        let pts: Vec<[f32; 2]> = (0..10)
            .flat_map(|x| (0..10).map(move |z| [x as f32, z as f32]))
            .collect();
        let _ = apply_proposal_over_base(&base, &raise(7.0), &pts);
        assert_eq!(grid, snapshot, "base heightfield mutated — NFR-1 violated");
    }

    #[test]
    fn ramp_interpolates_base_to_target_across_x() {
        let base = |_x: f32, _z: f32| Some(0.0);
        let p = TerrainProposal {
            op: TerrainOp::Ramp,
            target_height: Some(10.0),
            delta: None,
            ..raise(0.0)
        };
        // frac = x / 10 across the square (interior samples; the right/top edges
        // are excluded by the even-odd rule, so stay strictly inside).
        let out = apply_proposal_over_base(&base, &p, &[[0.0, 5.0], [5.0, 5.0], [9.0, 5.0]]);
        assert_eq!(out[0], Some(0.0));
        assert_eq!(out[1], Some(5.0));
        assert_eq!(out[2], Some(9.0));
    }

    #[test]
    fn slope_applies_constant_grade_along_run() {
        let base = |_x: f32, _z: f32| Some(1.0);
        let p = TerrainProposal {
            op: TerrainOp::Slope,
            delta: Some(0.2), // rise 0.2 per world unit of +X run
            target_height: None,
            ..raise(0.0)
        };
        let out = apply_proposal_over_base(&base, &p, &[[0.0, 5.0], [5.0, 5.0]]);
        assert_eq!(out[0], Some(1.0));
        assert_eq!(out[1], Some(2.0)); // 1.0 + 0.2*5
    }

    #[test]
    fn pad_is_fill_only_cut_is_remove_only() {
        let base = |_x: f32, _z: f32| Some(5.0);
        let pt = [[5.0, 5.0]];
        let with = |op: TerrainOp, target: f32| TerrainProposal {
            op,
            target_height: Some(target),
            delta: None,
            ..raise(0.0)
        };
        // Pad below ground keeps ground; pad above ground fills up.
        assert_eq!(
            apply_proposal_over_base(&base, &with(TerrainOp::Pad, 3.0), &pt),
            vec![Some(5.0)]
        );
        assert_eq!(
            apply_proposal_over_base(&base, &with(TerrainOp::Pad, 9.0), &pt),
            vec![Some(9.0)]
        );
        // Cut above ground keeps ground; cut below ground digs down.
        assert_eq!(
            apply_proposal_over_base(&base, &with(TerrainOp::Cut, 8.0), &pt),
            vec![Some(5.0)]
        );
        assert_eq!(
            apply_proposal_over_base(&base, &with(TerrainOp::Cut, 2.0), &pt),
            vec![Some(2.0)]
        );
    }

    #[test]
    fn json_roundtrips_exact() {
        let proposals = vec![
            TerrainProposal {
                id: "p1".into(),
                op: TerrainOp::Raise,
                footprint: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
                target_height: Some(12.5),
                delta: Some(3.0),
                material: None,
            },
            TerrainProposal {
                id: "p2".into(),
                op: TerrainOp::Flatten,
                footprint: vec![[2.0, 2.0], [3.0, 2.0], [3.0, 3.0], [2.0, 3.0]],
                target_height: Some(4.0),
                delta: None,
                material: None,
            },
        ];
        let json = to_json(&proposals);
        // op serializes snake_case per the contract.
        assert_eq!(json[0]["op"], serde_json::json!("raise"));
        assert_eq!(json[1]["op"], serde_json::json!("flatten"));
        let parsed = parse_proposals(&json);
        assert_eq!(parsed, proposals);
    }

    #[test]
    fn parse_proposals_drops_invalid_and_handles_non_array() {
        let mixed = serde_json::json!([
            { "id": "ok", "op": "raise", "footprint": [[0.0, 0.0]], "delta": 1.0 },
            { "id": "bad", "op": "not_an_op", "footprint": [] },
            { "garbage": true }
        ]);
        let parsed = parse_proposals(&mixed);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "ok");
        assert!(parse_proposals(&serde_json::json!({})).is_empty());
        assert!(parse_proposals(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn overlay_mesh_lifts_footprint_to_proposed_height() {
        let base = |_x: f32, _z: f32| Some(2.0);
        let (positions, indices) =
            proposal_overlay_mesh_data(&base, &raise(3.0)).expect("mesh built");
        assert_eq!(positions.len(), 4); // one vertex per footprint corner
        assert!(indices.len() >= 6); // at least two triangles for a quad
        for p in &positions {
            assert!((p[1] - 5.0).abs() < 1e-4, "vertex y should be base+delta");
        }
    }

    #[test]
    fn overlay_mesh_none_for_degenerate_footprint() {
        let base = |_x: f32, _z: f32| Some(0.0);
        let p = TerrainProposal {
            footprint: vec![[0.0, 0.0], [1.0, 1.0]],
            ..raise(1.0)
        };
        assert!(proposal_overlay_mesh_data(&base, &p).is_none());
    }

    #[test]
    fn report_known_square_scaled_and_unscaled() {
        // 10×10 world square, raise delta 2. At 0.1 world-units/m → 100×100 m.
        let p = raise(2.0);
        let r = proposal_report(&p, 0.1);
        assert!(r.scaled);
        assert!((r.extent_m[0] - 100.0).abs() < 1e-6);
        assert!((r.area_m2 - 10_000.0).abs() < 1e-3); // 100 m × 100 m
                                                      // volume = area(10_000 m²) × real rise(2 / 0.1 = 20 m) = 200_000 m³
        assert!((r.volume_m3 - 200_000.0).abs() < 1e-1);
        // slope is scale-invariant: 100 × 2 / 10 = 20%
        assert!((r.slope_pct - 20.0).abs() < 1e-6);

        // Unscaled: world units, scaled flag false, never fake meters.
        let ru = proposal_report(&p, f64::NAN);
        assert!(!ru.scaled);
        assert!((ru.extent_m[0] - 10.0).abs() < 1e-6); // world units
        assert!((ru.area_m2 - 100.0).abs() < 1e-6);
        // slope unchanged (scale-invariant)
        assert!((ru.slope_pct - 20.0).abs() < 1e-6);
    }

    #[test]
    fn proposal_cut_fill_raise_is_all_fill() {
        // Raise delta 2 over the flat 10×10 square at scale 1 → fill 200, cut 0
        // (FR-4: separated cut/fill for an existing proposal op, FR-6 evolve).
        let base = |_x: f32, _z: f32| Some(0.0);
        let cf = proposal_cut_fill(&base, &raise(2.0), 0.25, 1.0);
        assert!(cf.scaled);
        assert!(cf.cut_m3.abs() < 1.0, "no cut, got {}", cf.cut_m3);
        assert!((cf.fill_m3 - 200.0).abs() / 200.0 < 0.02);
    }

    #[test]
    fn proposal_cut_fill_flatten_over_relief_splits_cut_and_fill() {
        // Flatten to target 5 over a ramp base h=x on [0,10] → cut where x>5,
        // fill where x<5, both > 0 and ~symmetric.
        let base = |x: f32, _z: f32| Some(x);
        let p = TerrainProposal {
            op: TerrainOp::Flatten,
            target_height: Some(5.0),
            delta: None,
            ..raise(0.0)
        };
        let cf = proposal_cut_fill(&base, &p, 0.1, 1.0);
        assert!(cf.cut_m3 > 0.0 && cf.fill_m3 > 0.0);
        assert!((cf.cut_m3 - cf.fill_m3).abs() < 5.0);
    }

    #[test]
    fn earthwork_region_predicate_is_material_presence() {
        // The T3 JSON contract: sculpt commits always carry `material`, plain
        // proposals never do — presence IS the ghost-suppression predicate.
        let plain = raise(1.0);
        assert!(!plain.is_earthwork_region());
        let region = TerrainProposal {
            material: Some("earth".into()),
            ..raise(1.0)
        };
        assert!(region.is_earthwork_region());
        // Round-trip from the persisted JSON shapes.
        let parsed = parse_proposals(&serde_json::json!([
            { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0]], "delta": 1.0 },
            { "id": "r1", "op": "level", "footprint": [[0.0, 0.0]],
              "target_height": 2.0, "material": "gravel" }
        ]));
        assert_eq!(parsed.len(), 2, "level op must parse (config robustness)");
        assert!(!parsed[0].is_earthwork_region());
        assert!(parsed[1].is_earthwork_region());
        assert_eq!(parsed[1].op, TerrainOp::Level);
    }

    #[test]
    fn sculpt_vocab_ops_parse_and_apply() {
        // `level`/`smooth` records must not fail TerrainConfig parsing; Level is
        // absolute, Smooth is identity at this seam (regions never ghost).
        let base = |_x: f32, _z: f32| Some(5.0);
        let level = TerrainProposal {
            op: TerrainOp::Level,
            target_height: Some(2.0),
            delta: None,
            ..raise(0.0)
        };
        assert_eq!(
            apply_proposal_over_base(&base, &level, &[[5.0, 5.0]]),
            vec![Some(2.0)]
        );
        let smooth = TerrainProposal {
            op: TerrainOp::Smooth,
            ..raise(0.0)
        };
        assert_eq!(
            apply_proposal_over_base(&base, &smooth, &[[5.0, 5.0]]),
            vec![Some(5.0)]
        );
        assert_eq!(TerrainOp::Level.as_snake(), "level");
        assert_eq!(TerrainOp::Smooth.as_snake(), "smooth");
    }

    #[test]
    fn report_bearing_follows_first_edge() {
        // First edge (0,0)->(10,0) is due east in Bevy XZ (X=east) → 90°.
        let r = proposal_report(&raise(1.0), 1.0);
        assert!((r.bearing_deg - 90.0).abs() < 1e-6);
    }

    /// NFR-1 strong proof against the REAL read-only heightfield (render feature).
    #[cfg(feature = "render")]
    #[test]
    fn nfr1_real_heightfield_is_untouched_after_apply() {
        use fe_renderer::terrain_height::{HeightTile, TerrainHeightField};

        let mut field = TerrainHeightField::default();
        field.insert(
            (10, 0, 0),
            HeightTile {
                anchor: bevy::prelude::Vec3::ZERO,
                tile_size: 20.0,
                grid: vec![0.0, 4.0, 2.0, 6.0],
                width: 2,
                height: 2,
            },
        );
        let len_before = field.len();
        let probe: Vec<[f32; 2]> = (0..5)
            .flat_map(|x| (0..5).map(move |z| [x as f32 * 4.0, z as f32 * 4.0]))
            .collect();
        let before: Vec<Option<f32>> = probe.iter().map(|&[x, z]| field.height_at(x, z)).collect();

        let base = |x: f32, z: f32| field.height_at(x, z);
        let _ = apply_proposal_over_base(
            &base,
            &TerrainProposal {
                id: "p".into(),
                op: TerrainOp::Raise,
                footprint: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
                target_height: None,
                delta: Some(9.0),
                material: None,
            },
            &probe,
        );

        let after: Vec<Option<f32>> = probe.iter().map(|&[x, z]| field.height_at(x, z)).collect();
        assert_eq!(field.len(), len_before, "tileset residency changed");
        assert_eq!(before, after, "true heightfield mutated — NFR-1 violated");
    }
}
