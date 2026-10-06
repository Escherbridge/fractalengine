//! Topbar area manager (FR-4): renders the top toolbar — transform-tool
//! switcher, deselect, and the Data/Tools/Settings/Maps cluster. The tool data
//! (`TOOL_DEFS`), the `InputContext` temp-data stash, and `mode_button_fill`
//! stay single-source in `panels::toolbar`; this manager calls them. Tool
//! activation routes through `ToolState::activate` — the single writer. See
//! `fe-ui/src/ui_shell/AGENTS.md` §topbar.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::panels::toolbar::{
    mode_button_fill, stash_input_context, tool_tooltip_text, InputContext, TOOL_DEFS,
};
use crate::plugin::ToolState;
use crate::theme;
use crate::ui_shell::left_sidebar::LeftSidebarState;
use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

/// Topbar-owned UI state. Minimal this phase (the toolbar reads shared manager
/// state); reserved as the seam for future topbar-local state (e.g. overflow /
/// compact mode) so downstream slices need not re-touch `plugin.rs`.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TopbarState;

/// Renders the top toolbar. Migrated verbatim from `panels::toolbar::top_toolbar`
/// (FR-4). The former "Tools" button is the **Options** toggle since D7 — the
/// `PathTools` section it used to reveal is retired into the Options
/// dispatcher's Pen arm. Section buttons here write `RightSidebarState::toggle`
/// directly rather than `UiAction::RevealSection`: they are TOGGLES (re-press
/// closes), and the addressable reveal action is deliberately idempotent.
pub fn render_topbar(
    ctx: &egui::Context,
    _topbar: &mut TopbarState,
    tool: &mut ToolState,
    node_mgr: &mut crate::node_manager::NodeManager,
    gis_panel: &mut crate::gis::GisPanelState,
    left: &mut LeftSidebarState,
    right: &mut RightSidebarState,
) {
    // FR-3 (D-A11): keyboard shortcut for the user-sticky left-sidebar toggle
    // (Ctrl/Cmd+B). Flips session-scoped intent; the manager honors it verbatim.
    if ctx.input(|i| i.key_pressed(egui::Key::B) && i.modifiers.command) {
        left.user_intent = !left.user_intent;
    }

    egui::TopBottomPanel::top("toolbar")
        .exact_height(40.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_TOOLBAR)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // FR-3: explicit left-sidebar toggle (the only reveal/hide path
                // now that auto-collapse is gone). Active fill = currently open.
                if ui
                    .add(egui::Button::new("\u{2630}").fill(mode_button_fill(left.user_intent)))
                    .on_hover_text("Toggle left sidebar (Ctrl+B)")
                    .clicked()
                {
                    left.user_intent = !left.user_intent;
                }
                ui.separator();

                for def in &TOOL_DEFS {
                    let active = tool.active_tool == def.tool;
                    // tool_inspector_ux_20260719 (FR-1): active MODE reads via
                    // luminance, not a saturated-blue hue (ui_ux.md §1).
                    let btn = egui::Button::new(format!("{} {}", def.glyph, def.name))
                        .fill(mode_button_fill(active));
                    if ui.add(btn).on_hover_text(tool_tooltip_text(def)).clicked() {
                        // D1/D9: one activation writer — re-press toggles to
                        // Select, and the options reveal is its job, not ours.
                        tool.activate(def.tool, right);
                    }
                }
                // Publish the frame's input context for the egui-side surfaces
                // that cannot read Bevy resources (viewport hint + right-click).
                stash_input_context(
                    ui.ctx(),
                    InputContext {
                        active_tool: tool.active_tool,
                        gesture_active: node_mgr.gesture_active,
                    },
                );

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

                    // D7: "Tools" was the PathTools reveal; that section is
                    // retired, so the button is the Options surface's toggle —
                    // one label for "the active tool's settings", whatever the
                    // active tool is.
                    if ui
                        .add(egui::Button::new("\u{1F527} Options").fill(
                            if right.is_active(RightSidebarSection::Options) {
                                theme::BG_BUTTON_ACTIVE
                            } else {
                                theme::BG_BUTTON
                            },
                        ))
                        .on_hover_text("Settings for the active tool (pen curves, brush, stamping)")
                        .clicked()
                    {
                        right.toggle(RightSidebarSection::Options);
                    }

                    // FR-1 (D-A10): Settings is a one-at-a-time right-sidebar
                    // section now, not a floating modal — toggle it like the
                    // other tool surfaces.
                    if ui
                        .add(egui::Button::new("\u{2699} Settings").fill(
                            if right.is_active(RightSidebarSection::Settings) {
                                theme::BG_BUTTON_ACTIVE
                            } else {
                                theme::BG_BUTTON
                            },
                        ))
                        .on_hover_text("Application settings (render distance, mesh budget, ...)")
                        .clicked()
                    {
                        right.toggle(RightSidebarSection::Settings);
                    }

                    // FR-2 (D-A10): Maps (map manager) is a right-sidebar section
                    // too; the section self-seeds its data + refresh on open.
                    if ui
                        .add(egui::Button::new("\u{1F4E6} Maps").fill(
                            if right.is_active(RightSidebarSection::Maps) {
                                theme::BG_BUTTON_ACTIVE
                            } else {
                                theme::BG_BUTTON
                            },
                        ))
                        .on_hover_text("Manage petal maps")
                        .clicked()
                    {
                        right.toggle(RightSidebarSection::Maps);
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::panels::toolbar::{input_context, Tool};

    #[test]
    fn options_button_toggles_the_options_section_both_ways() {
        // Mirrors the (ex-"Tools") Options button arm: a toggle, not a reveal.
        let mut right = RightSidebarState::default();
        right.toggle(RightSidebarSection::Options);
        assert!(right.is_active(RightSidebarSection::Options));
        right.toggle(RightSidebarSection::Options);
        assert_eq!(
            right.requested, None,
            "re-press falls back to the selection default"
        );
    }

    #[test]
    fn tool_button_press_routes_through_the_single_activation_writer() {
        // Mirrors the button arm: no Brush special case survives here.
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        tool.activate(Tool::Brush, &mut right);
        assert_eq!(tool.active_tool, Tool::Brush);
        assert!(right.is_active(RightSidebarSection::Options));
        tool.activate(Tool::Brush, &mut right);
        assert_eq!(tool.active_tool, Tool::Select, "re-press rests on Select");
        assert_eq!(right.requested, None);
    }

    #[test]
    fn topbar_publishes_both_halves_of_the_input_context() {
        // Mirrors the stash call after the tool-button loop: the tool half from
        // `ToolState`, the gesture half from the `NodeManager` per-frame mirror.
        let ctx = egui::Context::default();
        let tool = ToolState {
            active_tool: Tool::Pen,
        };
        let node_mgr = crate::node_manager::NodeManager {
            gesture_active: true,
            ..Default::default()
        };
        stash_input_context(
            &ctx,
            InputContext {
                active_tool: tool.active_tool,
                gesture_active: node_mgr.gesture_active,
            },
        );
        let read = input_context(&ctx).expect("topbar stashed it");
        assert_eq!(read.active_tool, Tool::Pen);
        assert!(read.gesture_active);
    }
}
