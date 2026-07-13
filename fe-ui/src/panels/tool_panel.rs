//! Tools panel: a floating window (independent of `ActiveDialog`, like
//! `gis_panel`) that hosts the hexon-path-asset stamping controls. The
//! "Stamp along path" button emits `UiAction::PathAssetApply` for the track
//! currently being edited (`PathEditorState.editing_track_id`), building the
//! descriptor from the repetition/pattern controls. See
//! `fe-ui/src/panels/AGENTS.md` §tool-panel.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::node_manager::curve::{self, PenMode};
use crate::theme;

/// How repeated instances along a path are spaced. Mirrors
/// `fe_sdk::path_asset::SpacingMode`; kept as a local egui-facing enum so the
/// panel state stays free of the SDK type, converted at emit time.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum SpacingMode {
    #[default]
    FixedSpacing,
    FixedCount,
}

impl SpacingMode {
    /// Map the panel's local spacing mode to the SDK descriptor enum.
    fn to_sdk(self) -> fe_sdk::path_asset::SpacingMode {
        match self {
            SpacingMode::FixedSpacing => fe_sdk::path_asset::SpacingMode::FixedSpacing,
            SpacingMode::FixedCount => fe_sdk::path_asset::SpacingMode::FixedCount,
        }
    }
}

/// Persistent state for the Tools panel (open flag + path-asset stamp
/// controls). `selected_hexon_ref` doubles as the v1 asset-path text buffer
/// (`blob://{hash}.glb`) until a real hexon picker lands.
#[derive(Resource, Default)]
pub struct ToolPanelState {
    pub open: bool,
    // --- W4 path-asset stamp controls (owned by the gis_tool_panel track) ---
    pub selected_hexon_ref: Option<String>,
    pub spacing_mode: SpacingMode,
    pub spacing_value: f32,
    pub count_value: u32,
    pub tangent_align: bool,
    // --- W7b pen-tool phase-2 controls (curves + shapes) — see AGENTS.md ---
    /// How placed control points become the final path (Polyline/Catmull/Bezier).
    pub pen_mode: PenMode,
    /// Sharp↔smooth sensitivity for `CatmullRom` (0 = round, 1 = sharp/straight).
    pub pen_tension: f32,
    /// Samples emitted per curve segment when smoothing.
    pub pen_samples_per_segment: usize,
    /// Radius used by the "Add Circle" / X-radius of "Add Ellipse" shape.
    pub shape_radius: f32,
    /// Z-radius for "Add Ellipse" (X-radius is `shape_radius`).
    pub shape_radius_z: f32,
    /// Segment count for generated shape rings.
    pub shape_segments: usize,
    /// Pen-tool actions queued during the egui pass; drained by
    /// `actions::process_ui_actions` (avoids threading `ui_mgr` through the
    /// `render_tool_panel` signature). See AGENTS.md §tool-panel.
    pub pending_actions: Vec<UiAction>,
}

impl ToolPanelState {
    /// Queues a pen-tool `UiAction` for the drain in `process_ui_actions`.
    pub fn queue_action(&mut self, action: UiAction) {
        self.pending_actions.push(action);
    }

    /// Drains queued pen-tool actions (called by `process_ui_actions`).
    pub fn drain_pending(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.pending_actions)
    }
}

pub fn render_tool_panel(
    ctx: &egui::Context,
    state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    path_state: &PathEditorState,
) {
    if !state.open {
        return;
    }

    let mut still_open = true;
    egui::Window::new("Tools")
        .open(&mut still_open)
        .resizable(true)
        .default_width(320.0)
        .min_width(280.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            render_path_asset_section(ui, state, ui_mgr, path_state);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            render_pen_section(ui, state);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            render_terrain_tools_section(ui);
        });

    if !still_open {
        state.open = false;
    }
}

fn render_path_asset_section(
    ui: &mut egui::Ui,
    state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    path_state: &PathEditorState,
) {
    ui.label(egui::RichText::new("Path Asset").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    // v1 hexon reference: a plain asset-path field (`blob://{hash}.glb`).
    // A real hexon picker replaces this later without touching the emit path.
    let mut asset_buf = state.selected_hexon_ref.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Asset").small().color(theme::TEXT_DIM));
        if ui
            .add(egui::TextEdit::singleline(&mut asset_buf).hint_text("blob://<hash>.glb"))
            .changed()
        {
            state.selected_hexon_ref = if asset_buf.trim().is_empty() { None } else { Some(asset_buf.clone()) };
        }
    });
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.spacing_mode, SpacingMode::FixedSpacing, "Fixed spacing");
        ui.selectable_value(&mut state.spacing_mode, SpacingMode::FixedCount, "Fixed count");
    });
    ui.add_space(4.0);

    match state.spacing_mode {
        SpacingMode::FixedSpacing => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Spacing").small().color(theme::TEXT_DIM));
                ui.add(egui::DragValue::new(&mut state.spacing_value).speed(0.1).range(0.0..=f32::MAX));
            });
        }
        SpacingMode::FixedCount => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Count").small().color(theme::TEXT_DIM));
                ui.add(egui::DragValue::new(&mut state.count_value).range(0..=u32::MAX));
            });
        }
    }
    ui.add_space(4.0);

    ui.checkbox(&mut state.tangent_align, "Align to path tangent");
    ui.add_space(6.0);

    // The stamp target is the track currently being edited in the Paths tab.
    let target_track = path_state.editing_track_id.clone();
    let asset_ref = state.selected_hexon_ref.clone().unwrap_or_default();
    let can_stamp = target_track.is_some() && !asset_ref.trim().is_empty();

    let resp = ui.add_enabled(can_stamp, egui::Button::new("Stamp along path"));
    if resp.clicked() {
        if let Some(track_node_id) = target_track.clone() {
            let descriptor = build_descriptor(state, asset_ref.clone());
            ui_mgr.push_action(UiAction::PathAssetApply { track_node_id, descriptor });
        }
    }

    if target_track.is_none() {
        ui.label(
            egui::RichText::new("Select a track in the Paths tab to stamp along.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    } else if asset_ref.trim().is_empty() {
        ui.label(
            egui::RichText::new("Enter an asset reference to stamp.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }
}

/// Build the SDK path-asset descriptor from the panel's control state.
fn build_descriptor(state: &ToolPanelState, asset_path: String) -> fe_sdk::path_asset::PathAssetDescriptor {
    fe_sdk::path_asset::PathAssetDescriptor {
        asset_path,
        spacing_mode: state.spacing_mode.to_sdk(),
        spacing_value: state.spacing_value,
        count: state.count_value,
        tangent_align: state.tangent_align,
    }
}

/// Pen-tool phase-2 controls: mode radio, sensitivity (tension) slider, a
/// "Smooth path" button (resample + replace the edited track's points), and
/// shape buttons (ellipse/circle). All buttons queue a `UiAction` into
/// `state.pending_actions`, drained by `process_ui_actions`. See
/// `node_manager/AGENTS.md` §pen-tool.
fn render_pen_section(ui: &mut egui::Ui, state: &mut ToolPanelState) {
    // `ToolPanelState` is `#[derive(Default)]` (shared with W4), so the pen
    // fields start at zero. Lazily seed usable defaults on first paint.
    if state.pen_samples_per_segment == 0 {
        state.pen_samples_per_segment = 12;
    }
    if state.shape_radius <= 0.0 {
        state.shape_radius = 5.0;
    }
    if state.shape_radius_z <= 0.0 {
        state.shape_radius_z = 3.0;
    }
    if state.shape_segments < 3 {
        state.shape_segments = 24;
    }

    ui.label(egui::RichText::new("Pen").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    ui.label(egui::RichText::new("Curve mode").small().color(theme::TEXT_DIM));
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.pen_mode, PenMode::Polyline, PenMode::Polyline.label());
        ui.selectable_value(&mut state.pen_mode, PenMode::CatmullRom, PenMode::CatmullRom.label());
        ui.selectable_value(&mut state.pen_mode, PenMode::Bezier, PenMode::Bezier.label());
    });
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("Sensitivity: sharp ↔ smooth")
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.add(egui::Slider::new(&mut state.pen_tension, 0.0..=1.0).show_value(false));
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Samples/segment").small().color(theme::TEXT_DIM));
        let mut samples = state.pen_samples_per_segment as u32;
        if ui.add(egui::DragValue::new(&mut samples).range(1..=128)).changed() {
            state.pen_samples_per_segment = samples as usize;
        }
    });
    ui.add_space(4.0);

    if ui
        .add_enabled(
            state.pen_mode != PenMode::Polyline,
            egui::Button::new("Smooth path"),
        )
        .on_hover_text("Resample the edited track's points into the chosen curve")
        .clicked()
    {
        state.queue_action(UiAction::PathSmoothCurrent {
            mode: state.pen_mode,
            tension: state.pen_tension,
            samples_per_segment: state.pen_samples_per_segment.max(1),
        });
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Shapes").small().color(theme::TEXT_DIM));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Radius X").small().color(theme::TEXT_DIM));
        ui.add(egui::DragValue::new(&mut state.shape_radius).speed(0.1).range(0.01..=f32::MAX));
        ui.label(egui::RichText::new("Radius Z").small().color(theme::TEXT_DIM));
        ui.add(egui::DragValue::new(&mut state.shape_radius_z).speed(0.1).range(0.01..=f32::MAX));
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Segments").small().color(theme::TEXT_DIM));
        let mut segs = state.shape_segments.max(3) as u32;
        if ui.add(egui::DragValue::new(&mut segs).range(3..=256)).changed() {
            state.shape_segments = segs as usize;
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Add Ellipse").on_hover_text("Append an ellipse ring to the edited track").clicked() {
            let pts = curve::ellipse(
                [0.0, 0.0, 0.0],
                state.shape_radius,
                state.shape_radius_z,
                state.shape_segments.max(3),
            );
            state.queue_action(UiAction::PathAppendShape { points: pts });
        }
        if ui.button("Add Circle").on_hover_text("Append a circle ring to the edited track").clicked() {
            let pts = curve::circle([0.0, 0.0, 0.0], state.shape_radius, state.shape_segments.max(3));
            state.queue_action(UiAction::PathAppendShape { points: pts });
        }
    });
    ui.label(
        egui::RichText::new("Shapes append at the origin; drag points to reposition.")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
}

fn render_terrain_tools_section(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Terrain Tools").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("(placeholder — future terrain tools)")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_descriptor_maps_all_controls() {
        let state = ToolPanelState {
            open: true,
            selected_hexon_ref: Some("blob://model.glb".to_string()),
            spacing_mode: SpacingMode::FixedCount,
            spacing_value: 2.5,
            count_value: 7,
            tangent_align: true,
            ..Default::default()
        };
        let desc = build_descriptor(&state, "blob://model.glb".to_string());
        assert_eq!(desc.asset_path, "blob://model.glb");
        assert_eq!(desc.spacing_mode, fe_sdk::path_asset::SpacingMode::FixedCount);
        assert_eq!(desc.spacing_value, 2.5);
        assert_eq!(desc.count, 7);
        assert!(desc.tangent_align);
    }

    #[test]
    fn spacing_mode_to_sdk_roundtrips_both_variants() {
        assert_eq!(SpacingMode::FixedSpacing.to_sdk(), fe_sdk::path_asset::SpacingMode::FixedSpacing);
        assert_eq!(SpacingMode::FixedCount.to_sdk(), fe_sdk::path_asset::SpacingMode::FixedCount);
    }
}
