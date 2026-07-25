//! Topbar area manager (FR-4): renders the top toolbar — transform-tool
//! switcher, deselect, and the Data/Tools/Settings/Maps cluster. The tool data
//! (`TOOL_DEFS`), the active-tool temp-data stash, and `mode_button_fill` stay
//! single-source in `panels::toolbar`; this manager calls them. See
//! `fe-ui/src/ui_shell/AGENTS.md` §topbar.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::dialogs::ActiveDialog;
use crate::panels::toolbar::{mode_button_fill, stash_active_tool, tool_tooltip_text, TOOL_DEFS};
use crate::plugin::ToolState;
use crate::terrain_map::{HexonManagerTab, StorageInfoDto};
use crate::theme;
use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

/// Topbar-owned UI state. Minimal this phase (the toolbar reads shared manager
/// state); reserved as the seam for future topbar-local state (e.g. overflow /
/// compact mode) so downstream slices need not re-touch `plugin.rs`.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TopbarState;

/// Renders the top toolbar. Migrated verbatim from `panels::toolbar::top_toolbar`
/// (FR-4). Phase 4 (FR-9) retired the Phase-2 compat shim: the Tools button
/// now toggles the right-sidebar `Tool` section directly — that toggle is the
/// SOLE reveal path (no more mirrored `ToolPanelState.open` legacy flag).
pub fn render_topbar(
    ctx: &egui::Context,
    _topbar: &mut TopbarState,
    tool: &mut ToolState,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    gis_panel: &mut crate::gis::GisPanelState,
    right: &mut RightSidebarState,
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
                for def in &TOOL_DEFS {
                    let active = tool.active_tool == def.tool;
                    // tool_inspector_ux_20260719 (FR-1): active MODE reads via
                    // luminance, not a saturated-blue hue (ui_ux.md §1).
                    let btn = egui::Button::new(format!("{} {}", def.glyph, def.name))
                        .fill(mode_button_fill(active));
                    if ui.add(btn).on_hover_text(tool_tooltip_text(def)).clicked() {
                        tool.active_tool = def.tool;
                    }
                }
                stash_active_tool(ui.ctx(), tool.active_tool);

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
                        .add(egui::Button::new("\u{1F5FA} Data").fill(if gis_panel.open {
                            theme::BG_BUTTON_ACTIVE
                        } else {
                            theme::BG_BUTTON
                        }))
                        .on_hover_text("Query nodes and layers, export for BI")
                        .clicked()
                    {
                        gis_panel.open = !gis_panel.open;
                    }

                    if ui
                        .add(egui::Button::new("\u{1F527} Tools").fill(
                            if right.is_active(RightSidebarSection::Tool) {
                                theme::BG_BUTTON_ACTIVE
                            } else {
                                theme::BG_BUTTON
                            },
                        ))
                        .on_hover_text("Path-asset stamp, pen curves, and shape tools")
                        .clicked()
                    {
                        // FR-4/FR-9: the Tools button toggles the right-sidebar
                        // `Tool` section — the sole reveal path (Phase 4 retired
                        // the Phase-2 compat shim).
                        right.toggle(RightSidebarSection::Tool);
                    }

                    if ui
                        .add(egui::Button::new("\u{2699} Settings").fill(theme::BG_BUTTON))
                        .on_hover_text("Application settings (render distance, mesh budget, ...)")
                        .clicked()
                    {
                        ui_mgr.push_action(UiAction::SettingsToggle);
                    }

                    if ui
                        .add(egui::Button::new("\u{1F4E6} Maps").fill(theme::BG_BUTTON))
                        .on_hover_text("Manage petal maps")
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
