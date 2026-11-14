//! Active-tool options dispatcher; see panels/AGENTS.md section tool-options.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::dimension_callout;
use crate::actions::terrain_proposal::{SculptOpKind, SculptShapeMode, SculptToolState};
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::node_manager::measure::{
    readout, readout_rows, MeasureEvent, MeasureMode, MeasureState,
};
use crate::node_manager::road_chain::{reduce, RoadBuilderState, RoadEvent, RoadMode, RoadSettings};
use crate::node_manager::road_overlay::{chain_total_world, format_readout};
use crate::node_manager::SelectionKind;
use crate::panels::tool_inspector::panel_descriptor;
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::toolbar::Tool;
use crate::panels::{path_editor_card, terrain_tools_panel, tool_panel};
use crate::theme;
use crate::ui_shell::right_sidebar::RightSidebarSection;
use crate::verse_manager::VerseManager;

/// Which body [`render`] paints for a tool. Pure classification, extracted so
/// the tool→arm mapping is unit-testable without an egui context (the render
/// arms themselves are thin wrappers over already-tested helpers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolOptionsArm {
    /// Select — what is picked, plus the selection-filter placeholders.
    Selection,
    /// Move/Rotate/Scale — the gimbal affordance plus snap/axis-lock placeholders.
    Transform,
    /// Pen — curve/anchor/shape controls, path-asset stamping, corner editor.
    Pen,
    /// Brush — sculpt radius/strength/op/material + shape mode + cross-links.
    Brush,
    /// Measure — Tape/Area mode, live readout, Undo/Clear/Close polygon.
    Measure,
    /// Road — controls filled by the road builder options slice.
    Road,
}

/// Map the active tool to its options arm. Pure + total.
pub(crate) fn options_arm(tool: Tool) -> ToolOptionsArm {
    match tool {
        Tool::Select => ToolOptionsArm::Selection,
        Tool::Move | Tool::Rotate | Tool::Scale => ToolOptionsArm::Transform,
        Tool::Pen => ToolOptionsArm::Pen,
        Tool::Brush => ToolOptionsArm::Brush,
        Tool::Measure => ToolOptionsArm::Measure,
        Tool::Road => ToolOptionsArm::Road,
    }
}

/// Whether an arm's body is real, live controls (`true`) or the calm
/// "(soon)" placeholder list from `panel_descriptor().options_hints` (`false`).
/// Pure; keeps the descriptor hints and the rendered bodies from drifting into
/// double-documenting the same control (finding #15's failure mode).
pub(crate) fn arm_has_live_controls(arm: ToolOptionsArm) -> bool {
    matches!(
        arm,
        ToolOptionsArm::Transform
            | ToolOptionsArm::Pen
            | ToolOptionsArm::Brush
            | ToolOptionsArm::Measure
            | ToolOptionsArm::Road
    )
}

/// Render the active tool's controls through their shared state.
pub(crate) fn render(
    ui: &mut egui::Ui,
    active_tool: Tool,
    selection: &SelectionKind,
    tool_panel_state: &mut ToolPanelState,
    sculpt_state: &mut SculptToolState,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    verse_mgr: &VerseManager,
    active_petal_id: Option<&str>,
    measure: &mut MeasureState,
    measure_world_scale: f64,
    road: &mut RoadBuilderState,
    snap: &mut crate::node_manager::ToolSnapSettings,
    authoring: &mut crate::node_manager::TerrainAuthoringState,
    nav: &crate::navigation_manager::NavigationManager,
    map: &crate::terrain_map::PetalMapState,
    role: &crate::plugin::LocalUserRole,
) {
    let arm = options_arm(active_tool);
    match arm {
        ToolOptionsArm::Selection => render_select_options(ui, selection),
        ToolOptionsArm::Transform => crate::panels::transform_options::render(
            ui,
            active_tool,
            selection,
            snap,
            if map.loaded {
                crate::node_manager::measure::measure_world_scale(
                    map,
                    nav.active_petal_id.as_deref(),
                )
            } else {
                f64::NAN
            },
        ),
        ToolOptionsArm::Pen => render_pen_options(
            ui,
            tool_panel_state,
            path_state,
            ui_mgr,
            verse_mgr,
            active_petal_id,
            measure_world_scale,
            nav,
        ),
        ToolOptionsArm::Brush => {
            render_brush_options(ui, sculpt_state, ui_mgr, authoring, nav, map, role)
        }
        ToolOptionsArm::Measure => {
            render_measure_options(ui, measure, measure_world_scale, ui_mgr, active_petal_id)
        }
        ToolOptionsArm::Road => render_road_options(ui, road, path_state, measure_world_scale),
    }
    // Never-blank (`ui_ux.md §7`): an arm without live controls falls back to
    // the descriptor's calm "(soon)" lines — one table, no duplicated copy.
    if !arm_has_live_controls(arm) {
        render_options_hints(ui, active_tool);
    }
}

/// Calm per-tool placeholder lines, sourced from the single descriptor table so
/// the topbar tooltip and this surface can never disagree (`ui_ux.md §7`).
fn render_options_hints(ui: &mut egui::Ui, tool: Tool) {
    let hints = panel_descriptor(tool).options_hints;
    if hints.is_empty() {
        return;
    }
    ui.add_space(4.0);
    for line in hints {
        ui.label(egui::RichText::new(*line).small().color(theme::TEXT_MUTED));
    }
}

/// Select arm — the selection readout is already in the section header, so the
/// body only names what to do next; the filter placeholders come from the
/// descriptor via `render`'s fallback.
fn render_select_options(ui: &mut egui::Ui, selection: &SelectionKind) {
    ui.label(
        egui::RichText::new("SELECT")
            .small()
            .color(theme::TEXT_SECTION),
    );
    if matches!(selection, SelectionKind::Empty) {
        ui.label(
            egui::RichText::new("Click an object, path point, or segment to select it.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }
}

fn render_pen_picker(
    ui: &mut egui::Ui,
    path: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    tree: &VerseManager,
    petal_id: Option<&str>,
    world_scale: f64,
) {
    ui.label("Path in the active Petal");
    let Some(petal) = petal_id.and_then(|id| tree.find_petal(id)) else {
        ui.label("Open a Petal before choosing a path.");
        return;
    };
    if ui
        .add_enabled(!path.tracks_pending, egui::Button::new("Refresh paths"))
        .named(ui, "Refresh Pen path picker")
        .clicked()
    {
        ui_mgr.push_action(UiAction::PathQueryTracks {
            petal_id: petal.id.clone(),
        });
    }
    if path.tracks_pending {
        ui.label("Loading paths...");
    }
    for track in path
        .tracks
        .iter()
        .filter(|row| petal.nodes.iter().any(|node| node.id == row.node_id))
        .take(256)
    {
        if ui
            .add(
                egui::Button::selectable(
                    path.editing_track_id.as_deref() == Some(track.node_id.as_str()),
                    &track.name,
                )
                .truncate(),
            )
            .named(ui, format_args!("Edit Pen path {}", track.name))
            .on_hover_text(&track.node_id)
            .clicked()
        {
            ui_mgr.push_action(UiAction::PathSelectTrack {
                track_node_id: track.node_id.clone(),
            });
        }
    }
    if !path
        .tracks
        .iter()
        .any(|row| petal.nodes.iter().any(|node| node.id == row.node_id))
        && !path.tracks_pending
    {
        ui.label("No paths loaded. Refresh, or click the viewport with Pen to create one.");
    }
    if path
        .editing_track_id
        .as_deref()
        .is_some_and(|id| petal.nodes.iter().any(|node| node.id == id))
    {
        if path.points_pending {
            ui.label("Loading path points...");
            return;
        }
        let unit = if world_scale.is_finite() && world_scale > 0.0 {
            "m"
        } else {
            "world units"
        };
        for (index, point) in path.points.iter().take(256).enumerate() {
            let [x, y, z] = point
                .position
                .map(|v| crate::geometry::world_to_meters(v, world_scale));
            if ui
                .add(
                    egui::Button::selectable(
                        path.selected_point == Some(index),
                        format!("Point {}: {x:.2}, {y:.2}, {z:.2} {unit}", index + 1),
                    )
                    .truncate(),
                )
                .named(ui, format_args!("Select Pen path point {}", index + 1))
                .clicked()
            {
                path.selected_point = Some(index);
                path.selected_segment = None;
            }
        }
        if path.points.len() > 256 {
            ui.label("First 256 points shown; Data > Paths contains the complete editor.");
        }
    }
}

/// Pen controls, current-Petal path picker and anchor selection.
fn render_pen_options(
    ui: &mut egui::Ui,
    tool_panel_state: &mut ToolPanelState,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    verse_mgr: &VerseManager,
    active_petal: Option<&str>,
    world_scale: f64,
    nav: &crate::navigation_manager::NavigationManager,
) {
    render_pen_picker(ui, path_state, ui_mgr, verse_mgr, active_petal, world_scale);
    if path_state.editing_track_id.as_deref().is_some_and(|id| {
        !active_petal
            .and_then(|p| verse_mgr.find_petal(p))
            .is_some_and(|p| p.nodes.iter().any(|node| node.id == id))
    }) {
        ui.label("The edited path belongs to another Petal. Choose a current path above.");
        return;
    }
    tool_panel::render_pen_section(ui, tool_panel_state);
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    render_corner_editor(ui, path_state, ui_mgr,nav);
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    tool_panel::render_path_asset_section(ui, tool_panel_state, ui_mgr, path_state, verse_mgr);
}

/// D8: the per-anchor corner/smoothness card, relocated here from
/// `path_editor_card`'s edit view. The pure toggle/readback math and its tests
/// stay in `path_editor_card` (they are the Paths-tab authority's own rules);
/// only the CALL SITE moved, keeping the deferred-persist idiom intact —
/// live-edit the row buffer, push `PathSetAnchorCorner`/`PathSetAnchorHandles`
/// once the `points` borrow ends.
fn render_corner_editor(
    ui: &mut egui::Ui,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    nav:&crate::navigation_manager::NavigationManager,
) {
    let Some(track_id) = path_state.editing_track_id.clone() else {
        ui.label(
            egui::RichText::new(
                "Choose a path above, or click the viewport with Pen to create one.",
            )
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
        );
        return;
    };
    let Some(idx) = path_state.selected_point else {
        ui.label(
            egui::RichText::new(
                "Select a point above or in the viewport to edit its corner settings.",
            )
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
        );
        return;
    };
    let route_key=crate::undo::PointEditOrigin::confirmation_key(path_state,nav);
    let origin_id=ui.id().with(("corner-edit-origin",route_key));
    let origin=ui.data(|data|data.get_temp::<std::sync::Arc<crate::undo::PointEditOrigin>>(origin_id))
        .or_else(||crate::undo::PointEditOrigin::capture(path_state,nav).map(std::sync::Arc::new));
    let Some(origin)=origin else {return;};
    ui.data_mut(|data|data.insert_temp(origin_id,origin.clone()));
    let (corner,handles)=ui.push_id(route_key,|ui|path_editor_card::render_corner_settings(ui,&path_state.points,idx)).inner;
    if corner.is_some() || handles.is_some() {
        ui_mgr.push_action(UiAction::PathSetAnchorMetadata {track_node_id:track_id,index:idx,corner,handles,origin:(*origin).clone()});
    }
}

/// Brush arm (D6-B) — `render_brush_controls` is the ONLY copy of the sculpt
/// radius/strength/op/material widgets now (`render_sculpt_placeholder` was
/// deleted, finding #7), plus the shape-mode picker moved here from it (now
/// wired to a real Apply affordance, finding #14), plus D11 cross-links into
/// the two surfaces Brush work shows up in.
fn render_brush_options(
    ui: &mut egui::Ui,
    sculpt: &mut SculptToolState,
    ui_mgr: &mut UiManager,
    authoring: &mut crate::node_manager::TerrainAuthoringState,
    nav: &crate::navigation_manager::NavigationManager,
    map: &crate::terrain_map::PetalMapState,
    role: &crate::plugin::LocalUserRole,
) {
    ui.add_enabled_ui(!authoring.shape_armed(), |ui| {
        terrain_tools_panel::render_brush_controls(ui, sculpt)
    });
    ui.add_space(6.0);
    render_sculpt_shape_picker(ui, sculpt, authoring, nav, map, role);
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    render_brush_cross_links(ui, ui_mgr);
}

/// Current centered shape footprint in Petal-local meters.
pub(crate) fn shape_apply_footprint(sculpt: &SculptToolState) -> Vec<[f32; 2]> {
    match sculpt.shape_mode {
        SculptShapeMode::Brush => Vec::new(),
        SculptShapeMode::Circle | SculptShapeMode::Rect => crate::node_manager::shape_at(
            sculpt.shape_mode,
            sculpt.shape_center_m,
            sculpt.sanitized_radius(),
        ),
        SculptShapeMode::Polygon => sculpt.region_draft.clone(),
    }
}

/// Whether the Apply button may be pressed: a real footprint
/// (`handle_shape_region`'s own `>= 3` points floor) AND a known active
/// petal. Pure.
fn can_apply_shape_region(footprint_len: usize, active_petal_id: Option<&str>) -> bool {
    footprint_len >= 3 && active_petal_id.is_some()
}

/// Maps the armed op to `SculptShapeRegion`'s `target_height`/`delta` pair —
/// the same op-shaped branching `BrushSnapshot::from_state`
/// (`node_manager/brush_interaction.rs`) uses for the freeform stroke, minus
/// `strength` (this action carries none) and minus any meters→world
/// conversion: `SculptShapeRegion`'s footprint/heights stay petal-local
/// meters (N-1), unlike the Brush stroke's world-unit snapshot. Pure.
pub(crate) fn shape_region_op_fields(sculpt: &SculptToolState) -> (Option<f32>, Option<f32>) {
    match sculpt.op {
        SculptOpKind::Level => (Some(sculpt.target_height), None),
        SculptOpKind::Raise | SculptOpKind::Lower => (None, Some(sculpt.delta)),
        SculptOpKind::Smooth => (None, None),
    }
}

pub(crate) fn shape_region_material(sculpt: &SculptToolState) -> String {
    let material = sculpt.material.trim();
    if material.is_empty() {
        "earth".into()
    } else {
        material.into()
    }
}

/// Shape placement and polygon draft controls.
fn render_sculpt_shape_picker(
    ui: &mut egui::Ui,
    sculpt: &mut SculptToolState,
    authoring: &mut crate::node_manager::TerrainAuthoringState,
    nav: &crate::navigation_manager::NavigationManager,
    map: &crate::terrain_map::PetalMapState,
    role: &crate::plugin::LocalUserRole,
) {
    ui.label("Shape");
    ui.add_enabled_ui(!authoring.is_active(), |ui| {
        ui.horizontal_wrapped(|ui| {
            for mode in SculptShapeMode::ALL {
                if ui
                    .selectable_value(&mut sculpt.shape_mode, mode, mode.label())
                    .named(ui, format_args!("Earthwork shape: {}", mode.label()))
                    .changed()
                {
                    sculpt.region_draft.clear();
                }
            }
        })
    });
    if sculpt.shape_mode == SculptShapeMode::Brush {
        ui.label("Press and drag in the viewport, then release to commit. Escape cancels.");
        return;
    }
    let can_place = role.can_edit()
        && nav.active_verse_id.is_some()
        && nav.active_fractal_id.is_some()
        && map.loaded
        && nav
            .active_petal_id
            .as_deref()
            .is_some_and(|id| crate::actions::terrain_proposal::petal_map_enabled(map, id));
    if sculpt.shape_mode == SculptShapeMode::Polygon {
        ui.label(format!(
            "Polygon draft: {} points",
            sculpt.region_draft.len()
        ));
        ui.label("Click viewport vertices. Enter or double-click closes; Backspace removes the last vertex; Escape cancels.");
        let can_close = can_place
            && authoring.polygon_active()
            && terrain_tools_panel::sculpt_footprint_area(sculpt) > 0.000001
            && can_apply_shape_region(sculpt.region_draft.len(), nav.active_petal_id.as_deref());
        if sculpt.region_draft.len() >= 3
            && terrain_tools_panel::sculpt_footprint_area(sculpt) <= 0.000001
        {
            ui.label("Add a non-collinear vertex so the polygon has a nonzero area.");
        }
        if ui
            .add_enabled(can_close, egui::Button::new("Close polygon"))
            .named(ui, "Close earthwork polygon")
            .clicked()
        {
            authoring.request_close();
        }
    } else {
        if ui
            .add_enabled(
                can_place && !authoring.is_active(),
                egui::Button::new(format!("Place {} region", sculpt.shape_mode.label())),
            )
            .named(ui, "Arm earthwork region placement")
            .clicked()
        {
            authoring.arm_shape(nav, map, role, sculpt, Tool::Brush);
        }
        if authoring.shape_armed() {
            ui.label(
                "Click the viewport to place the captured shape. Escape or Cancel stops placement.",
            );
        }
    }
    let area = terrain_tools_panel::sculpt_footprint_area(sculpt);
    ui.label(format!("Footprint area \u{2248} {area:.1} m\u{b2}"));
    if ui
        .add_enabled(
            authoring.is_active() || !sculpt.region_draft.is_empty(),
            egui::Button::new("Cancel / clear draft"),
        )
        .named(ui, "Cancel earthwork placement or polygon draft")
        .clicked()
    {
        authoring.cancel();
        sculpt.region_draft.clear();
    }
    if !can_place {
        ui.label("Editor access and an enabled, loaded map in the active Petal are required.");
    }
}

/// D11 cross-links: the two surfaces a Brush stroke shows up in, addressed by
/// stable slug through `UiAction::RevealSection` rather than by poking
/// `RightSidebarState` from a panel body.
fn render_brush_cross_links(ui: &mut egui::Ui, ui_mgr: &mut UiManager) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Terrain Tools \u{2192}")
            .observed(ui)
            .on_hover_text("Open the proposal palette and proposal list")
            .clicked()
        {
            ui_mgr.push_action(reveal(RightSidebarSection::TerrainTools));
        }
        if ui
            .button("Volume report \u{2192}")
            .observed(ui)
            .on_hover_text("Open the cut/fill report for the current petal")
            .clicked()
        {
            ui_mgr.push_action(reveal(RightSidebarSection::ProposalReport));
        }
    });
}

/// Measure arm (hexon_scale FR-9): mode picker, live readout rows (each a
/// copyable value box), and the Undo / Clear / Close polygon buttons. Every
/// button goes through the same `measure::reduce` as viewport clicks.
///
/// HS-P6-CALLOUT: a "Save as callout" button queues
/// `UiAction::SaveMeasureAsCallout`, enabled only once
/// `dimension_callout::callout_from_measure` sees a COMPLETE measurement
/// (Tape with 2 anchors, or a closed Area) and an active petal exists —
/// nothing is re-derived once the action is queued, so a later click in the
/// viewport can't retroactively change what gets persisted.
fn render_measure_options(
    ui: &mut egui::Ui,
    measure: &mut MeasureState,
    world_scale: f64,
    ui_mgr: &mut UiManager,
    active_petal_id: Option<&str>,
) {
    ui.label(egui::RichText::new("Mode").small().color(theme::TEXT_DIM));
    ui.horizontal(|ui| {
        for mode in MeasureMode::ALL {
            if ui
                .selectable_label(measure.mode == mode, mode.label())
                .observed(ui)
                .clicked()
            {
                measure.apply(MeasureEvent::SetMode(mode));
            }
        }
    });
    ui.add_space(4.0);

    let current = readout(measure, world_scale);
    let rows = readout_rows(&current, measure.closed);
    if rows.is_empty() {
        let hint = match measure.mode {
            MeasureMode::Tape => "Click two points in the viewport.",
            MeasureMode::Area => "Click three or more points, then Close polygon.",
        };
        ui.label(
            egui::RichText::new(hint)
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }
    for (label, value) in &rows {
        crate::panels::widgets::copy_row(ui, ui_mgr, label, value);
    }
    if !current.scaled {
        ui.label(
            egui::RichText::new("no map scale — showing world units")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!measure.is_empty(), egui::Button::new("Undo"))
            .named(ui, "Undo last measurement point")
            .clicked()
        {
            measure.apply(MeasureEvent::Undo);
        }
        if ui
            .add_enabled(!measure.is_empty(), egui::Button::new("Clear"))
            .named(ui, "Clear measurement")
            .clicked()
        {
            measure.apply(MeasureEvent::Clear);
        }
        if measure.mode == MeasureMode::Area {
            let can_close = measure.anchors.len() >= 3 && !measure.closed;
            if ui
                .add_enabled(can_close, egui::Button::new("Close polygon"))
                .observed(ui)
                .clicked()
            {
                measure.apply(MeasureEvent::Close);
            }
        }
    });

    let callout = dimension_callout::callout_from_measure(measure);
    let can_save = callout.is_some() && active_petal_id.is_some();
    if ui
        .add_enabled(can_save, egui::Button::new("Save as callout"))
        .observed(ui)
        .on_hover_text("Persist this measurement as an addressable Dimension Callout node")
        .clicked()
    {
        if let (Some(geometry), Some(petal_id)) = (callout, active_petal_id) {
            let name = dimension_callout::callout_display_name(measure, world_scale);
            ui_mgr.push_action(UiAction::SaveMeasureAsCallout {
                petal_id: petal_id.to_string(),
                name,
                mode: dimension_callout::mode_str(geometry.mode).to_string(),
                anchors: geometry.anchors,
                closed: geometry.closed,
            });
        }
    }
}

/// One edit to the road builder settings, applied through the reducer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RoadSettingEdit {
    AngleSnap(bool),
    AngleIncrementDeg(f32),
    SnapToPaths(bool),
    Guidelines(bool),
    SnapRadiusM(f32),
    CurveSamples(usize),
    FreeformSpacingM(f32),
}

/// Allowed snap radius range, metres.
pub(crate) const ROAD_SNAP_RADIUS_RANGE: std::ops::RangeInclusive<f32> = 0.5..=50.0;
/// Allowed curve samples per segment.
pub(crate) const ROAD_CURVE_SAMPLES_RANGE: std::ops::RangeInclusive<usize> = 4..=64;
/// Allowed freeform sample spacing, metres.
pub(crate) const ROAD_FREEFORM_SPACING_RANGE: std::ops::RangeInclusive<f32> = 0.5..=20.0;

fn clamp_finite(value: f32, range: &std::ops::RangeInclusive<f32>, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(*range.start(), *range.end())
    } else {
        fallback
    }
}

/// Apply one clamped settings edit via `RoadEvent::SetSettings`; angle increment snaps to 45 or 90.
pub(crate) fn apply_road_setting(state: &mut RoadBuilderState, edit: RoadSettingEdit) {
    let mut settings: RoadSettings = state.settings;
    match edit {
        RoadSettingEdit::AngleSnap(on) => settings.angle_snap = on,
        RoadSettingEdit::AngleIncrementDeg(deg) => {
            settings.angle_increment_deg = if deg >= 90.0 { 90.0 } else { 45.0 };
        }
        RoadSettingEdit::SnapToPaths(on) => settings.snap_to_paths = on,
        RoadSettingEdit::Guidelines(on) => settings.guidelines = on,
        RoadSettingEdit::SnapRadiusM(value) => {
            settings.snap_radius_m =
                clamp_finite(value, &ROAD_SNAP_RADIUS_RANGE, settings.snap_radius_m);
        }
        RoadSettingEdit::CurveSamples(value) => {
            settings.curve_samples = value.clamp(
                *ROAD_CURVE_SAMPLES_RANGE.start(),
                *ROAD_CURVE_SAMPLES_RANGE.end(),
            );
        }
        RoadSettingEdit::FreeformSpacingM(value) => {
            settings.freeform_spacing_m = clamp_finite(
                value,
                &ROAD_FREEFORM_SPACING_RANGE,
                settings.freeform_spacing_m,
            );
        }
    }
    reduce(state, RoadEvent::SetSettings(settings));
}

/// Undo segment is meaningful once the road's track exists and has a removable segment;
/// the viewport system issues the queued removals.
pub(crate) fn road_can_undo(state: &RoadBuilderState) -> bool {
    state.can_undo_segment()
}

/// End road is meaningful while a segment is in flight or a chain is open.
pub(crate) fn road_can_end(state: &RoadBuilderState) -> bool {
    state.is_active() || !state.log.is_empty()
}

/// Road arm: mode picker, snap toggles, numeric tolerances, chain length, Undo / End.
/// Every change goes through `road_chain::reduce`, like viewport input.
fn render_road_options(
    ui: &mut egui::Ui,
    road: &mut RoadBuilderState,
    path_state: &PathEditorState,
    world_scale: f64,
) {
    ui.horizontal(|ui| {
        for mode in RoadMode::ALL {
            if ui
                .selectable_label(road.settings.mode == mode, mode.label())
                .named(ui, format_args!("Road mode {}", mode.label()))
                .clicked()
            {
                reduce(road, RoadEvent::SetMode(mode));
            }
        }
    });
    ui.add_space(4.0);

    let mut edit: Option<RoadSettingEdit> = None;
    let settings = road.settings;
    let mut angle_snap = settings.angle_snap;
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut angle_snap, "Angle snap")
            .named(ui, "Road angle snap")
            .changed()
        {
            edit = Some(RoadSettingEdit::AngleSnap(angle_snap));
        }
        ui.add_enabled_ui(settings.angle_snap, |ui| {
            for degrees in [45.0_f32, 90.0] {
                if ui
                    .selectable_label(
                        settings.angle_increment_deg == degrees,
                        format!("{degrees:.0}\u{b0}"),
                    )
                    .named(ui, format_args!("Angle snap increment {degrees:.0} degrees"))
                    .clicked()
                {
                    edit = Some(RoadSettingEdit::AngleIncrementDeg(degrees));
                }
            }
        });
    });
    let mut snap_to_paths = settings.snap_to_paths;
    if ui
        .checkbox(&mut snap_to_paths, "Snap to paths")
        .named(ui, "Road snap to paths")
        .changed()
    {
        edit = Some(RoadSettingEdit::SnapToPaths(snap_to_paths));
    }
    let mut guidelines = settings.guidelines;
    if ui
        .checkbox(&mut guidelines, "Guidelines")
        .named(ui, "Road guidelines")
        .changed()
    {
        edit = Some(RoadSettingEdit::Guidelines(guidelines));
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Snap radius (m)");
        let mut radius = settings.snap_radius_m;
        if ui
            .add(egui::DragValue::new(&mut radius).range(ROAD_SNAP_RADIUS_RANGE).speed(0.1))
            .named(ui, "Road snap radius in meters")
            .changed()
        {
            edit = Some(RoadSettingEdit::SnapRadiusM(radius));
        }
    });
    ui.horizontal(|ui| {
        ui.label("Curve samples");
        let mut samples = settings.curve_samples;
        if ui
            .add(egui::DragValue::new(&mut samples).range(ROAD_CURVE_SAMPLES_RANGE).speed(0.25))
            .named(ui, "Road curve samples per segment")
            .changed()
        {
            edit = Some(RoadSettingEdit::CurveSamples(samples));
        }
    });
    ui.horizontal(|ui| {
        ui.label("Freeform spacing (m)");
        let mut spacing = settings.freeform_spacing_m;
        if ui
            .add(egui::DragValue::new(&mut spacing).range(ROAD_FREEFORM_SPACING_RANGE).speed(0.1))
            .named(ui, "Road freeform sample spacing in meters")
            .changed()
        {
            edit = Some(RoadSettingEdit::FreeformSpacingM(spacing));
        }
    });
    if let Some(edit) = edit {
        apply_road_setting(road, edit);
    }

    ui.add_space(4.0);
    let session_open =
        road.session_track.is_some() && road.session_track == path_state.editing_track_id;
    let total_world = if session_open {
        chain_total_world(path_state.points.iter().map(|row| row.position))
    } else {
        0.0
    };
    ui.horizontal(|ui| {
        ui.label("Chain length");
        ui.label(format_readout(total_world, 0.0, world_scale));
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(road_can_undo(road), egui::Button::new("Undo segment"))
            .named(ui, "Undo last road segment")
            .clicked()
        {
            reduce(road, RoadEvent::Backspace);
        }
        if ui
            .add_enabled(road_can_end(road), egui::Button::new("End road"))
            .named(ui, "End road")
            .clicked()
        {
            reduce(road, RoadEvent::Escape);
        }
    });
}

/// Build a `RevealSection` action from a section, so call sites cannot mistype
/// a slug literal. Pure.
pub(crate) fn reveal(section: RightSidebarSection) -> UiAction {
    UiAction::RevealSection {
        slug: section.slug().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_tools() -> [Tool; 8] {
        [
            Tool::Select,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
            Tool::Pen,
            Tool::Brush,
            Tool::Measure,
            Tool::Road,
        ]
    }

    #[test]
    fn dispatcher_picks_one_arm_per_tool() {
        assert_eq!(options_arm(Tool::Select), ToolOptionsArm::Selection);
        for t in [Tool::Move, Tool::Rotate, Tool::Scale] {
            assert_eq!(options_arm(t), ToolOptionsArm::Transform, "{t:?}");
        }
        assert_eq!(options_arm(Tool::Pen), ToolOptionsArm::Pen);
        assert_eq!(options_arm(Tool::Brush), ToolOptionsArm::Brush);
        assert_eq!(options_arm(Tool::Measure), ToolOptionsArm::Measure);
        assert_eq!(options_arm(Tool::Road), ToolOptionsArm::Road);

        // Road settings edits clamp, snap the increment, and gate Undo/End on state.
        let mut road = RoadBuilderState::default();
        assert!(!road_can_undo(&road) && !road_can_end(&road));
        apply_road_setting(&mut road, RoadSettingEdit::SnapRadiusM(999.0));
        apply_road_setting(&mut road, RoadSettingEdit::CurveSamples(1));
        apply_road_setting(&mut road, RoadSettingEdit::FreeformSpacingM(f32::NAN));
        apply_road_setting(&mut road, RoadSettingEdit::AngleIncrementDeg(60.0));
        assert_eq!(road.settings.snap_radius_m, 50.0);
        assert_eq!(road.settings.curve_samples, 4);
        assert_eq!(road.settings.freeform_spacing_m, RoadSettings::default().freeform_spacing_m);
        assert_eq!(road.settings.angle_increment_deg, 45.0);
        reduce(&mut road, RoadEvent::Click([0.0, 0.0, 0.0]));
        assert!(road_can_end(&road));
        reduce(&mut road, RoadEvent::Click([10.0, 0.0, 0.0]));
        assert!(!road_can_undo(&road), "Undo waits for the created track");
        road.session_track = Some("track-1".into());
        assert!(road_can_undo(&road));
        reduce(&mut road, RoadEvent::Backspace);
        assert_eq!(road.pending_removals, vec![1], "the panel undo queues the removal for the viewport system");
    }

    #[test]
    fn every_tool_resolves_to_a_nonblank_arm() {
        // ui_ux §7 never-blank: no tool may fall through to an empty body —
        // either the arm has live controls or the descriptor supplies hints.
        for tool in all_tools() {
            let arm = options_arm(tool);
            assert!(
                arm_has_live_controls(arm) || !panel_descriptor(tool).options_hints.is_empty(),
                "{tool:?} would render a blank Options body"
            );
        }
    }

    #[test]
    fn live_control_arms_carry_no_soon_hints() {
        // The inverse guard for finding #15: a tool whose Options body is real
        // must not ALSO carry "(soon)" placeholder copy pointing elsewhere.
        for tool in all_tools() {
            if arm_has_live_controls(options_arm(tool)) {
                assert!(
                    panel_descriptor(tool).options_hints.is_empty(),
                    "{tool:?} has live controls AND stale hint copy"
                );
            }
        }
    }

    #[test]
    fn pen_and_brush_are_the_live_arms() {
        assert!(arm_has_live_controls(ToolOptionsArm::Pen));
        assert!(arm_has_live_controls(ToolOptionsArm::Brush));
        assert!(arm_has_live_controls(ToolOptionsArm::Measure));
        assert!(arm_has_live_controls(ToolOptionsArm::Road));
        assert!(!arm_has_live_controls(ToolOptionsArm::Selection));
        assert!(arm_has_live_controls(ToolOptionsArm::Transform));
        let ctx=egui::Context::default();let mut time=0.0;
        let mut nav=crate::navigation_manager::NavigationManager::default();
        nav.navigate_to_verse("v","verse");nav.navigate_to_fractal("f","fractal");nav.navigate_to_petal("p");
        let mut path=PathEditorState::default();path.start_editing("track".into());
        path.points=vec![[0.0,0.0,0.0],[2.0,0.0,0.0],[4.0,0.0,1.0]].into_iter().map(|position|crate::gis::PathPointRow {position,..Default::default()}).collect();
        path.selected_point=Some(1);let original=path.points.clone();let mut manager=UiManager::default();
        let frame=|events:Vec<egui::Event>,time:&mut f64,path:&mut PathEditorState,manager:&mut UiManager| {
            *time+=0.05;crate::ui_shell::layout_policy::begin_frame(&ctx,true);
            let _=ctx.run(egui::RawInput {screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(700.0,500.0))),time:Some(*time),events,..Default::default()},|ctx| {
                crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                egui::CentralPanel::default().show(ctx,|ui|render_corner_editor(ui,path,manager,&nav));
            });
            crate::ui_shell::layout_policy::observations(&ctx)
        };
        let observed=frame(vec![],&mut time,&mut path,&mut manager);
        let smooth=observed.controls[1].hit.center();
        for pressed in [true,false] {frame(vec![egui::Event::PointerMoved(smooth),egui::Event::PointerButton {pos:smooth,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE}],&mut time,&mut path,&mut manager);}
        assert_eq!(path.points,original,"actual Corner widget edits a draft, never authoritative rows");
        let actions=manager.drain_actions();
        let [UiAction::PathSetAnchorMetadata {corner:Some(crate::gis::CornerKind::Smooth),handles:Some(_),origin,..}]=actions.as_slice() else {panic!("one combined checked corner/handles intent is required")};
        assert!(origin.matches(&path,&nav),"the original route accompanies the finalized UI intent");
    }

    #[test]
    fn cross_links_address_their_surface_by_stable_slug() {
        for section in [
            RightSidebarSection::TerrainTools,
            RightSidebarSection::ProposalReport,
        ] {
            let UiAction::RevealSection { slug } = reveal(section) else {
                panic!("reveal must build a RevealSection");
            };
            assert_eq!(slug, section.slug());
            assert_eq!(RightSidebarSection::from_slug(&slug), Some(section));
        }
    }

    // --- finding #14: shape-mode Apply wiring (pure parts) ---

    #[test]
    fn shape_apply_footprint_per_mode() {
        let mut sculpt = SculptToolState {
            radius: 4.0,
            shape_mode: SculptShapeMode::Brush,
            ..Default::default()
        };
        assert!(
            shape_apply_footprint(&sculpt).is_empty(),
            "Brush has no shape-region affordance"
        );

        sculpt.shape_mode = SculptShapeMode::Circle;
        let circle = shape_apply_footprint(&sculpt);
        assert_eq!(
            circle.len(),
            terrain_tools_panel::circle_footprint(4.0).len()
        );
        assert!(circle.len() >= 3);

        sculpt.shape_mode = SculptShapeMode::Rect;
        let rect = shape_apply_footprint(&sculpt);
        assert_eq!(rect.len(), 4);

        sculpt.shape_mode = SculptShapeMode::Polygon;
        sculpt.region_draft = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert_eq!(shape_apply_footprint(&sculpt), sculpt.region_draft);
    }

    #[test]
    fn shape_apply_footprint_polygon_passes_through_a_too_short_draft() {
        // The floor lives in `can_apply_shape_region`, not the footprint
        // builder itself — an under-3-point draft still round-trips as-is so
        // the caller can show an accurate "need N more points" message.
        let sculpt = SculptToolState {
            shape_mode: SculptShapeMode::Polygon,
            region_draft: vec![[0.0, 0.0], [1.0, 0.0]],
            ..Default::default()
        };
        assert_eq!(shape_apply_footprint(&sculpt), vec![[0.0, 0.0], [1.0, 0.0]]);
    }

    #[test]
    fn can_apply_shape_region_requires_a_real_footprint_and_a_petal() {
        assert!(
            !can_apply_shape_region(2, Some("p1")),
            "under the 3-point floor"
        );
        assert!(!can_apply_shape_region(3, None), "no active petal");
        assert!(can_apply_shape_region(3, Some("p1")));
        assert!(can_apply_shape_region(24, Some("p1")));
    }

    #[test]
    fn shape_region_op_fields_matches_op_kind() {
        // `SculptToolState` has no `Clone` (String + Vec fields), so build a
        // fresh instance per case rather than reusing one via `..base` moves.
        fn state_with_op(op: SculptOpKind) -> SculptToolState {
            SculptToolState {
                target_height: 7.0,
                delta: -3.0,
                op,
                ..Default::default()
            }
        }
        assert_eq!(
            shape_region_op_fields(&state_with_op(SculptOpKind::Level)),
            (Some(7.0), None)
        );
        assert_eq!(
            shape_region_op_fields(&state_with_op(SculptOpKind::Raise)),
            (None, Some(-3.0))
        );
        assert_eq!(
            shape_region_op_fields(&state_with_op(SculptOpKind::Lower)),
            (None, Some(-3.0))
        );
        assert_eq!(
            shape_region_op_fields(&state_with_op(SculptOpKind::Smooth)),
            (None, None)
        );
    }

    #[test]
    fn shape_region_material_defaults_to_earth_and_trims() {
        let mut sculpt = SculptToolState {
            material: "   ".to_string(),
            ..Default::default()
        };
        assert_eq!(shape_region_material(&sculpt), "earth");
        sculpt.material = " gravel ".to_string();
        assert_eq!(shape_region_material(&sculpt), "gravel");
    }
}
