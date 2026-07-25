//! FR-6 (terrain_editor_overhaul_20260718): report for the currently selected
//! terrain proposal — real-unit extent/area/volume/slope/bearing.
//! ui_shell_architecture Phase 4 folded the former floating window into
//! `ui_shell::right_sidebar::render_proposal_report_section`;
//! `render_report_body` is the pure-egui body it calls, with calm empty-state
//! hints replacing the old window's "just don't render" early returns.
//! NFR-4 (reporting honesty): when no map scale is set (`world_scale` unset
//! or `<= 0`), reports WORLD UNITS with an explicit "no map scale" chip;
//! never fabricates meters. Mirrors the `RulerPlugin`/
//! `fe_terrain::ruler_plugin::draw_unscaled_chip` precedent. See
//! `panels/AGENTS.md` §terrain-tools.

use bevy_egui::egui;

use crate::geometry::{bearing_deg, polygon_area_m2, world_to_real_distance};
use crate::terrain_proposal_state::ProposalEditState;
use crate::theme;

/// Section body (no window/chrome — the caller, `right_sidebar::section_chrome`
/// via `render_proposal_report_section`, supplies that). Calm empty-state
/// hints replace the old floating window's "just don't render" early returns
/// (never-blank — `ui_ux.md §7`); the <3-point guard's message is preserved
/// verbatim.
pub(crate) fn render_report_body(
    ui: &mut egui::Ui,
    proposal_state: &ProposalEditState,
    world_scale: f64,
) {
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
    ui.label(format!(
        "Volume: {:.2} {}",
        report.volume, report.volume_unit
    ));
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
    fn compute_report_slope_and_bearing_are_scale_invariant() {
        let scaled = compute_report(&SQUARE, 5.0, 0.1).unwrap();
        let unscaled = compute_report(&SQUARE, 5.0, 1.0).unwrap();
        // 10-world-unit-wide square, delta 5.0 -> 50% slope regardless of scale.
        assert!((scaled.slope_pct - 50.0).abs() < 1e-6);
        assert!((scaled.slope_pct - unscaled.slope_pct).abs() < 1e-6);
        assert!((scaled.bearing_deg - unscaled.bearing_deg).abs() < 1e-9);
    }
}
