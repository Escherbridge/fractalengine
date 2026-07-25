//! Topbar area manager (FR-4): renders the top toolbar — transform-tool
//! switcher, deselect, and the Data/Tools/Settings/Maps cluster. The tool data
//! (`TOOL_DEFS`), the active-tool temp-data stash, and `mode_button_fill` stay
//! single-source in `panels::toolbar`; this manager calls them. See
//! `fe-ui/src/ui_shell/AGENTS.md` §topbar.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::dialogs::ActiveDialog;
use crate::panels::toolbar::{mode_button_fill, stash_active_tool, TOOL_DEFS};
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
/// (FR-4); the only behavioral change is the Tools button also driving the
/// right-sidebar `Tool` section (with a compat shim mirroring the legacy flag).
pub fn render_topbar(
    ctx: &egui::Context,
    _topbar: &mut TopbarState,
    tool: &mut ToolState,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    gis_panel: &mut crate::gis::GisPanelState,
    tool_panel: &mut crate::panels::tool_panel::ToolPanelState,
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
                    if ui
                        .add(btn)
                        .on_hover_text(format!("{} ({})", def.tip, def.key))
                        .clicked()
                    {
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
                        // FR-4: the Tools button now toggles the right-sidebar
                        // `Tool` section (the target architecture).
                        right.toggle(RightSidebarSection::Tool);
                        // COMPAT SHIM (Phase 2): mirror until Phase 4 removes floating windows
                        tool_panel.open = right.is_active(RightSidebarSection::Tool);
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
