//! Toolbar constants + the `Tool` enum: single source for `TOOL_DEFS`, the
//! shortcut hint line, the active-tool temp-data stash, and `mode_button_fill`.
//! The top-toolbar RENDER body lives in `ui_shell::topbar` (FR-4) and calls
//! these. See `fe-ui/src/panels/AGENTS.md`.

use bevy::prelude::KeyCode;
use bevy_egui::egui;

use crate::panels::tool_inspector::panel_descriptor;
use crate::theme;

/// Active viewport transform tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
    Pen,
    /// Paint spaced, non-destructive earthwork dabs in the viewport.
    Brush,
}

/// One viewport tool: button glyph, user-facing name, shortcut, key binding.
/// The hover tooltip text is sourced from `tool_inspector::panel_descriptor`
/// (joined in `tool_tooltip_text`) so button and description can't drift.
pub(crate) struct ToolDef {
    pub(crate) tool: Tool,
    pub(crate) glyph: &'static str,
    pub(crate) name: &'static str,
    pub(crate) key: &'static str,
    pub(crate) key_code: KeyCode,
}

/// Single source for toolbar buttons/tooltips, the keyboard bindings
/// (`node_manager::shortcuts`), and the viewport hint line — so they can't drift.
pub(crate) const TOOL_DEFS: [ToolDef; 6] = [
    ToolDef {
        tool: Tool::Select,
        glyph: "\u{2B1A}",
        name: "Select",
        key: "S",
        key_code: KeyCode::KeyS,
    },
    ToolDef {
        tool: Tool::Move,
        glyph: "\u{271B}",
        name: "Move",
        key: "G",
        key_code: KeyCode::KeyG,
    },
    ToolDef {
        tool: Tool::Rotate,
        glyph: "\u{21BB}",
        name: "Rotate",
        key: "R",
        key_code: KeyCode::KeyR,
    },
    ToolDef {
        tool: Tool::Scale,
        glyph: "\u{2921}",
        name: "Scale",
        key: "X",
        key_code: KeyCode::KeyX,
    },
    ToolDef {
        tool: Tool::Pen,
        glyph: "\u{270E}",
        name: "Pen",
        key: "P",
        key_code: KeyCode::KeyP,
    },
    ToolDef {
        tool: Tool::Brush,
        glyph: "\u{25CF}",
        name: "Brush",
        key: "B",
        key_code: KeyCode::KeyB,
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

/// Id under which `stash_active_tool` stashes the frame's active tool (egui
/// temp data — same idiom as the sidebar drag index) for panels that can't
/// reach `ToolState`, e.g. the viewport hint.
fn active_tool_id() -> egui::Id {
    egui::Id::new("fe_active_tool")
}

/// Reads back the active tool stashed by the topbar this frame.
pub(crate) fn active_tool_hint(ctx: &egui::Context) -> Option<Tool> {
    ctx.data(|d| d.get_temp(active_tool_id()))
}

/// Stashes the frame's active tool under the private temp-data key that
/// [`active_tool_hint`] reads. Called by `ui_shell::topbar` (FR-4) so the
/// temp-data key stays single-source here.
pub(crate) fn stash_active_tool(ctx: &egui::Context, tool: Tool) {
    ctx.data_mut(|d| d.insert_temp(active_tool_id(), tool));
}

/// Active-MODE button fill: a luminance emphasis (brighter neutral), not a hue
/// shift (`code_styleguides/ui_ux.md §1`). Single source for the topbar's
/// active-tool button + the right-sidebar section rail.
pub(crate) fn mode_button_fill(active: bool) -> egui::Color32 {
    if active {
        theme::BG_MODE_ACTIVE
    } else {
        theme::BG_BUTTON
    }
}

/// Rich hover-tooltip text for a topbar mode button: title + shortcut on the
/// first line, its one-line description, then its Use-zone guidance — joined
/// from `TOOL_DEFS` (shortcut) and `tool_inspector::panel_descriptor`
/// (title/subtitle/Use zone) so button, shortcut, and tooltip can never drift.
/// Pure; ready for `ui_shell::topbar::render_topbar` to call in place of its
/// current inline `format!("{tip} ({key})")` (Phase 6 wiring — see
/// `panels/AGENTS.md` §tool-inspector).
pub(crate) fn tool_tooltip_text(def: &ToolDef) -> String {
    let desc = panel_descriptor(def.tool);
    let mut text = format!("{} ({})\n{}", desc.title, def.key, desc.subtitle);
    if !desc.use_zone.is_empty() {
        text.push_str("\n\nUse:");
        for item in desc.use_zone {
            text.push_str(&format!("\n\u{2022} {item}"));
        }
    }
    text
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
            Tool::Brush,
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

    #[test]
    fn mode_button_fill_uses_luminance_emphasis() {
        assert_eq!(mode_button_fill(true), crate::theme::BG_MODE_ACTIVE);
        assert_eq!(mode_button_fill(false), crate::theme::BG_BUTTON);
    }

    #[test]
    fn tool_tooltip_text_covers_every_tool_with_title_shortcut_description_and_use_zone() {
        // Single-source join (FR-8): the tooltip can never drift from TOOL_DEFS
        // or panel_descriptor because it is built FROM them.
        for def in &TOOL_DEFS {
            let tip = tool_tooltip_text(def);
            let desc = panel_descriptor(def.tool);
            assert!(
                tip.contains(desc.title),
                "{:?} tooltip missing title: {tip}",
                def.tool
            );
            assert!(
                tip.contains(def.key),
                "{:?} tooltip missing shortcut: {tip}",
                def.tool
            );
            assert!(
                tip.contains(desc.subtitle),
                "{:?} tooltip missing description: {tip}",
                def.tool
            );
            for item in desc.use_zone {
                assert!(
                    tip.contains(item),
                    "{:?} tooltip missing use-zone item {item}: {tip}",
                    def.tool
                );
            }
        }
    }
}
