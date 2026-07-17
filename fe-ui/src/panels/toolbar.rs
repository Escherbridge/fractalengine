//! Top toolbar: transform tool switcher, deselect, GIS/Tools/Maps buttons.

use bevy::prelude::KeyCode;
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

/// One viewport tool: button glyph, user-facing name, shortcut, tooltip phrase.
pub(crate) struct ToolDef {
    pub(crate) tool: Tool,
    pub(crate) glyph: &'static str,
    pub(crate) name: &'static str,
    pub(crate) key: &'static str,
    pub(crate) key_code: KeyCode,
    pub(crate) tip: &'static str,
}

/// Single source for toolbar buttons/tooltips, the keyboard bindings
/// (`node_manager::shortcuts`), and the viewport hint line — so they can't drift.
pub(crate) const TOOL_DEFS: [ToolDef; 5] = [
    ToolDef {
        tool: Tool::Select,
        glyph: "\u{2B1A}",
        name: "Select",
        key: "S",
        key_code: KeyCode::KeyS,
        tip: "Select objects",
    },
    ToolDef {
        tool: Tool::Move,
        glyph: "\u{271B}",
        name: "Move",
        key: "G",
        key_code: KeyCode::KeyG,
        tip: "Move selected object",
    },
    ToolDef {
        tool: Tool::Rotate,
        glyph: "\u{21BB}",
        name: "Rotate",
        key: "R",
        key_code: KeyCode::KeyR,
        tip: "Rotate selected object",
    },
    ToolDef {
        tool: Tool::Scale,
        glyph: "\u{2921}",
        name: "Scale",
        key: "X",
        key_code: KeyCode::KeyX,
        tip: "Scale selected object",
    },
    ToolDef {
        tool: Tool::Pen,
        glyph: "\u{270E}",
        name: "Pen",
        key: "P",
        key_code: KeyCode::KeyP,
        tip: "Draw a path: click the viewport to add points",
    },
];

/// Viewport shortcut hint line generated from [`TOOL_DEFS`].
pub(crate) fn shortcut_hint_line() -> String {
    let mut line = TOOL_DEFS
        .iter()
        .map(|d| format!("{} = {}", d.key, d.name))
        .collect::<Vec<_>>()
        .join("  ");
    line.push_str("  \u{2022}  Esc = deselect  \u{2022}  Right-click = menu");
    line
}

/// Id under which `top_toolbar` stashes the frame's active tool (egui temp
/// data — same idiom as the sidebar drag index) for panels that can't reach
/// `ToolState`, e.g. the viewport hint.
fn active_tool_id() -> egui::Id {
    egui::Id::new("fe_active_tool")
}

/// Reads back the active tool stashed by `top_toolbar` this frame.
pub(crate) fn active_tool_hint(ctx: &egui::Context) -> Option<Tool> {
    ctx.data(|d| d.get_temp(active_tool_id()))
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
                for def in &TOOL_DEFS {
                    let active = tool.active_tool == def.tool;
                    let btn =
                        egui::Button::new(format!("{} {}", def.glyph, def.name)).fill(if active {
                            theme::BG_BUTTON_ACTIVE
                        } else {
                            theme::BG_BUTTON
                        });
                    if ui
                        .add(btn)
                        .on_hover_text(format!("{} ({})", def.tip, def.key))
                        .clicked()
                    {
                        tool.active_tool = def.tool;
                    }
                }
                ui.ctx()
                    .data_mut(|d| d.insert_temp(active_tool_id(), tool.active_tool));

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
                        tool_panel.open = !tool_panel.open;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_line_covers_every_tool_and_escape() {
        let line = shortcut_hint_line();
        for def in &TOOL_DEFS {
            assert!(
                line.contains(&format!("{} = {}", def.key, def.name)),
                "hint line missing {}: {line}",
                def.name
            );
        }
        assert!(line.contains("Esc"), "hint line missing Esc: {line}");
    }

    #[test]
    fn tool_defs_cover_all_tools_with_unique_keys() {
        for t in [
            Tool::Select,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
            Tool::Pen,
        ] {
            assert!(
                TOOL_DEFS.iter().any(|d| d.tool == t),
                "no ToolDef for {t:?}"
            );
        }
        let mut keys: Vec<&str> = TOOL_DEFS.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), TOOL_DEFS.len(), "duplicate shortcut keys");
    }
}
