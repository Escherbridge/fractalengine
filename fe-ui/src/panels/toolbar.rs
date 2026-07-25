//! Toolbar constants + the `Tool` enum: single source for `TOOL_DEFS`, the
//! shortcut hint line, the active-tool temp-data stash, and `mode_button_fill`.
//! The top-toolbar RENDER body lives in `ui_shell::topbar` (FR-4) and calls
//! these. See `fe-ui/src/panels/AGENTS.md`.

use bevy::prelude::KeyCode;
use bevy_egui::egui;

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
/// active-tool button + the right-sidebar section rail. (`tool_inspector.rs`
/// keeps its own identical copy until the Phase-5 sibling folds it in.)
pub(crate) fn mode_button_fill(active: bool) -> egui::Color32 {
    if active {
        theme::BG_MODE_ACTIVE
    } else {
        theme::BG_BUTTON
    }
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

    #[test]
    fn mode_button_fill_uses_luminance_emphasis() {
        assert_eq!(mode_button_fill(true), crate::theme::BG_MODE_ACTIVE);
        assert_eq!(mode_button_fill(false), crate::theme::BG_BUTTON);
    }
}
