//! FR-6 (terrain_editor_overhaul_20260718): report for the currently selected
//! terrain proposal — real-unit extent/area/volume/slope/bearing.
//! ui_shell_architecture Phase 4 folded the former floating window into
//! `ui_shell::right_sidebar::render_proposal_report_section`;
//! `render_report_body` is the pure-egui body it calls, with calm empty-state
//! hints replacing the old window's "just don't render" early returns.
//! NFR-4 (reporting honesty): when no map scale is set (`world_scale` unset
//! or `<= 0`), reports WORLD UNITS with an explicit "no map scale" chip;
//! never fabricates meters. Mirrors the `RulerPlugin`/
//! `fe_terrain::ruler_plugin::draw_unscaled_chip` precedent.
//! Totals also read Brush-created earthwork regions straight off the passed
//! terrain doc (`ui_semantics_unification_20260808` finding #5 fix) — see
//! `earthwork_regions_from_terrain`. See `panels/AGENTS.md` §terrain-tools.

use bevy_egui::egui;

use crate::geometry::{bearing_deg, polygon_area_m2, world_to_real_distance};
use crate::terrain_proposal_state::{ProposalEditState, ProposalOp};
use crate::theme;

/// How an op's representative volume classifies as earthwork (T3 FR-5): added
/// material (fill), removed material (cut), or a reshaping op whose net cut/fill
/// needs the base surface to split (reported as "net" here — the true separated
/// figure comes from `fe_terrain::sculpt::cut_fill_volume` at bake).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Earthwork {
    Fill,
    Cut,
    Net,
}

/// Classify a proposal op as fill/cut/net for the earthwork report.
fn earthwork_kind(op: ProposalOp) -> Earthwork {
    match op {
        ProposalOp::Raise | ProposalOp::Fill | ProposalOp::Pad => Earthwork::Fill,
        ProposalOp::Lower | ProposalOp::Cut => Earthwork::Cut,
        ProposalOp::Flatten | ProposalOp::Ramp | ProposalOp::Slope => Earthwork::Net,
    }
}

/// One Brush/shape earthwork region read straight out of the terrain doc's
/// `proposals` array (`actions::terrain_proposal::region_json`'s shape) —
/// distinct from `ProposalRecord`: region entries always carry a `material`
/// field `ProposalRecord` does not, which is the discriminator
/// [`earthwork_regions_from_terrain`] uses to separate "Brush region" from
/// "palette proposal" within the same array. NOT sourced via
/// `terrain_proposal_state::from_json` — `"level"`/`"smooth"` region ops
/// are not valid `ProposalOp` variants, so that parse would silently drop
/// them (finding #5, `ui_semantics_unification_20260808`).
struct EarthworkRegionSummary {
    op: String,
    footprint: Vec<[f32; 2]>,
    delta: Option<f32>,
}

/// Classify a Brush-region sculpt op tag (`actions::terrain_proposal::
/// SculptOpKind::to_snake`) as fill/cut/net, mirroring [`earthwork_kind`] for
/// palette ops. Caveat: for `"level"`/`"smooth"`, the persisted `delta` is a
/// brush strength fraction (0-1), not a height — bucketed `Net` like the
/// palette's own reshaping ops carries the same "approximate, not the true
/// baked figure" caveat already documented on [`cut_fill_totals`].
fn earthwork_kind_from_sculpt_op(op: &str) -> Earthwork {
    match op {
        "raise" => Earthwork::Fill,
        "lower" => Earthwork::Cut,
        _ => Earthwork::Net,
    }
}

/// Parse Brush/shape earthwork regions out of a petal's terrain doc (finding
/// #5: the report was structurally blind to them — Brush never touches
/// `ProposalEditState`). Best-effort skips malformed entries, mirrors
/// `terrain_proposal_state::from_json`'s lenient decode. Pure.
fn earthwork_regions_from_terrain(
    terrain_json: Option<&serde_json::Value>,
) -> Vec<EarthworkRegionSummary> {
    let Some(items) = terrain_json
        .and_then(|t| t.get("proposals"))
        .and_then(|p| p.as_array())
    else {
        return Vec::new();
    };
    items
        .iter()
        // `material` is the reliable structural marker of a region entry —
        // see `EarthworkRegionSummary`'s doc comment.
        .filter(|item| item.get("material").is_some())
        .filter_map(|item| {
            let op = item.get("op")?.as_str()?.to_string();
            let footprint: Vec<[f32; 2]> = item
                .get("footprint")?
                .as_array()?
                .iter()
                .filter_map(|p| serde_json::from_value::<[f32; 2]>(p.clone()).ok())
                .collect();
            let delta = item.get("delta").and_then(serde_json::Value::as_f64);
            Some(EarthworkRegionSummary {
                op,
                footprint,
                delta: delta.map(|d| d as f32),
            })
        })
        .collect()
}

/// Running cut/fill totals over a region set (real units when scaled).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct CutFillTotals {
    cut: f64,
    fill: f64,
    net: f64,
}

/// Sum representative cut/fill across every proposal record AND Brush-created
/// region (FR-5 "per-region and total"; finding #5 fix — regions used to be
/// invisible here). Each entry's magnitude is `area × |delta|` in real units
/// via the shared `compute_report`; the op decides the cut/fill/net bucket.
/// This is a footprint-area approximation, not the true baked volume — the
/// real figure for a Brush region lives on its node's `cut_volume_m3`/
/// `fill_volume_m3` properties (`actions::terrain_proposal::
/// persist_earthwork_volumes`), which this terrain-doc-only panel does not
/// read. Pure.
fn cut_fill_totals(
    state: &ProposalEditState,
    world_scale: f64,
    regions: &[EarthworkRegionSummary],
) -> CutFillTotals {
    let mut totals = CutFillTotals::default();
    for record in &state.proposals {
        let footprint: Vec<[f64; 3]> = record
            .footprint
            .iter()
            .map(|p| [f64::from(p[0]), 0.0, f64::from(p[1])])
            .collect();
        let delta = f64::from(record.delta.unwrap_or(0.0));
        let Some(report) = compute_report(&footprint, delta, world_scale) else {
            continue;
        };
        let v = report.volume.abs();
        match earthwork_kind(record.op) {
            Earthwork::Fill => totals.fill += v,
            Earthwork::Cut => totals.cut += v,
            Earthwork::Net => totals.net += v,
        }
    }
    for region in regions {
        let footprint: Vec<[f64; 3]> = region
            .footprint
            .iter()
            .map(|p| [f64::from(p[0]), 0.0, f64::from(p[1])])
            .collect();
        let delta = f64::from(region.delta.unwrap_or(0.0));
        let Some(report) = compute_report(&footprint, delta, world_scale) else {
            continue;
        };
        let v = report.volume.abs();
        match earthwork_kind_from_sculpt_op(&region.op) {
            Earthwork::Fill => totals.fill += v,
            Earthwork::Cut => totals.cut += v,
            Earthwork::Net => totals.net += v,
        }
    }
    totals
}

/// Section body (no window/chrome — the caller, `right_sidebar::section_chrome`
/// via `render_proposal_report_section`, supplies that). Calm empty-state
/// hints replace the old floating window's "just don't render" early returns
/// (never-blank — `ui_ux.md §7`); the <3-point guard's message is preserved
/// verbatim.
pub(crate) fn render_report_body(
    ui: &mut egui::Ui,
    proposal_state: &ProposalEditState,
    world_scale: f64,
    terrain_json: Option<&serde_json::Value>,
) {
    // Finding #5 fix: totals now cover BOTH the palette mirror AND
    // Brush-created regions parsed straight out of the terrain doc — the
    // report used to be structurally blind to everything Brush ever made.
    let regions = earthwork_regions_from_terrain(terrain_json);
    let region_count = proposal_state.proposals.len() + regions.len();

    // FR-5: earthwork totals across ALL regions (per-region detail follows for
    // the selection). Shown whenever any region exists, selected or not.
    if region_count > 0 {
        let totals = cut_fill_totals(proposal_state, world_scale, &regions);
        let has_scale = world_scale.is_finite() && world_scale > 0.0;
        let vu = if has_scale { "m\u{b3}" } else { "wu\u{b3}" };
        ui.label(
            egui::RichText::new(format!("Earthwork totals ({region_count} region(s))"))
                .strong()
                .color(theme::TEXT_SECTION),
        );
        ui.label(format!("Fill: {:.2} {vu}", totals.fill));
        ui.label(format!("Cut: {:.2} {vu}", totals.cut));
        if totals.net > 0.0 {
            ui.label(format!("Reshaped (net): {:.2} {vu}", totals.net));
        }
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
    }

    let Some(selected) = proposal_state.selected.as_deref() else {
        ui.label(
            egui::RichText::new("Select a proposal to see its metrics here.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    };
    let Some(record) = proposal_state.proposals.iter().find(|r| r.id == selected) else {
        ui.label(
            egui::RichText::new("Selected proposal no longer exists.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    };

    // Footprint is 2-D [x, z] (JSON contract); lift to [x, 0, z] for the
    // shared [f64;3] geometry helpers.
    let footprint: Vec<[f64; 3]> = record
        .footprint
        .iter()
        .map(|p| [f64::from(p[0]), 0.0, f64::from(p[1])])
        .collect();
    let report = compute_report(
        &footprint,
        f64::from(record.delta.unwrap_or(0.0)),
        world_scale,
    );

    ui.label(
        egui::RichText::new(format!("{:?}", record.op))
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);
    let Some(report) = report else {
        ui.label(
            egui::RichText::new("Footprint has fewer than 3 points — nothing to report.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    };
    ui.label(format!(
        "Extent: {:.2} {u} x {:.2} {u}",
        report.extent_x,
        report.extent_z,
        u = report.length_unit
    ));
    ui.label(format!("Area: {:.2} {}", report.area, report.area_unit));
    // FR-5: label the volume as cut/fill/net earthwork by op (the true separated
    // cut+fill over relief is `fe_terrain::sculpt::cut_fill_volume` at bake).
    let vol_label = match earthwork_kind(record.op) {
        Earthwork::Fill => "Fill volume",
        Earthwork::Cut => "Cut volume",
        Earthwork::Net => "Reshaped volume (net)",
    };
    ui.label(format!(
        "{vol_label}: {:.2} {}",
        report.volume.abs(),
        report.volume_unit
    ));
    ui.label(
        egui::RichText::new("Material: earth")
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.label(format!("Slope: {:.1}%", report.slope_pct));
    ui.label(format!("Bearing: {:.1}\u{00b0}", report.bearing_deg));
    if !report.has_scale {
        ui.add_space(4.0);
        ui.colored_label(theme::TEXT_MUTED, "no map scale — showing world units");
    }
}

/// Pure geometry report for one proposal footprint. Kept free of egui/ECS so
/// it is unit-testable without a Bevy `App`.
struct ProposalReport {
    has_scale: bool,
    extent_x: f64,
    extent_z: f64,
    length_unit: &'static str,
    area: f64,
    area_unit: &'static str,
    volume: f64,
    volume_unit: &'static str,
    slope_pct: f64,
    bearing_deg: f64,
}

/// Bounding box (min, max) of a point set on all three axes. `None` for an
/// empty slice.
fn bbox(points: &[[f64; 3]]) -> Option<([f64; 3], [f64; 3])> {
    let mut iter = points.iter();
    let first = *iter.next()?;
    let mut min = first;
    let mut max = first;
    for p in iter {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    Some((min, max))
}

/// Computes the FR-6 report fields. `world_scale <= 0` or non-finite is
/// treated as "no map scale" (NFR-4): the same shoelace/distance math runs
/// with `scale = 1.0`, which is definitionally "world units" (never
/// mislabeled as meters). Slope (%) and bearing (deg) are unit-invariant
/// ratios/angles, so they are always reported regardless of scale. `None`
/// when the footprint has fewer than 3 points (degenerate — nothing to
/// report).
fn compute_report(footprint: &[[f64; 3]], delta: f64, world_scale: f64) -> Option<ProposalReport> {
    if footprint.len() < 3 {
        return None;
    }
    let (min, max) = bbox(footprint)?;
    let has_scale = world_scale.is_finite() && world_scale > 0.0;
    let scale = if has_scale { world_scale } else { 1.0 };

    let corner_x = [max[0], min[1], min[2]];
    let corner_z = [min[0], min[1], max[2]];
    let origin = [min[0], min[1], min[2]];
    let extent_x = world_to_real_distance(origin, corner_x, scale);
    let extent_z = world_to_real_distance(origin, corner_z, scale);
    let area = polygon_area_m2(footprint, scale);
    let volume = area * (delta / scale);
    // Slope is rise/run in the SAME (raw world) unit system, so `scale`
    // cancels out algebraically — computed from the raw footprint extent,
    // not `extent_x` (which already has `scale` divided in), so it stays
    // correct and scale-invariant regardless of map-scale state.
    let raw_run = (max[0] - min[0]).abs();
    let slope_pct = if raw_run > 1e-9 {
        (delta / raw_run) * 100.0
    } else {
        0.0
    };
    let bearing = bearing_deg(origin, [max[0], min[1], max[2]]);

    let (length_unit, area_unit, volume_unit) = if has_scale {
        ("m", "m\u{b2}", "m\u{b3}")
    } else {
        ("wu", "wu\u{b2}", "wu\u{b3}")
    };

    Some(ProposalReport {
        has_scale,
        extent_x,
        extent_z,
        length_unit,
        area,
        area_unit,
        volume,
        volume_unit,
        slope_pct,
        bearing_deg: bearing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SQUARE: [[f64; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 10.0],
        [0.0, 0.0, 10.0],
    ];

    #[test]
    fn bbox_empty_is_none() {
        assert!(bbox(&[]).is_none());
    }

    #[test]
    fn bbox_finds_min_max_per_axis() {
        let (min, max) = bbox(&SQUARE).unwrap();
        assert_eq!(min, [0.0, 0.0, 0.0]);
        assert_eq!(max, [10.0, 0.0, 10.0]);
    }

    #[test]
    fn compute_report_degenerate_footprint_is_none() {
        assert!(compute_report(&[], 1.0, 1.0).is_none());
        assert!(compute_report(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], 1.0, 1.0).is_none());
    }

    #[test]
    fn compute_report_with_scale_reports_meters() {
        // 0.1 world units per meter -> a 10x10 world square is 100x100 real
        // meters (mirrors fe_terrain::ruler's polygon_area test convention).
        let report = compute_report(&SQUARE, 2.0, 0.1).unwrap();
        assert!(report.has_scale);
        assert_eq!(report.length_unit, "m");
        assert!((report.extent_x - 100.0).abs() < 1e-6);
        assert!((report.area - 10_000.0).abs() < 1e-3);
        // volume = area * (delta / scale) = 10000 * (2.0/0.1) = 200000
        assert!((report.volume - 200_000.0).abs() < 1.0);
    }

    #[test]
    fn compute_report_without_scale_falls_back_to_world_units_never_fabricates_meters() {
        for bad_scale in [0.0, -1.0, f64::NAN] {
            let report = compute_report(&SQUARE, 5.0, bad_scale).unwrap();
            assert!(!report.has_scale);
            assert_eq!(report.length_unit, "wu");
            // scale=1.0 fallback: raw world-unit numbers, not silently scaled.
            assert!((report.extent_x - 10.0).abs() < 1e-9);
            assert!((report.area - 100.0).abs() < 1e-9);
            assert!((report.volume - 500.0).abs() < 1e-9);
        }
    }

    #[test]
    fn earthwork_kind_classifies_ops() {
        assert_eq!(earthwork_kind(ProposalOp::Raise), Earthwork::Fill);
        assert_eq!(earthwork_kind(ProposalOp::Fill), Earthwork::Fill);
        assert_eq!(earthwork_kind(ProposalOp::Pad), Earthwork::Fill);
        assert_eq!(earthwork_kind(ProposalOp::Lower), Earthwork::Cut);
        assert_eq!(earthwork_kind(ProposalOp::Cut), Earthwork::Cut);
        assert_eq!(earthwork_kind(ProposalOp::Flatten), Earthwork::Net);
        assert_eq!(earthwork_kind(ProposalOp::Ramp), Earthwork::Net);
    }

    #[test]
    fn cut_fill_totals_bucket_by_op() {
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut state = ProposalEditState::default();
        // Raise delta 2 → fill; Lower delta 3 → cut (world_scale 1 → wu³).
        state.push_new(ProposalOp::Raise, square.clone(), None, Some(2.0));
        state.push_new(ProposalOp::Lower, square.clone(), None, Some(3.0));
        state.push_new(ProposalOp::Flatten, square, Some(5.0), Some(1.0));
        let t = cut_fill_totals(&state, 1.0, &[]);
        // area 100 × |delta|: fill = 100×2 = 200, cut = 100×3 = 300, net = 100×1.
        assert!((t.fill - 200.0).abs() < 1e-6);
        assert!((t.cut - 300.0).abs() < 1e-6);
        assert!((t.net - 100.0).abs() < 1e-6);
    }

    #[test]
    fn cut_fill_totals_empty_is_zero() {
        let t = cut_fill_totals(&ProposalEditState::default(), 1.0, &[]);
        assert_eq!(t, CutFillTotals::default());
    }

    #[test]
    fn compute_report_slope_and_bearing_are_scale_invariant() {
        let scaled = compute_report(&SQUARE, 5.0, 0.1).unwrap();
        let unscaled = compute_report(&SQUARE, 5.0, 1.0).unwrap();
        // 10-world-unit-wide square, delta 5.0 -> 50% slope regardless of scale.
        assert!((scaled.slope_pct - 50.0).abs() < 1e-6);
        assert!((scaled.slope_pct - unscaled.slope_pct).abs() < 1e-6);
        assert!((scaled.bearing_deg - unscaled.bearing_deg).abs() < 1e-9);
    }

    // --- Finding #5 fix: Brush-created regions in the report ---

    fn region_json(id: &str, op: &str, footprint: Vec<[f32; 2]>, delta: f32) -> serde_json::Value {
        json!({
            "id": id,
            "op": op,
            "footprint": footprint,
            "material": "earth",
            "delta": delta,
        })
    }

    #[test]
    fn earthwork_regions_from_terrain_reads_regions_and_skips_palette_entries() {
        let terrain = json!({
            "enabled": true,
            "proposals": [
                // A palette proposal (no "material") — must be skipped here.
                { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 2.0 },
                region_json("r1", "raise", vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], 3.0),
            ],
        });
        let regions = earthwork_regions_from_terrain(Some(&terrain));
        assert_eq!(regions.len(), 1, "only the material-tagged region entry");
        assert_eq!(regions[0].op, "raise");
        assert_eq!(regions[0].delta, Some(3.0));
        assert_eq!(regions[0].footprint.len(), 4);
    }

    #[test]
    fn earthwork_regions_from_terrain_handles_absent_and_malformed_docs() {
        assert!(earthwork_regions_from_terrain(None).is_empty());
        assert!(earthwork_regions_from_terrain(Some(&json!({ "enabled": true }))).is_empty());
        // A region missing its footprint is skipped, not a panic.
        let terrain =
            json!({ "proposals": [ { "id": "r1", "op": "raise", "material": "earth" } ] });
        assert!(earthwork_regions_from_terrain(Some(&terrain)).is_empty());
    }

    #[test]
    fn cut_fill_totals_includes_brush_regions_from_terrain_doc() {
        // Finding #5: totals must see Brush regions even though
        // `ProposalEditState` never carries them.
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let regions = earthwork_regions_from_terrain(Some(&json!({
            "proposals": [
                region_json("r1", "raise", square.clone(), 2.0),
                region_json("r2", "lower", square, 3.0),
            ],
        })));
        assert_eq!(regions.len(), 2);
        let t = cut_fill_totals(&ProposalEditState::default(), 1.0, &regions);
        // area 100 × |delta|: fill = 100×2 = 200 (raise), cut = 100×3 = 300 (lower).
        assert!(
            (t.fill - 200.0).abs() < 1e-6,
            "raise region counted as fill"
        );
        assert!((t.cut - 300.0).abs() < 1e-6, "lower region counted as cut");
    }

    #[test]
    fn cut_fill_totals_combines_palette_proposals_and_brush_regions() {
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut state = ProposalEditState::default();
        state.push_new(ProposalOp::Raise, square.clone(), None, Some(1.0)); // palette: fill 100
        let regions = earthwork_regions_from_terrain(Some(&json!({
            "proposals": [region_json("r1", "raise", square, 1.0)],
        }))); // brush: fill 100
        let t = cut_fill_totals(&state, 1.0, &regions);
        assert!(
            (t.fill - 200.0).abs() < 1e-6,
            "both sources contribute to one total"
        );
    }

    #[test]
    fn earthwork_kind_from_sculpt_op_classifies_ops() {
        assert_eq!(earthwork_kind_from_sculpt_op("raise"), Earthwork::Fill);
        assert_eq!(earthwork_kind_from_sculpt_op("lower"), Earthwork::Cut);
        assert_eq!(earthwork_kind_from_sculpt_op("level"), Earthwork::Net);
        assert_eq!(earthwork_kind_from_sculpt_op("smooth"), Earthwork::Net);
    }

    // --- Finding #2 fix: no double count once F1 stops hydration from
    // absorbing region entries into the palette mirror ---

    /// Mirrors `db_results::terrain::handle_petal_terrain_loaded`'s
    /// finding-#1 hydration filter (that fn is `pub(super)` to
    /// `verse_manager`, unreachable from `panels`) — palette-only entries
    /// (no `material` key) enter the mirror, exactly as production hydration
    /// now does post-fix.
    fn hydrate_like_production(terrain: &serde_json::Value) -> ProposalEditState {
        let mut state = ProposalEditState::default();
        let proposals = terrain
            .get("proposals")
            .and_then(|v| v.as_array())
            .map(|items| {
                let palette_only: Vec<serde_json::Value> = items
                    .iter()
                    .filter(|item| item.get("material").is_none())
                    .cloned()
                    .collect();
                crate::terrain_proposal_state::from_json(&serde_json::Value::Array(palette_only))
            })
            .unwrap_or_default();
        state.replace_all(proposals);
        state
    }

    #[test]
    fn cut_fill_totals_counts_a_hydrated_raise_region_exactly_once() {
        // Finding #2 regression: a raise region present in `terrain_json`
        // PLUS a hydrated mirror must count exactly once, not twice. Before
        // finding #1's hydration filter, the mirror would have absorbed the
        // region (stripping `material`) and BOTH loops in `cut_fill_totals`
        // would have summed it.
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let terrain = json!({
            "enabled": true,
            "proposals": [region_json("r1", "raise", square, 2.0)],
        });

        let state = hydrate_like_production(&terrain);
        assert!(
            state.proposals.is_empty(),
            "the region entry must never enter the palette mirror"
        );

        let regions = earthwork_regions_from_terrain(Some(&terrain));
        assert_eq!(regions.len(), 1);

        let totals = cut_fill_totals(&state, 1.0, &regions);
        // area 100 x delta 2 = 200, counted exactly once (not 400).
        assert!(
            (totals.fill - 200.0).abs() < 1e-6,
            "expected exactly one count of 200, got {}",
            totals.fill
        );
    }
}
