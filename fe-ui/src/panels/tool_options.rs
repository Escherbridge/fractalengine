//! ui_semantics_unification_20260808 Phase 3 (D6-B/D7/D8/D9): the ONE
//! active-tool options dispatcher. `render` has exactly one arm per
//! [`Tool`], and every arm REUSES an existing render helper — nothing here is
//! a second copy of a control that lives elsewhere (finding #7/#8). This module
//! owns no state of its own: it never holds a selection, only ever edits tool
//! PARAMETERS (`ToolPanelState`/`SculptToolState`) and the edited track's
//! anchor buffer. See `fe-ui/src/panels/AGENTS.md` §tool-options.
//!
//! Phase 6 (finding #14): the Brush arm's shape-mode picker now has a real
//! Apply affordance — `render_sculpt_shape_picker` builds a footprint
//! (`shape_apply_footprint`) and pushes `UiAction::SculptShapeRegion` with the
//! current sculpt params, gated on `can_apply_shape_region`. `render`'s new
//! `active_petal_id` parameter is display/gating-only, sourced from the
//! caller's `NavigationManager.active_petal_id`.

use bevy_egui::egui;

use crate::actions::terrain_proposal::{SculptOpKind, SculptShapeMode, SculptToolState};
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
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
}

/// Map the active tool to its options arm. Pure + total.
pub(crate) fn options_arm(tool: Tool) -> ToolOptionsArm {
    match tool {
        Tool::Select => ToolOptionsArm::Selection,
        Tool::Move | Tool::Rotate | Tool::Scale => ToolOptionsArm::Transform,
        Tool::Pen => ToolOptionsArm::Pen,
        Tool::Brush => ToolOptionsArm::Brush,
    }
}

/// Whether an arm's body is real, live controls (`true`) or the calm
/// "(soon)" placeholder list from `panel_descriptor().options_hints` (`false`).
/// Pure; keeps the descriptor hints and the rendered bodies from drifting into
/// double-documenting the same control (finding #15's failure mode).
pub(crate) fn arm_has_live_controls(arm: ToolOptionsArm) -> bool {
    matches!(arm, ToolOptionsArm::Pen | ToolOptionsArm::Brush)
}

/// The one options dispatcher, called by
/// `ui_shell::right_sidebar::render_options_section` after the selection
/// readout + separator. One arm per tool; no arm renders another arm's widgets.
///
/// `active_petal_id` (finding #14, Phase 6): the Brush arm's shape-mode Apply
/// affordance needs the active petal to build `UiAction::SculptShapeRegion`;
/// threaded from the caller's `NavigationManager.active_petal_id` (display-only
/// everywhere else in this module, mirroring how `sculpt_state`/`path_state`
/// are the only mutable inputs). `too_many_arguments` is crate-wide allowed
/// (`fe-ui/src/lib.rs:3`), matching every other wide dispatcher/section fn.
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
) {
    let arm = options_arm(active_tool);
    match arm {
        ToolOptionsArm::Selection => render_select_options(ui, selection),
        ToolOptionsArm::Transform => render_transform_options(ui, active_tool),
        ToolOptionsArm::Pen => {
            render_pen_options(ui, tool_panel_state, path_state, ui_mgr, verse_mgr)
        }
        ToolOptionsArm::Brush => render_brush_options(ui, sculpt_state, ui_mgr, active_petal_id),
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

/// Move/Rotate/Scale arm — names the gimbal gesture; snap/axis-lock arrive as
/// descriptor hints via `render`'s fallback. The live "gimbal active"
/// affordance is in the section header readout, so it is NOT repeated here.
fn render_transform_options(ui: &mut egui::Ui, tool: Tool) {
    let desc = panel_descriptor(tool);
    ui.label(
        egui::RichText::new(desc.title.to_uppercase())
            .small()
            .color(theme::TEXT_SECTION),
    );
    ui.label(
        egui::RichText::new(desc.subtitle)
            .small()
            .color(theme::TEXT_MUTED),
    );
}

/// Pen arm (D7 + D8) — the ONLY host of the pen/curve/shape controls (the
/// retired `PathTools` section), the path-asset stamp picker, and the
/// per-anchor corner editor moved out of the Data window's Paths tab. The Paths
/// tab keeps the track list, start/stop editing, and the per-point list.
fn render_pen_options(
    ui: &mut egui::Ui,
    tool_panel_state: &mut ToolPanelState,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    verse_mgr: &VerseManager,
) {
    tool_panel::render_pen_section(ui, tool_panel_state);
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    render_corner_editor(ui, path_state, ui_mgr);
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
) {
    let Some(track_id) = path_state.editing_track_id.clone() else {
        ui.label(
            egui::RichText::new("Open a path in the Data window's Paths tab to edit its anchors.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    };
    let Some(idx) = path_state.selected_point else {
        ui.label(
            egui::RichText::new("Select a path point to edit its corner settings.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    };
    let (to_corner, to_handles) =
        path_editor_card::render_corner_settings(ui, &mut path_state.points, idx);
    if let Some(corner) = to_corner {
        ui_mgr.push_action(UiAction::PathSetAnchorCorner {
            track_node_id: track_id.clone(),
            index: idx,
            corner,
        });
    }
    if let Some((handle_in, handle_out, smoothness)) = to_handles {
        ui_mgr.push_action(UiAction::PathSetAnchorHandles {
            track_node_id: track_id,
            index: idx,
            handle_in,
            handle_out,
            smoothness,
        });
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
    active_petal_id: Option<&str>,
) {
    terrain_tools_panel::render_brush_controls(ui, sculpt);
    ui.add_space(6.0);
    render_sculpt_shape_picker(ui, sculpt, active_petal_id, ui_mgr);
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    render_brush_cross_links(ui, ui_mgr);
}

/// Footprint the "Apply" button submits for the sculpt shape picker, one case
/// per [`SculptShapeMode`] (finding #14). Circle/Rect are synthesized fresh
/// from `SculptToolState.radius` via `terrain_tools_panel::circle_footprint`/
/// `rect_footprint` — the same origin-anchored, petal-local-METERS convention
/// (N-1) `SculptShapeRegion` expects, unlike the freeform Brush stroke's
/// world-unit snapshot (`brush_interaction::BrushSnapshot`). Polygon reads the
/// in-progress `region_draft` as-is. Brush has no shape-region affordance — it
/// commits through the viewport drag gesture instead — so it returns empty.
/// Pure.
fn shape_apply_footprint(sculpt: &SculptToolState) -> Vec<[f32; 2]> {
    match sculpt.shape_mode {
        SculptShapeMode::Brush => Vec::new(),
        SculptShapeMode::Circle => terrain_tools_panel::circle_footprint(sculpt.sanitized_radius()),
        SculptShapeMode::Rect => terrain_tools_panel::rect_footprint(sculpt.sanitized_radius()),
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
fn shape_region_op_fields(sculpt: &SculptToolState) -> (Option<f32>, Option<f32>) {
    match sculpt.op {
        SculptOpKind::Level => (Some(sculpt.target_height), None),
        SculptOpKind::Raise | SculptOpKind::Lower => (None, Some(sculpt.delta)),
        SculptOpKind::Smooth => (None, None),
    }
}

/// Non-empty material tag, defaulting to `"earth"` — mirrors
/// `BrushSnapshot::from_state`'s trimming rule so Circle/Rect/Polygon regions
/// and Brush strokes agree on the same default. Pure.
fn shape_region_material(sculpt: &SculptToolState) -> String {
    let trimmed = sculpt.material.trim();
    if trimmed.is_empty() {
        "earth".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Shape-mode picker + polygon-draft management + live footprint readout +
/// the Apply affordance (finding #14) that actually commits a Circle/Rect/
/// Polygon region via `UiAction::SculptShapeRegion`. Moved out of the deleted
/// `render_sculpt_placeholder` so `SculptToolState` has one host.
/// `sculpt_footprint_area` stays homed (and unit-tested) in
/// `terrain_tools_panel`.
fn render_sculpt_shape_picker(
    ui: &mut egui::Ui,
    sculpt: &mut SculptToolState,
    active_petal_id: Option<&str>,
    ui_mgr: &mut UiManager,
) {
    ui.label(egui::RichText::new("Shape").small().color(theme::TEXT_DIM));
    ui.horizontal_wrapped(|ui| {
        for mode in SculptShapeMode::ALL {
            ui.selectable_value(&mut sculpt.shape_mode, mode, mode.label());
        }
    });

    if sculpt.shape_mode == SculptShapeMode::Brush {
        ui.label(
            egui::RichText::new(
                "Brush paints directly in the viewport by dragging — no Apply needed here.",
            )
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
        );
        return;
    }

    if sculpt.shape_mode == SculptShapeMode::Polygon {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Polygon draft: {} point(s)",
                    sculpt.region_draft.len()
                ))
                .small()
                .color(theme::TEXT_DIM),
            );
            if ui.small_button("Clear").clicked() {
                sculpt.region_draft.clear();
            }
        });
    }

    ui.add_space(4.0);
    let area = terrain_tools_panel::sculpt_footprint_area(sculpt);
    ui.label(
        egui::RichText::new(format!("Footprint area \u{2248} {area:.1} m\u{b2}"))
            .small()
            .color(theme::TEXT_MUTED),
    );

    ui.add_space(4.0);
    let footprint = shape_apply_footprint(sculpt);
    let can_apply = can_apply_shape_region(footprint.len(), active_petal_id);
    let clicked = ui
        .add_enabled(
            can_apply,
            egui::Button::new(format!("Apply {} region", sculpt.shape_mode.label())),
        )
        .on_hover_text("Commits a defined-shape earthwork region — a persisted, addressable node")
        .clicked();
    if clicked {
        if let Some(petal_id) = active_petal_id {
            let (target_height, delta) = shape_region_op_fields(sculpt);
            ui_mgr.push_action(UiAction::SculptShapeRegion {
                petal_id: petal_id.to_string(),
                footprint,
                op: sculpt.op.to_snake().to_string(),
                target_height,
                delta,
                material: shape_region_material(sculpt),
            });
        }
    }
    if active_petal_id.is_none() {
        ui.label(
            egui::RichText::new("Navigate to a petal to apply this region.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    } else if sculpt.shape_mode == SculptShapeMode::Polygon && sculpt.region_draft.len() < 3 {
        ui.label(
            egui::RichText::new("Add at least 3 draft points to apply a polygon region.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }
}

/// D11 cross-links: the two surfaces a Brush stroke shows up in, addressed by
/// stable slug through `UiAction::RevealSection` rather than by poking
/// `RightSidebarState` from a panel body.
fn render_brush_cross_links(ui: &mut egui::Ui, ui_mgr: &mut UiManager) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Terrain Tools \u{2192}")
            .on_hover_text("Open the proposal palette and proposal list")
            .clicked()
        {
            ui_mgr.push_action(reveal(RightSidebarSection::TerrainTools));
        }
        if ui
            .button("Earthwork report \u{2192}")
            .on_hover_text("Open the cut/fill report for the current petal")
            .clicked()
        {
            ui_mgr.push_action(reveal(RightSidebarSection::ProposalReport));
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

    fn all_tools() -> [Tool; 6] {
        [
            Tool::Select,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
            Tool::Pen,
            Tool::Brush,
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
        assert!(!arm_has_live_controls(ToolOptionsArm::Selection));
        assert!(!arm_has_live_controls(ToolOptionsArm::Transform));
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
