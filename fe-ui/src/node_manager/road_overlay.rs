//! Road builder overlay: ghost polyline, snap guides, target ring and the
//! cursor-tethered live length readout. Pure helpers first, then the two draw
//! systems. See `fe-ui/src/node_manager/AGENTS.md` §road-chain.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use super::measure::measure_world_scale;
use super::road_chain::{decimate_into, expand_segment, pending_length, RoadBuilderState, RoadMode};
use super::road_interaction::RoadHoverState;
use super::road_snap::{Guide, SnapKind};
use crate::geometry::{format_distance_m, meters_to_world, world_to_meters};
use crate::gimbal::GimbalGizmoGroup;
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::{ToolState, ViewportRect};
use crate::terrain_map::PetalMapState;
use crate::theme;

/// Two chain points closer than this (world units, XZ) are the same point.
const ZERO_LENGTH_EPSILON: f32 = 1e-4;
/// Dashes drawn per guide line (one half-line each side of the origin).
const GUIDE_DASHES_PER_HALF: usize = 24;
/// Guide half-length floor and ceiling, world units.
const GUIDE_HALF_LENGTH_MIN: f32 = 10.0;
const GUIDE_HALF_LENGTH_MAX: f32 = 1000.0;
/// Snap-target ring radius as a fraction of the snap radius.
const TARGET_RING_FRACTION: f32 = 0.5;
/// Screen offset of the readout label from the pointer, in egui points.
const READOUT_OFFSET: egui::Vec2 = egui::vec2(16.0, 16.0);

/// Why the ghost cannot be committed right now (FR-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoadValidity {
    Valid,
    NoPetal,
    ZeroLength,
    NonFinite,
}

/// The snapped cursor when a snap resolved, else the raw ground point.
fn cursor_point(hover: &RoadHoverState) -> Option<[f32; 3]> {
    hover
        .outcome
        .as_ref()
        .map(|outcome| outcome.point)
        .or(hover.point)
}

fn is_finite_point(point: [f32; 3]) -> bool {
    point.iter().all(|value| value.is_finite())
}

/// FR-6: no active petal, a non-finite point or a zero-length segment is invalid.
pub(crate) fn classify_validity(
    state: &RoadBuilderState,
    hover: &RoadHoverState,
    active_petal: Option<&str>,
) -> RoadValidity {
    if active_petal.is_none() {
        return RoadValidity::NoPetal;
    }
    let Some(cursor) = cursor_point(hover) else {
        return RoadValidity::Valid;
    };
    if !is_finite_point(cursor) || state.anchor.is_some_and(|anchor| !is_finite_point(anchor)) {
        return RoadValidity::NonFinite;
    }
    match state.anchor {
        Some(anchor)
            if (cursor[0] - anchor[0]).abs() <= ZERO_LENGTH_EPSILON
                && (cursor[2] - anchor[2]).abs() <= ZERO_LENGTH_EPSILON =>
        {
            RoadValidity::ZeroLength
        }
        _ => RoadValidity::Valid,
    }
}

/// Fill `out` with the in-flight segment preview; empty until a chain anchor exists.
/// Straight and freeform reuse `out` (no per-frame allocation); curved samples via `bezier`.
pub(crate) fn ghost_polyline_into(
    state: &RoadBuilderState,
    hover: &RoadHoverState,
    out: &mut Vec<[f32; 3]>,
) {
    out.clear();
    let (Some(anchor), Some(cursor)) = (state.anchor, cursor_point(hover)) else {
        return;
    };
    let settings = &state.settings;
    match settings.mode {
        // Same two points `expand_segment(Straight, ..)` commits.
        RoadMode::Straight => out.extend_from_slice(&[anchor, cursor]),
        RoadMode::Curved => match state.pending.first() {
            Some(&control) => out.extend(expand_segment(
                RoadMode::Curved,
                &[anchor, control, cursor],
                settings.curve_samples,
            )),
            None => out.extend_from_slice(&[anchor, cursor]),
        },
        RoadMode::Freeform => {
            if state.freeform_samples.is_empty() {
                out.push(anchor);
            } else {
                let spacing_world = meters_to_world(settings.freeform_spacing_m, state.world_scale);
                decimate_into(&state.freeform_samples, spacing_world, out);
            }
            out.push(cursor);
        }
    }
}

/// The in-flight segment preview (allocating form of [`ghost_polyline_into`]).
pub(crate) fn ghost_polyline(state: &RoadBuilderState, hover: &RoadHoverState) -> Vec<[f32; 3]> {
    let mut points = Vec::new();
    ghost_polyline_into(state, hover, &mut points);
    points
}

/// Committed chain length, world units: the sum of consecutive planar distances.
pub(crate) fn chain_total_world(points: impl Iterator<Item = [f32; 3]>) -> f64 {
    let mut total = 0.0_f64;
    let mut previous: Option<[f32; 3]> = None;
    for point in points {
        if let Some(last) = previous {
            let length = ((point[0] - last[0]) as f64).hypot((point[2] - last[2]) as f64);
            if length.is_finite() {
                total += length;
            }
        }
        previous = Some(point);
    }
    total
}

/// Cursor label text: pending segment length in metres (or world units when the
/// petal has no map scale) plus the chain running total once one exists.
pub(crate) fn format_readout(pending_len_world: f64, chain_total_world: f64, scale: f64) -> String {
    let scaled = scale.is_finite() && scale > 0.0;
    let format_length = |world: f64| -> String {
        if scaled {
            format_distance_m(world_to_meters(world as f32, scale) as f64)
        } else {
            format!("{world:.2} world units")
        }
    };
    if chain_total_world > 0.0 {
        format!(
            "{} \u{b7} total {}",
            format_length(pending_len_world),
            format_length(chain_total_world + pending_len_world)
        )
    } else {
        format_length(pending_len_world)
    }
}

fn gizmo_color(color: egui::Color32) -> Color {
    Color::srgba_u8(color.r(), color.g(), color.b(), color.a())
}

/// Dashed infinite guide, drawn as short segments on both sides of its origin.
fn draw_guide(gizmos: &mut Gizmos<GimbalGizmoGroup>, guide: &Guide, cursor: Vec3, color: Color) {
    let origin = Vec3::from(guide.origin);
    let direction = Vec3::new(guide.dir[0], 0.0, guide.dir[1]);
    let half = (origin.distance(cursor) * 2.0).clamp(GUIDE_HALF_LENGTH_MIN, GUIDE_HALF_LENGTH_MAX);
    let step = half / GUIDE_DASHES_PER_HALF as f32;
    for side in [-1.0_f32, 1.0] {
        for index in (0..GUIDE_DASHES_PER_HALF).step_by(2) {
            let start = index as f32 * step;
            gizmos.line(
                origin + direction * (side * start),
                origin + direction * (side * (start + step)),
                color,
            );
        }
    }
}

/// Ghost polyline, snap guides, target ring and angle tick; Road tool only.
pub(super) fn draw_road_overlay(
    tool: Res<ToolState>,
    state: Res<RoadBuilderState>,
    hover: Res<RoadHoverState>,
    nav: Res<NavigationManager>,
    petal_map: Res<PetalMapState>,
    mut gizmos: Gizmos<GimbalGizmoGroup>,
    mut ghost: Local<Vec<[f32; 3]>>,
) {
    if tool.active_tool != Tool::Road {
        return;
    }
    let Some(cursor) = cursor_point(&hover) else {
        return;
    };
    let validity = classify_validity(&state, &hover, nav.active_petal_id.as_deref());
    let ghost_color = gizmo_color(if validity == RoadValidity::Valid {
        theme::TREE_PRIMARY_ACCENT
    } else {
        theme::STATUS_OFFLINE_DOT
    });
    let guide_color = gizmo_color(theme::TREE_SELECTED_ACCENT).with_alpha(0.6);

    ghost_polyline_into(&state, &hover, &mut ghost);
    gizmos.linestrip(ghost.iter().map(|point| Vec3::from(*point)), ghost_color);

    let Some(outcome) = hover.outcome.as_ref() else {
        return;
    };
    let cursor_vec = Vec3::from(cursor);
    for guide in &outcome.guides {
        draw_guide(&mut gizmos, guide, cursor_vec, guide_color);
    }
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    // Snap radius is metres; gizmos draw in world units (map scale, or 1:1 without a map).
    let world_scale = measure_world_scale(&petal_map, nav.active_petal_id.as_deref());
    let ring_radius =
        meters_to_world(state.settings.snap_radius_m * TARGET_RING_FRACTION, world_scale).max(1e-3);
    if let (SnapKind::Vertex, Some(target)) = (outcome.kind, outcome.target_vertex) {
        gizmos.circle(
            Isometry3d::new(Vec3::from(target.position), flat),
            ring_radius,
            ghost_color,
        );
    }
    if let (SnapKind::Angle, Some(anchor)) = (outcome.kind, state.anchor) {
        // Tick: a short crossbar at the cursor end, perpendicular to the ghost direction.
        let along = (cursor_vec - Vec3::from(anchor)).with_y(0.0).normalize_or_zero();
        let across = Vec3::new(-along.z, 0.0, along.x) * ring_radius;
        gizmos.line(cursor_vec - across, cursor_vec + across, ghost_color);
    }
}

/// Cursor-tethered length label (and angle when locked); Road tool only.
pub(crate) fn draw_road_readout(
    mut contexts: EguiContexts,
    tool: Res<ToolState>,
    state: Res<RoadBuilderState>,
    hover: Res<RoadHoverState>,
    petal_map: Res<PetalMapState>,
    nav: Res<NavigationManager>,
    path_state: Res<PathEditorState>,
    viewport_rect: Res<ViewportRect>,
    mut ghost: Local<Vec<[f32; 3]>>,
) {
    if tool.active_tool != Tool::Road || state.anchor.is_none() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Some(pointer) = ctx.input(|input| input.pointer.latest_pos()) else {
        return;
    };
    if !viewport_rect.0.contains(pointer) {
        return;
    }
    let active_petal = nav.active_petal_id.as_deref();
    let validity = classify_validity(&state, &hover, active_petal);
    ghost_polyline_into(&state, &hover, &mut ghost);
    let scale = measure_world_scale(&petal_map, active_petal);
    let session_is_open = state.session_track.is_some()
        && state.session_track == path_state.editing_track_id;
    let chain_total = if session_is_open {
        chain_total_world(path_state.points.iter().map(|row| row.position))
    } else {
        0.0
    };
    let mut label = format_readout(pending_length(&ghost), chain_total, scale);
    if let Some(angle) = hover
        .outcome
        .as_ref()
        .filter(|outcome| outcome.kind == SnapKind::Angle)
        .and_then(|outcome| outcome.angle_deg)
    {
        label.push_str(&format!(" \u{b7} {angle:.0}\u{b0}"));
    }
    let color = if validity == RoadValidity::Valid {
        theme::TEXT_BRIGHT
    } else {
        theme::STATUS_OFFLINE_DOT
    };
    egui::Area::new(egui::Id::new("road_readout"))
        .fixed_pos(pointer + READOUT_OFFSET)
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::BG_CONTEXT_MENU)
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(label).color(color));
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_manager::road_chain::decimate;
    use crate::node_manager::road_snap::SnapOutcome;

    fn at(x: f32, z: f32) -> [f32; 3] {
        [x, 0.0, z]
    }

    fn hover_at(point: [f32; 3]) -> RoadHoverState {
        RoadHoverState {
            point: Some(point),
            outcome: None,
        }
    }

    fn state_with(mode: RoadMode, anchor: Option<[f32; 3]>) -> RoadBuilderState {
        let mut state = RoadBuilderState::default();
        state.settings.mode = mode;
        state.anchor = anchor;
        state
    }

    #[test]
    fn classify_validity_table() {
        let anchored = state_with(RoadMode::Straight, Some(at(0.0, 0.0)));
        let cases: Vec<(&str, RoadValidity)> = vec![
            ("no petal", RoadValidity::NoPetal),
            ("zero", RoadValidity::ZeroLength),
            ("nan", RoadValidity::NonFinite),
            ("ok", RoadValidity::Valid),
        ];
        for (name, expected) in cases {
            let (petal, point) = match name {
                "no petal" => (None, at(5.0, 5.0)),
                "zero" => (Some("p"), at(0.0, 0.0)),
                "nan" => (Some("p"), at(f32::NAN, 1.0)),
                _ => (Some("p"), at(5.0, 5.0)),
            };
            assert_eq!(
                classify_validity(&anchored, &hover_at(point), petal),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn ghost_polyline_follows_each_mode() {
        let hover = hover_at(at(10.0, 0.0));

        let straight = state_with(RoadMode::Straight, Some(at(0.0, 0.0)));
        assert_eq!(ghost_polyline(&straight, &hover), vec![at(0.0, 0.0), at(10.0, 0.0)]);

        let mut curved = state_with(RoadMode::Curved, Some(at(0.0, 0.0)));
        assert_eq!(ghost_polyline(&curved, &hover).len(), 2, "line before the control click");
        curved.pending.push(at(5.0, 5.0));
        let expected = expand_segment(
            RoadMode::Curved,
            &[at(0.0, 0.0), at(5.0, 5.0), at(10.0, 0.0)],
            curved.settings.curve_samples,
        );
        assert_eq!(ghost_polyline(&curved, &hover), expected);

        let mut freeform = state_with(RoadMode::Freeform, Some(at(0.0, 0.0)));
        freeform.freeform_samples = vec![at(0.0, 0.0), at(5.0, 0.0), at(5.5, 0.0)];
        let trail = ghost_polyline(&freeform, &hover);
        assert_eq!(trail.last(), Some(&at(10.0, 0.0)));
        assert!(trail.len() >= 3);

        // 1:100 petal: ghost decimates at the same 0.02 world spacing as commit (plus the cursor).
        freeform.world_scale = 0.01;
        freeform.freeform_samples = (0..=40).map(|step| at(step as f32 * 0.005, 0.0)).collect();
        let committed_count = decimate(&freeform.freeform_samples, 0.02).len();
        assert_eq!(ghost_polyline(&freeform, &hover).len(), committed_count + 1);

        let idle = state_with(RoadMode::Straight, None);
        assert!(ghost_polyline(&idle, &hover).is_empty());
    }

    #[test]
    fn snapped_outcome_point_wins_over_the_raw_cursor() {
        let hover = RoadHoverState {
            point: Some(at(9.0, 0.0)),
            outcome: Some(SnapOutcome {
                point: at(10.0, 0.0),
                kind: SnapKind::Raw,
                target_vertex: None,
                guides: Vec::new(),
                angle_deg: None,
            }),
        };
        let state = state_with(RoadMode::Straight, Some(at(0.0, 0.0)));
        assert_eq!(ghost_polyline(&state, &hover).last(), Some(&at(10.0, 0.0)));
    }

    #[test]
    fn format_readout_scales_to_meters_km_and_degrades_to_world_units() {
        assert_eq!(format_readout(500.0, 0.0, 1.0), "500 m");
        assert_eq!(format_readout(1.5, 0.0, 0.001), "1.5 km");
        assert_eq!(format_readout(2.0, 0.0, f64::NAN), "2.00 world units");
        assert_eq!(
            format_readout(100.0, 400.0, 1.0),
            "100 m \u{b7} total 500 m"
        );
    }

    #[test]
    fn chain_total_sums_planar_segments() {
        let total = chain_total_world([at(0.0, 0.0), at(3.0, 4.0), at(3.0, 10.0)].into_iter());
        assert!((total - 11.0).abs() < 1e-9);
    }
}
