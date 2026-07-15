//! Top toolbar: transform tool switcher, deselect, GIS/Tools/Hexons buttons.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::dialogs::ActiveDialog;
use crate::plugin::ToolState;
use crate::terrain_map::{HexonManagerTab, StorageInfoDto};
use crate::theme;

/// Active viewport transform tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
    /// Click-to-place polyline pen for path editing; see `node_manager/AGENTS.md` §pen-tool.
    Pen,
}

pub(crate) fn top_toolbar(
    ctx: &egui::Context,
    tool: &mut ToolState,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    gis_panel: &mut crate::gis::GisPanelState,
    tool_panel: &mut crate::panels::tool_panel::ToolPanelState,
) {
    egui::TopBottomPanel::top("toolbar")
        .exact_height(40.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_TOOLBAR)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // Sidebar toggle removed: auto-collapse in panels/mod.rs overwrites
                // `sidebar.open` every frame, making a manual button a no-op.
                // TODO: re-add manual sidebar toggle when needed
                for (t, label, tooltip) in [
                    (Tool::Select, "\u{2B1A} Select", "Select objects (S)"),
                    (Tool::Move, "\u{271B} Move", "Move selected object (G)"),
                    (
                        Tool::Rotate,
                        "\u{21BB} Rotate",
                        "Rotate selected object (R)",
                    ),
                    (Tool::Scale, "\u{2921} Scale", "Scale selected object (X)"),
                    (
                        Tool::Pen,
                        "\u{270E} Pen",
                        "Draw a path: click the viewport to add points (P)",
                    ),
                ] {
                    let active = tool.active_tool == t;
                    let btn = egui::Button::new(label).fill(if active {
                        theme::BG_BUTTON_ACTIVE
                    } else {
                        theme::BG_BUTTON
                    });
                    if ui.add(btn).on_hover_text(tooltip).clicked() {
                        tool.active_tool = t;
                    }
                }

                ui.separator();

                if node_mgr.selected_entity().is_some()
                    && ui
                        .add(egui::Button::new("\u{2715} Deselect").fill(theme::BG_DANGER))
                        .clicked()
                {
                    node_mgr.deselect();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("FractalEngine")
                            .color(theme::TEXT_DIM)
                            .small(),
                    );

                    if ui
                        .add(egui::Button::new("\u{1F5FA} GIS").fill(if gis_panel.open {
                            theme::BG_BUTTON_ACTIVE
                        } else {
                            theme::BG_BUTTON
                        }))
                        .on_hover_text("Query nodes, annotations, and terrain layers")
                        .clicked()
                    {
                        gis_panel.open = !gis_panel.open;
                    }

                    if ui
                        .add(
                            egui::Button::new("\u{1F527} Tools").fill(if tool_panel.open {
                                theme::BG_BUTTON_ACTIVE
                            } else {
                                theme::BG_BUTTON
                            }),
                        )
                        .on_hover_text("Path-asset stamp, pen curves, and shape tools")
                        .clicked()
                    {
                        tool_panel.open = !tool_panel.open;
                    }

                    if ui
                        .add(egui::Button::new("\u{1F4E6} Hexons").fill(theme::BG_BUTTON))
                        .on_hover_text("Manage terrain tilesets")
                        .clicked()
                    {
                        ui_mgr.open_dialog(ActiveDialog::HexonManager {
                            installed_tilesets: Vec::new(),
                            available_tilesets: Vec::new(),
                            download_progress: std::collections::HashMap::new(),
                            filter_text: String::new(),
                            active_tab: HexonManagerTab::Installed,
                            storage_info: StorageInfoDto {
                                base_dir: String::new(),
                                total_bytes: 0,
                                count: 0,
                            },
                            loading: true,
                            pending_remove: None,
                        });
                        ui_mgr.push_action(UiAction::HexonRefreshList);
                    }
                });
            });
        });
}
