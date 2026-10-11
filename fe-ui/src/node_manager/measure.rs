//! Measure tool (hexon_scale FR-9): Tape (distance + bearing) and Area anchors on
//! the ground plane — a pure reducer + readout, and the viewport system that
//! places anchors and draws them. See `node_manager/AGENTS.md` §measure-tool.

use bevy::prelude::*;
use fe_renderer::terrain_height::TerrainHeightField;

use super::brush_interaction::ground_hit;
use super::router::{ClickArbiter, ClickPriority};
use crate::actions::terrain_proposal::petal_map_enabled;
use crate::geometry::{
    bearing_deg, format_area_m2, format_distance_m, polygon_area_m2, world_to_real_distance,
};
use crate::gimbal::GimbalGizmoGroup;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::terrain_map::PetalMapState;

/// Defense-in-depth cap on placed anchors (one Area polygon).
pub(crate) const MAX_MEASURE_ANCHORS: usize = 256;

/// Anchor marker radius on screen-facing gizmo circles, in world units.
const ANCHOR_MARKER_RADIUS_FRACTION: f32 = 0.01;
const ANCHOR_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const SEGMENT_COLOR: Color = Color::srgba(1.0, 0.85, 0.2, 0.9);
const CLOSING_EDGE_COLOR: Color = Color::srgba(1.0, 0.85, 0.2, 0.5);

/// What the Measure tool is measuring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MeasureMode {
    /// Two anchors: ground distance + compass bearing.
    #[default]
    Tape,
    /// A polygon of anchors: perimeter, and area once closed.
    Area,
}

impl MeasureMode {
    /// Every mode, in picker order.
    pub(crate) const ALL: [MeasureMode; 2] = [MeasureMode::Tape, MeasureMode::Area];

    /// User-facing mode label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            MeasureMode::Tape => "Tape",
            MeasureMode::Area => "Area",
        }
    }
}

/// The Measure tool's anchors (world space; Y only grounds the drawing).
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct MeasureState {
    pub(crate) mode: MeasureMode,
    pub(crate) anchors: Vec<[f32; 3]>,
    /// Area mode only: the polygon has been closed (area is reported).
    pub(crate) closed: bool,
}

impl MeasureState {
    /// Apply one event through [`reduce`] in place.
    pub(crate) fn apply(&mut self, event: MeasureEvent) {
        *self = reduce(std::mem::take(self), event);
    }

    /// Whether there is anything to clear.
    pub(crate) fn is_empty(&self) -> bool {
        self.anchors.is_empty() && !self.closed
    }
}

/// Every change to [`MeasureState`] — viewport clicks and Options buttons alike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MeasureEvent {
    Place([f32; 3]),
    Undo,
    Close,
    Clear,
    SetMode(MeasureMode),
}

/// The Measure reducer. Pure + total; see AGENTS.md §measure-tool for the rules.
pub(crate) fn reduce(mut state: MeasureState, event: MeasureEvent) -> MeasureState {
    match event {
        MeasureEvent::Place(point) => {
            if !point.iter().all(|c| c.is_finite()) {
                return state;
            }
            let restart = match state.mode {
                MeasureMode::Tape => state.anchors.len() >= 2,
                MeasureMode::Area => state.closed,
            };
            if restart {
                state.anchors.clear();
                state.closed = false;
            }
            if state.anchors.len() < MAX_MEASURE_ANCHORS {
                state.anchors.push(point);
            }
        }
        MeasureEvent::Undo => {
            if state.closed {
                state.closed = false;
            } else {
                state.anchors.pop();
            }
        }
        MeasureEvent::Close => {
            if state.mode == MeasureMode::Area && state.anchors.len() >= 3 {
                state.closed = true;
            }
        }
        MeasureEvent::Clear => {
            state.anchors.clear();
            state.closed = false;
        }
        MeasureEvent::SetMode(mode) => {
            if mode != state.mode {
                state = MeasureState {
                    mode,
                    ..Default::default()
                };
            }
        }
    }
    state
}

/// Readout numbers. Real meters when `scaled`, else raw world units (NFR-4).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct MeasureReadout {
    pub(crate) distance_m: Option<f64>,
    pub(crate) bearing_deg: Option<f64>,
    pub(crate) perimeter_m: Option<f64>,
    pub(crate) area_m2: Option<f64>,
    pub(crate) scaled: bool,
}

fn as_f64(point: [f32; 3]) -> [f64; 3] {
    [point[0] as f64, point[1] as f64, point[2] as f64]
}

/// Compute the readout for `state` at `world_scale` (world units per meter).
pub(crate) fn readout(state: &MeasureState, world_scale: f64) -> MeasureReadout {
    let scaled = world_scale.is_finite() && world_scale > 0.0;
    let mut out = MeasureReadout {
        scaled,
        ..Default::default()
    };
    let points: Vec<[f64; 3]> = state.anchors.iter().copied().map(as_f64).collect();
    match state.mode {
        MeasureMode::Tape => {
            if let [a, b] = points[..] {
                let distance = world_to_real_distance(a, b, world_scale);
                out.distance_m = Some(distance);
                out.bearing_deg = (distance > 0.0).then(|| bearing_deg(a, b));
            }
        }
        MeasureMode::Area => {
            if points.len() >= 2 {
                let mut length: f64 = points
                    .windows(2)
                    .map(|pair| world_to_real_distance(pair[0], pair[1], world_scale))
                    .sum();
                if state.closed {
                    length +=
                        world_to_real_distance(points[points.len() - 1], points[0], world_scale);
                    out.area_m2 = Some(polygon_area_m2(&points, world_scale));
                }
                out.perimeter_m = Some(length);
            }
        }
    }
    out
}

/// The scale the Measure readout uses: the active petal's map scale when an
/// enabled map is assigned, else NaN (unscaled → world units, like the scale bar).
pub(crate) fn measure_world_scale(petal_map: &PetalMapState, active_petal_id: Option<&str>) -> f64 {
    match active_petal_id {
        Some(petal_id) if petal_map_enabled(petal_map, petal_id) => petal_map.world_scale,
        _ => f64::NAN,
    }
}

fn format_length(value: f64, scaled: bool) -> String {
    if scaled {
        format_distance_m(value)
    } else {
        format!("{value:.2} world units")
    }
}

fn format_area(value: f64, scaled: bool) -> String {
    if scaled {
        format_area_m2(value)
    } else {
        format!("{value:.2} world units\u{b2}")
    }
}

/// Labelled, unit-carrying readout rows for the Options panel (copyable).
pub(crate) fn readout_rows(readout: &MeasureReadout, closed: bool) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if let Some(distance) = readout.distance_m {
        rows.push(("Distance", format_length(distance, readout.scaled)));
    }
    if let Some(bearing) = readout.bearing_deg {
        rows.push(("Bearing", format!("{bearing:.1}\u{b0}")));
    }
    if let Some(length) = readout.perimeter_m {
        let label = if closed { "Perimeter" } else { "Length so far" };
        rows.push((label, format_length(length, readout.scaled)));
    }
    if let Some(area) = readout.area_m2 {
        rows.push(("Area", format_area(area, readout.scaled)));
    }
    rows
}

/// Places anchors while Measure is active (claims `ClickPriority::Measure`);
/// clears them on tool exit or petal change. Drawing is [`draw_measure_overlay`].
pub(super) fn handle_measure_interaction(
    tool: Res<ToolState>,
    nav: Res<NavigationManager>,
    height_field: Option<Res<TerrainHeightField>>,
    mut state: ResMut<MeasureState>,
    mut arbiter: ResMut<ClickArbiter>,
    mut last_petal: Local<Option<String>>,
) {
    if *last_petal != nav.active_petal_id {
        last_petal.clone_from(&nav.active_petal_id);
        if !state.is_empty() {
            state.apply(MeasureEvent::Clear);
        }
    }
    if tool.active_tool != Tool::Measure {
        if !state.is_empty() {
            state.apply(MeasureEvent::Clear);
        }
        return;
    }
    if arbiter.is_fresh_press() {
        if let Some([x, z]) = arbiter.ray().and_then(ground_hit) {
            if arbiter.claim(ClickPriority::Measure) {
                let y = height_field
                    .as_deref()
                    .and_then(|field| field.height_at(x, z))
                    .filter(|h| h.is_finite())
                    .unwrap_or(0.0);
                state.apply(MeasureEvent::Place([x, y, z]));
            }
        }
    }
}

/// Anchor rings + segments (+ the closing edge once an Area polygon closes),
/// always on top (gimbal gizmo group). Mode-gated: nothing to draw off-tool.
pub(super) fn draw_measure_overlay(
    tool: Res<ToolState>,
    state: Res<MeasureState>,
    mut gizmos: Gizmos<GimbalGizmoGroup>,
) {
    if tool.active_tool != Tool::Measure {
        return;
    }
    let Some(first) = state.anchors.first() else {
        return;
    };
    let extent = state
        .anchors
        .iter()
        .map(|p| (p[0] - first[0]).abs().max((p[2] - first[2]).abs()))
        .fold(0.0_f32, f32::max);
    let marker_radius = (extent * ANCHOR_MARKER_RADIUS_FRACTION).max(0.05);
    let ground = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    for point in &state.anchors {
        gizmos.circle(
            Isometry3d::new(Vec3::from(*point), ground),
            marker_radius,
            ANCHOR_COLOR,
        );
    }
    for pair in state.anchors.windows(2) {
        gizmos.line(Vec3::from(pair[0]), Vec3::from(pair[1]), SEGMENT_COLOR);
    }
    if state.closed {
        if let (Some(last), Some(first)) = (state.anchors.last(), state.anchors.first()) {
            gizmos.line(Vec3::from(*last), Vec3::from(*first), CLOSING_EDGE_COLOR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(mode: MeasureMode, points: &[[f32; 3]]) -> MeasureState {
        let mut state = MeasureState {
            mode,
            ..Default::default()
        };
        for p in points {
            state.apply(MeasureEvent::Place(*p));
        }
        state
    }

    const A: [f32; 3] = [0.0, 0.0, 0.0];
    const B: [f32; 3] = [1.0, 0.0, 0.0];
    const C: [f32; 3] = [1.0, 0.0, 1.0];
    const D: [f32; 3] = [0.0, 0.0, 1.0];

    /// (start mode, placed anchors, event, expected anchors, expected closed)
    type ReducerCase = (
        MeasureMode,
        Vec<[f32; 3]>,
        MeasureEvent,
        Vec<[f32; 3]>,
        bool,
    );

    #[test]
    fn reducer_table() {
        let cases: Vec<ReducerCase> = vec![
            // Tape keeps two anchors; a third Place restarts at the new point.
            (
                MeasureMode::Tape,
                vec![A],
                MeasureEvent::Place(B),
                vec![A, B],
                false,
            ),
            (
                MeasureMode::Tape,
                vec![A, B],
                MeasureEvent::Place(C),
                vec![C],
                false,
            ),
            // Area appends.
            (
                MeasureMode::Area,
                vec![A, B],
                MeasureEvent::Place(C),
                vec![A, B, C],
                false,
            ),
            // Non-finite points are ignored.
            (
                MeasureMode::Area,
                vec![A],
                MeasureEvent::Place([f32::NAN, 0.0, 0.0]),
                vec![A],
                false,
            ),
            // Undo pops the last anchor.
            (
                MeasureMode::Area,
                vec![A, B, C],
                MeasureEvent::Undo,
                vec![A, B],
                false,
            ),
            (MeasureMode::Tape, vec![], MeasureEvent::Undo, vec![], false),
            // Close needs three anchors, and only in Area mode.
            (
                MeasureMode::Area,
                vec![A, B],
                MeasureEvent::Close,
                vec![A, B],
                false,
            ),
            (
                MeasureMode::Area,
                vec![A, B, C],
                MeasureEvent::Close,
                vec![A, B, C],
                true,
            ),
            (
                MeasureMode::Tape,
                vec![A, B],
                MeasureEvent::Close,
                vec![A, B],
                false,
            ),
            // Clear empties the anchors.
            (
                MeasureMode::Area,
                vec![A, B, C],
                MeasureEvent::Clear,
                vec![],
                false,
            ),
            // Switching mode starts over; re-selecting the same mode keeps work.
            (
                MeasureMode::Tape,
                vec![A, B],
                MeasureEvent::SetMode(MeasureMode::Area),
                vec![],
                false,
            ),
            (
                MeasureMode::Area,
                vec![A, B],
                MeasureEvent::SetMode(MeasureMode::Area),
                vec![A, B],
                false,
            ),
        ];
        for (mode, start, event, want_anchors, want_closed) in cases {
            let next = reduce(placed(mode, &start), event);
            assert_eq!(next.anchors, want_anchors, "{mode:?} {start:?} {event:?}");
            assert_eq!(next.closed, want_closed, "{mode:?} {start:?} {event:?}");
        }
    }

    #[test]
    fn closed_polygon_undo_reopens_and_place_starts_a_new_polygon() {
        let mut state = placed(MeasureMode::Area, &[A, B, C]);
        state.apply(MeasureEvent::Close);
        state.apply(MeasureEvent::Undo);
        assert!(!state.closed, "undo first reopens the polygon");
        assert_eq!(state.anchors.len(), 3, "reopening keeps every anchor");

        state.apply(MeasureEvent::Close);
        state.apply(MeasureEvent::Place(D));
        assert_eq!(state.anchors, vec![D]);
        assert!(!state.closed);
    }

    #[test]
    fn set_mode_keeps_the_new_mode() {
        let state = reduce(
            placed(MeasureMode::Tape, &[A]),
            MeasureEvent::SetMode(MeasureMode::Area),
        );
        assert_eq!(state.mode, MeasureMode::Area);
    }

    #[test]
    fn anchors_are_capped() {
        let mut state = MeasureState {
            mode: MeasureMode::Area,
            ..Default::default()
        };
        for i in 0..(MAX_MEASURE_ANCHORS + 10) {
            state.apply(MeasureEvent::Place([i as f32, 0.0, 0.0]));
        }
        assert_eq!(state.anchors.len(), MAX_MEASURE_ANCHORS);
    }

    #[test]
    fn tape_reports_real_distance_and_compass_bearing_at_both_scales() {
        // 300 m in each cardinal direction (north = -Z), at 1 and 0.01 world
        // units per meter (300 vs 3 world units apart).
        for scale in [1.0_f64, 0.01] {
            let span = (300.0 * scale) as f32;
            for (target, want_bearing) in [
                ([0.0, 0.0, -span], 0.0),
                ([span, 0.0, 0.0], 90.0),
                ([0.0, 0.0, span], 180.0),
                ([-span, 0.0, 0.0], 270.0),
            ] {
                let state = placed(MeasureMode::Tape, &[A, target]);
                let out = readout(&state, scale);
                assert!(out.scaled);
                let distance = out.distance_m.expect("two anchors");
                assert!((distance - 300.0).abs() < 1e-3, "scale {scale}: {distance}");
                let bearing = out.bearing_deg.expect("non-zero span");
                assert!(
                    (bearing - want_bearing).abs() < 1e-6,
                    "scale {scale}: {bearing}"
                );
                assert_eq!(out.area_m2, None);
            }
        }
    }

    #[test]
    fn tape_needs_two_anchors_and_a_non_zero_span_for_bearing() {
        assert_eq!(
            readout(&placed(MeasureMode::Tape, &[A]), 1.0).distance_m,
            None
        );
        let same = readout(&placed(MeasureMode::Tape, &[B, B]), 1.0);
        assert_eq!(same.distance_m, Some(0.0));
        assert_eq!(same.bearing_deg, None);
    }

    #[test]
    fn closed_square_reports_area_and_perimeter_at_both_scales() {
        // A 100 m x 100 m square = 10 000 m^2, perimeter 400 m.
        for scale in [1.0_f64, 0.01] {
            let s = (100.0 * scale) as f32;
            let mut state = placed(
                MeasureMode::Area,
                &[[0.0, 0.0, 0.0], [s, 0.0, 0.0], [s, 0.0, s], [0.0, 0.0, s]],
            );
            let open = readout(&state, scale);
            assert_eq!(open.area_m2, None, "no area until the polygon closes");
            assert!((open.perimeter_m.unwrap() - 300.0).abs() < 1e-3);

            state.apply(MeasureEvent::Close);
            let out = readout(&state, scale);
            assert!(
                (out.area_m2.unwrap() - 10_000.0).abs() < 1e-2,
                "scale {scale}"
            );
            assert!(
                (out.perimeter_m.unwrap() - 400.0).abs() < 1e-3,
                "scale {scale}"
            );
        }
    }

    #[test]
    fn unusable_scale_reports_world_units() {
        let state = placed(MeasureMode::Tape, &[A, [3.0, 0.0, 4.0]]);
        for bad in [0.0, -1.0, f64::NAN] {
            let out = readout(&state, bad);
            assert!(!out.scaled);
            assert_eq!(out.distance_m, Some(5.0));
            let rows = readout_rows(&out, false);
            assert_eq!(rows[0], ("Distance", "5.00 world units".to_string()));
        }
    }

    #[test]
    fn readout_rows_carry_units() {
        let mut state = placed(
            MeasureMode::Area,
            &[
                [0.0, 0.0, 0.0],
                [100.0, 0.0, 0.0],
                [100.0, 0.0, 100.0],
                [0.0, 0.0, 100.0],
            ],
        );
        assert_eq!(
            readout_rows(&readout(&state, 1.0), state.closed),
            vec![("Length so far", "300 m".to_string())]
        );
        state.apply(MeasureEvent::Close);
        assert_eq!(
            readout_rows(&readout(&state, 1.0), state.closed),
            vec![
                ("Perimeter", "400 m".to_string()),
                ("Area", "10000 m\u{b2}".to_string()),
            ]
        );
        let tape = placed(MeasureMode::Tape, &[A, [1500.0, 0.0, 0.0]]);
        assert_eq!(
            readout_rows(&readout(&tape, 1.0), false),
            vec![
                ("Distance", "1.5 km".to_string()),
                ("Bearing", "90.0\u{b0}".to_string()),
            ]
        );
    }

    #[test]
    fn measure_scale_is_the_map_scale_only_with_an_enabled_map() {
        let mut map = PetalMapState {
            petal_id: Some("p".into()),
            world_scale: 0.01,
            terrain_json: Some(serde_json::json!({ "enabled": true })),
            ..Default::default()
        };
        assert_eq!(measure_world_scale(&map, Some("p")), 0.01);
        assert!(measure_world_scale(&map, Some("other")).is_nan());
        assert!(measure_world_scale(&map, None).is_nan());
        map.terrain_json = Some(serde_json::json!({ "enabled": false }));
        assert!(measure_world_scale(&map, Some("p")).is_nan());
    }

    // --- the viewport flow: a click through the arbiter places an anchor ---

    /// A ray straight down onto ground point (x, z).
    fn down_at(x: f32, z: f32) -> Ray3d {
        Ray3d::new(Vec3::new(x, 50.0, z), Dir3::NEG_Y)
    }

    fn measure_app(tool: Tool, ray: Ray3d) -> App {
        let mut app = App::new();
        app.insert_resource(ToolState { active_tool: tool })
            .insert_resource(NavigationManager {
                active_petal_id: Some("petal-1".into()),
                ..Default::default()
            })
            .insert_resource(ClickArbiter::pressed_along(ray))
            .init_resource::<MeasureState>()
            .add_systems(Update, handle_measure_interaction);
        app
    }

    fn press(app: &mut App, ray: Ray3d) {
        app.insert_resource(ClickArbiter::pressed_along(ray));
        app.update();
    }

    #[test]
    fn viewport_clicks_place_tape_anchors_and_claim_the_press() {
        let mut app = measure_app(Tool::Measure, down_at(0.0, 0.0));
        app.update();
        press(&mut app, down_at(0.0, -3.0));

        let state = app.world().resource::<MeasureState>();
        assert_eq!(state.anchors, vec![[0.0, 0.0, 0.0], [0.0, 0.0, -3.0]]);
        let out = readout(state, 0.01);
        assert!((out.distance_m.unwrap() - 300.0).abs() < 1e-3);
        assert_eq!(out.bearing_deg, Some(0.0));
        assert!(
            app.world()
                .resource::<ClickArbiter>()
                .is_owner(ClickPriority::Measure),
            "the press is Measure's, so no path/node consumer sees it"
        );
    }

    #[test]
    fn other_tools_neither_place_nor_claim() {
        let mut app = measure_app(Tool::Select, down_at(1.0, 1.0));
        app.update();
        assert!(app.world().resource::<MeasureState>().anchors.is_empty());
        assert!(!app.world().resource::<ClickArbiter>().is_claimed());
    }

    #[test]
    fn leaving_the_tool_or_the_petal_clears_the_measurement() {
        let mut app = measure_app(Tool::Measure, down_at(2.0, 2.0));
        app.update();
        assert_eq!(app.world().resource::<MeasureState>().anchors.len(), 1);
        app.world_mut().resource_mut::<ToolState>().active_tool = Tool::Select;
        app.update();
        assert!(app.world().resource::<MeasureState>().anchors.is_empty());

        app.world_mut().resource_mut::<ToolState>().active_tool = Tool::Measure;
        press(&mut app, down_at(4.0, 4.0));
        assert_eq!(app.world().resource::<MeasureState>().anchors.len(), 1);
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .active_petal_id = Some("petal-2".into());
        app.insert_resource(ClickArbiter::default());
        app.update();
        assert!(
            app.world().resource::<MeasureState>().anchors.is_empty(),
            "anchors from another petal never carry over"
        );
    }
}
