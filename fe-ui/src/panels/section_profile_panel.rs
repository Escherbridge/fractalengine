//! Existing-vs-proposed section profile inside the Volume report; see panels/AGENTS.md §section-profile.

use bevy::prelude::*;
use bevy_egui::egui;

use fe_renderer::section_profile::{SectionProfileRequest, SectionProfileResult};
use fe_renderer::terrain_overlay::VolumeUnavailable;

use crate::accessibility::ResponseExt;
use crate::actions::UiManager;
use crate::navigation_manager::NavigationManager;
use crate::panels::volume_report_panel::{volume_unit_suffix, VolumeReportState};
use crate::theme;

const PLOT_HEIGHT: f32 = 120.0;
const COLOR_EXISTING: egui::Color32 = theme::TEXT_MUTED;
const COLOR_PROPOSED: egui::Color32 = theme::TREE_PRIMARY_ACCENT;
const COLOR_CUT: egui::Color32 = egui::Color32::from_rgba_premultiplied(84, 28, 28, 90);
const COLOR_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(24, 44, 84, 90);

/// The section's angle input, in-flight request and latest matching answer.
#[derive(Resource, Default, Debug, Clone)]
pub struct SectionProfileState {
    /// Chord direction in degrees; 0 runs along +X.
    pub angle_deg: f32,
    pub request_id: Option<String>,
    pub result: Option<SectionProfileResult>,
    petal_id: Option<String>,
    footprint: Vec<[f32; 2]>,
    sent_angle_deg: f32,
    volume_correlation: Option<String>,
    next_id: u64,
}

impl SectionProfileState {
    fn clear(&mut self) {
        self.request_id = None;
        self.result = None;
        self.petal_id = None;
        self.footprint.clear();
        self.volume_correlation = None;
    }
}

// ---------------------------------------------------------------------------
// Chord (pure)
// ---------------------------------------------------------------------------

fn polygon_centroid(footprint: &[[f32; 2]]) -> [f32; 2] {
    let mut area2 = 0.0f64;
    let mut cx = 0.0f64;
    let mut cz = 0.0f64;
    for (index, p) in footprint.iter().enumerate() {
        let q = footprint[(index + 1) % footprint.len()];
        let cross = f64::from(p[0]) * f64::from(q[1]) - f64::from(q[0]) * f64::from(p[1]);
        area2 += cross;
        cx += (f64::from(p[0]) + f64::from(q[0])) * cross;
        cz += (f64::from(p[1]) + f64::from(q[1])) * cross;
    }
    if area2.abs() < 1e-9 {
        let n = footprint.len() as f32;
        return [
            footprint.iter().map(|p| p[0]).sum::<f32>() / n,
            footprint.iter().map(|p| p[1]).sum::<f32>() / n,
        ];
    }
    [(cx / (3.0 * area2)) as f32, (cz / (3.0 * area2)) as f32]
}

/// Chord of the footprint through its centroid along `angle_deg` (outermost crossings), world XZ.
pub(crate) fn section_chord(footprint: &[[f32; 2]], angle_deg: f32) -> Option<([f32; 2], [f32; 2])> {
    if footprint.len() < 3 {
        return None;
    }
    let centroid = polygon_centroid(footprint);
    let (sin, cos) = angle_deg.to_radians().sin_cos();
    let mut t_min = f32::INFINITY;
    let mut t_max = f32::NEG_INFINITY;
    for (index, p) in footprint.iter().enumerate() {
        let q = footprint[(index + 1) % footprint.len()];
        let edge = [q[0] - p[0], q[1] - p[1]];
        let denominator = cos * edge[1] - sin * edge[0];
        if denominator.abs() < 1e-9 {
            continue;
        }
        let w = [p[0] - centroid[0], p[1] - centroid[1]];
        let t = (w[0] * edge[1] - w[1] * edge[0]) / denominator;
        let s = (w[0] * sin - w[1] * cos) / denominator;
        if (0.0..=1.0).contains(&s) {
            t_min = t_min.min(t);
            t_max = t_max.max(t);
        }
    }
    if !t_min.is_finite() || !t_max.is_finite() || t_max - t_min < 1e-6 {
        return None;
    }
    Some((
        [centroid[0] + t_min * cos, centroid[1] + t_min * sin],
        [centroid[0] + t_max * cos, centroid[1] + t_max * sin],
    ))
}

// ---------------------------------------------------------------------------
// Request / result systems
// ---------------------------------------------------------------------------

/// Requests a section whenever the footprint, angle, petal or volume answer changed.
/// Runs in `PostUpdate` after `sync_volume_report_request`.
pub fn sync_section_profile_request(
    nav: Res<NavigationManager>,
    volume: Res<VolumeReportState>,
    mut state: ResMut<SectionProfileState>,
    mut requests: MessageWriter<SectionProfileRequest>,
    mut last_petal: Local<Option<String>>,
) {
    if *last_petal != nav.active_petal_id {
        *last_petal = nav.active_petal_id.clone();
        state.clear();
        return;
    }
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        state.clear();
        return;
    };
    let footprint = volume.footprint.as_slice();
    if !volume.has_selection() || footprint.len() < 3 {
        state.clear();
        return;
    }
    let unchanged = state.petal_id.as_deref() == Some(petal_id)
        && state.footprint == footprint
        && state.sent_angle_deg == state.angle_deg
        && state.volume_correlation.as_deref() == volume.correlation_id();
    if unchanged {
        return;
    }
    let Some((a, b)) = section_chord(footprint, state.angle_deg) else {
        state.clear();
        return;
    };
    state.next_id += 1;
    let correlation_id = format!("section-profile:{}", state.next_id);
    state.request_id = Some(correlation_id.clone());
    state.result = None;
    state.petal_id = Some(petal_id.to_owned());
    state.footprint.clear();
    state.footprint.extend_from_slice(footprint);
    state.sent_angle_deg = state.angle_deg;
    state.volume_correlation = volume.correlation_id().map(str::to_owned);
    requests.write(SectionProfileRequest {
        correlation_id,
        petal_id: petal_id.to_owned(),
        a,
        b,
    });
}

/// Stores the reply matching the in-flight request; foreign correlation ids are dropped.
pub fn drain_section_profile_result(
    mut results: MessageReader<SectionProfileResult>,
    nav: Res<NavigationManager>,
    mut state: ResMut<SectionProfileState>,
) {
    for result in results.read() {
        if state.request_id.as_deref() == Some(result.correlation_id.as_str())
            && state.petal_id.as_deref() == Some(result.petal_id.as_str())
            && nav.active_petal_id.as_deref() == Some(result.petal_id.as_str())
        {
            state.result = Some(result.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Display helpers (pure)
// ---------------------------------------------------------------------------

/// Display units per world unit: meters when scaled, else 1 (from the full line, not the last surviving station).
fn display_factor(result: &SectionProfileResult) -> f32 {
    if result.scaled && result.world_length > 0.0 {
        (result.length / f64::from(result.world_length)) as f32
    } else {
        1.0
    }
}

/// Distance/base/proposed rows plus cut and fill areas as tab-separated text.
pub(crate) fn section_profile_to_tsv(result: &SectionProfileResult) -> String {
    if result.unavailable.is_some() {
        return String::new();
    }
    let unit = volume_unit_suffix(result.scaled);
    let factor = display_factor(result);
    let mut out = String::new();
    out.push_str("Section profile\n");
    out.push_str(&format!("Length\t{:.3}\t{unit}\n", result.length));
    out.push_str(&format!("Cut area\t{:.3}\t{unit}\u{b2}\n", result.cut_area));
    out.push_str(&format!("Fill area\t{:.3}\t{unit}\u{b2}\n\n", result.fill_area));
    out.push_str(&format!("Distance ({unit})\tExisting ({unit})\tProposed ({unit})\n"));
    for sample in &result.samples {
        out.push_str(&format!(
            "{:.3}\t{:.3}\t{:.3}\n",
            sample[0] * factor,
            sample[1] * factor,
            sample[2] * factor
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// egui block
// ---------------------------------------------------------------------------

/// Collapsible 'Section' block for the bottom of the Volume report body.
pub(crate) fn render_section_profile_block(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    state: &mut SectionProfileState,
    volume: &VolumeReportState,
) {
    // The volume body already shows the Maps prompt when the whole report is unavailable.
    let volume_unavailable = matches!(
        crate::panels::volume_report_panel::resolve_volume_display(volume),
        crate::panels::volume_report_panel::VolumeDisplay::Unavailable { .. }
    );
    if !volume.has_selection() || volume_unavailable {
        return;
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new(egui::RichText::new("Section").strong().color(theme::TEXT_SECTION))
        .id_salt("volume_report_section_profile")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Angle");
                let response = ui.add(
                    egui::DragValue::new(&mut state.angle_deg)
                        .speed(1.0)
                        .range(0.0..=360.0)
                        .suffix("\u{b0}"),
                );
                response.named(ui, "Section angle in degrees");
            });
            let Some(result) = state.result.as_ref() else {
                ui.label(
                    egui::RichText::new("Computing section\u{2026}")
                        .small()
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
                return;
            };
            match result.unavailable {
                // The petal has a map (the volume report answered), but no terrain under this line yet.
                Some(VolumeUnavailable::NoMap) => {
                    ui.label("No terrain under this section yet.");
                    return;
                }
                Some(VolumeUnavailable::WrongPetal) => {
                    ui.label("Waiting for this petal's map to load.");
                    return;
                }
                None => {}
            }
            if result.samples.len() < 2 {
                ui.label("Section is too short to plot.");
                return;
            }
            render_plot(ui, result);
            let unit = volume_unit_suffix(result.scaled);
            ui.label(format!("Cut: {:.2} {unit}\u{b2}", result.cut_area));
            ui.label(format!("Fill: {:.2} {unit}\u{b2}", result.fill_area));
            if !result.scaled {
                ui.colored_label(theme::TEXT_MUTED, "no map scale \u{2014} showing world units");
            }
            if ui
                .button("Copy section as TSV")
                .named(ui, "Copy section profile as TSV")
                .clicked()
            {
                ui.ctx().copy_text(section_profile_to_tsv(result));
                let now = ui.ctx().input(|i| i.time);
                ui_mgr.show_toast("Section profile copied as TSV", now);
            }
        });
}

fn render_plot(ui: &mut egui::Ui, result: &SectionProfileResult) {
    let factor = display_factor(result);
    let unit = volume_unit_suffix(result.scaled);
    let (low, high) = result
        .samples
        .iter()
        .flat_map(|s| [s[1], s[2]])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), y| (lo.min(y), hi.max(y)));
    let span = (high - low).max(1e-3);
    let end = result.samples.last().map_or(1.0, |s| s[0]).max(1e-6);

    let width = ui.available_width().max(120.0);
    let (response, painter) =
        ui.allocate_painter(egui::vec2(width, PLOT_HEIGHT), egui::Sense::hover());
    let response = response.named(ui, "Section profile plot");
    let rect = response.rect.shrink2(egui::vec2(4.0, 6.0));
    painter.rect_filled(response.rect, 2.0, theme::BG_PANEL);
    let to_screen = |distance: f32, elevation: f32| {
        egui::pos2(
            rect.left() + rect.width() * distance / end,
            rect.bottom() - rect.height() * (elevation - low) / span,
        )
    };

    // Cut/fill shading, split at crossings so each piece stays convex.
    for pair in result.samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let d0 = a[2] - a[1];
        let d1 = b[2] - b[1];
        let mut shade = |x0: f32, base0: f32, prop0: f32, x1: f32, base1: f32, prop1: f32, fill: bool| {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    to_screen(x0, base0),
                    to_screen(x1, base1),
                    to_screen(x1, prop1),
                    to_screen(x0, prop0),
                ],
                if fill { COLOR_FILL } else { COLOR_CUT },
                egui::Stroke::NONE,
            ));
        };
        if d0 * d1 >= 0.0 {
            shade(a[0], a[1], a[2], b[0], b[1], b[2], d0 + d1 > 0.0);
        } else {
            let t = d0 / (d0 - d1);
            let x = a[0] + (b[0] - a[0]) * t;
            let y = a[1] + (b[1] - a[1]) * t;
            shade(a[0], a[1], a[2], x, y, y, d0 > 0.0);
            shade(x, y, y, b[0], b[1], b[2], d1 > 0.0);
        }
    }
    let existing: Vec<egui::Pos2> = result.samples.iter().map(|s| to_screen(s[0], s[1])).collect();
    let proposed: Vec<egui::Pos2> = result.samples.iter().map(|s| to_screen(s[0], s[2])).collect();
    painter.add(egui::Shape::line(existing, egui::Stroke::new(1.5, COLOR_EXISTING)));
    painter.add(egui::Shape::line(proposed, egui::Stroke::new(1.5, COLOR_PROPOSED)));

    let font = egui::FontId::proportional(10.0);
    painter.text(
        response.rect.left_top() + egui::vec2(4.0, 1.0),
        egui::Align2::LEFT_TOP,
        format!("{:.1} {unit}", high * factor),
        font.clone(),
        theme::TEXT_DIM,
    );
    painter.text(
        response.rect.left_bottom() + egui::vec2(4.0, -1.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{:.1} {unit}", low * factor),
        font.clone(),
        theme::TEXT_DIM,
    );
    painter.text(
        response.rect.right_bottom() + egui::vec2(-4.0, -1.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{:.1} {unit}", result.length),
        font,
        theme::TEXT_DIM,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct SeenRequests(Vec<SectionProfileRequest>);

    /// Fake responder: one flat-vs-bumped sample row per world unit along the chord.
    fn fake_responder(
        mut requests: MessageReader<SectionProfileRequest>,
        mut seen: ResMut<SeenRequests>,
        mut results: MessageWriter<SectionProfileResult>,
    ) {
        for request in requests.read() {
            seen.0.push(request.clone());
            let samples = (0..=10)
                .map(|i| [i as f32, 0.0, if i % 2 == 0 { 1.0 } else { -1.0 }])
                .collect();
            results.write(SectionProfileResult {
                correlation_id: request.correlation_id.clone(),
                petal_id: request.petal_id.clone(),
                samples,
                length: 10.0,
                world_length: 10.0,
                step: 1.0,
                cut_area: 2.5,
                fill_area: 3.5,
                scaled: true,
                unavailable: None,
            });
        }
    }

    #[test]
    fn section_profile_round_trip_draws_both_curves() {
        let mut app = App::new();
        app.init_resource::<NavigationManager>()
            .init_resource::<VolumeReportState>()
            .init_resource::<SectionProfileState>()
            .init_resource::<SeenRequests>()
            .add_message::<SectionProfileRequest>()
            .add_message::<SectionProfileResult>()
            .add_systems(
                Update,
                (sync_section_profile_request, fake_responder, drain_section_profile_result).chain(),
            );
        app.world_mut().resource_mut::<NavigationManager>().active_petal_id = Some("p1".into());
        app.update(); // petal-entry baseline frame
        app.world_mut()
            .resource_mut::<VolumeReportState>()
            .seed_selection_for_test("p1", vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        app.update();

        let seen = &app.world().resource::<SeenRequests>().0;
        assert_eq!(seen.len(), 1);
        assert!((seen[0].a[0] - 0.0).abs() < 1e-3 && (seen[0].a[1] - 5.0).abs() < 1e-3);
        assert!((seen[0].b[0] - 10.0).abs() < 1e-3 && (seen[0].b[1] - 5.0).abs() < 1e-3);

        let state = app.world().resource::<SectionProfileState>();
        let result = state.result.as_ref().expect("matching reply stored");
        let tsv = section_profile_to_tsv(result);
        let rows = tsv
            .lines()
            .skip_while(|line| !line.starts_with("Distance"))
            .skip(1)
            .count();
        assert_eq!(rows, result.samples.len());
        assert!(tsv.contains("Cut area\t2.500"));

        // A new angle re-requests along the rotated chord; an unchanged frame does not.
        app.update();
        assert_eq!(app.world().resource::<SeenRequests>().0.len(), 1);
        app.world_mut().resource_mut::<SectionProfileState>().angle_deg = 90.0;
        app.update();
        let seen = &app.world().resource::<SeenRequests>().0;
        assert_eq!(seen.len(), 2);
        assert!((seen[1].a[0] - 5.0).abs() < 1e-3 && (seen[1].a[1] - 0.0).abs() < 1e-3);
        assert!((seen[1].b[1] - 10.0).abs() < 1e-3);
    }
}
