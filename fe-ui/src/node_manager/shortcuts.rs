//! Keyboard shortcuts: tool switching (bindings from `panels::toolbar::TOOL_DEFS`)
//! and staged Escape — first press exits path editing, second clears selection.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::NodeManager;
use crate::gis::PathEditorState;
use crate::panels::toolbar::Tool;
use crate::panels::toolbar::TOOL_DEFS;
use crate::plugin::ToolState;
use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

fn shortcut_allowed(tool: Tool, command_modifier: bool) -> bool {
    tool != Tool::Brush || !command_modifier
}

pub(super) fn handle_tool_shortcuts(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ToolState>,
    mut manager: ResMut<NodeManager>,
    mut path_state: ResMut<PathEditorState>,
    mut right_sidebar: ResMut<RightSidebarState>,
    mut egui_ctx: EguiContexts,
) {
    let egui_wants_kb = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.wants_keyboard_input())
        .unwrap_or(false);
    if egui_wants_kb {
        return;
    }

    for def in &TOOL_DEFS {
        let command_modifier = keyboard.pressed(KeyCode::ControlLeft)
            || keyboard.pressed(KeyCode::ControlRight)
            || keyboard.pressed(KeyCode::SuperLeft)
            || keyboard.pressed(KeyCode::SuperRight);
        if keyboard.just_pressed(def.key_code) && shortcut_allowed(def.tool, command_modifier) {
            tool.active_tool = def.tool;
            if def.tool == Tool::Brush {
                right_sidebar.requested = Some(RightSidebarSection::Tool);
            }
            return;
        }
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        if tool.active_tool == Tool::Brush {
            return;
        }
        if path_state.editing_track_id.is_some() {
            path_state.stop_editing();
        } else {
            manager.deselect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_uses_bare_b_without_stealing_sidebar_toggle() {
        assert!(shortcut_allowed(Tool::Brush, false));
        assert!(!shortcut_allowed(Tool::Brush, true));
        assert!(shortcut_allowed(Tool::Pen, true));
    }
}
