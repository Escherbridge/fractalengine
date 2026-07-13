//! Tools panel: a floating window (independent of `ActiveDialog`, like
//! `gis_panel`) that will host hexon-path-asset stamping controls. This pass
//! ships the shell + repetition/pattern controls; the hexon picker and the
//! action wiring that turns these fields into a stamped path asset are
//! deferred to a later unit. See `fe-ui/src/panels/AGENTS.md` §tool-panel.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::theme;

/// How repeated instances along a path are spaced.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum SpacingMode {
    #[default]
    FixedSpacing,
    FixedCount,
}

/// Persistent state for the Tools panel (open flag + path-asset stamp
/// controls). Hexon selection/action wiring lands with the stamp unit.
#[derive(Resource, Default)]
pub struct ToolPanelState {
    pub open: bool,
    pub selected_hexon_ref: Option<String>,
    pub spacing_mode: SpacingMode,
    pub spacing_value: f32,
    pub count_value: u32,
    pub tangent_align: bool,
}

pub fn render_tool_panel(ctx: &egui::Context, state: &mut ToolPanelState) {
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
            render_path_asset_section(ui, state);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            render_terrain_tools_section(ui);
        });

    if !still_open {
        state.open = false;
    }
}

fn render_path_asset_section(ui: &mut egui::Ui, state: &mut ToolPanelState) {
    ui.label(egui::RichText::new("Path Asset").strong().color(theme::TEXT_SECTION));
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("(hexon picker — coming soon)")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
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
