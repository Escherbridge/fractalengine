//! Keyboard shortcuts: tool switching (bindings from `panels::toolbar::TOOL_DEFS`)
//! and staged Escape — first press exits path editing, second clears selection.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::NodeManager;
use crate::gis::PathEditorState;
use crate::panels::toolbar::TOOL_DEFS;
use crate::plugin::ToolState;

pub(super) fn handle_tool_shortcuts(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ToolState>,
    mut manager: ResMut<NodeManager>,
    mut path_state: ResMut<PathEditorState>,
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
        if keyboard.just_pressed(def.key_code) {
            tool.active_tool = def.tool;
            return;
        }
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        if path_state.editing_track_id.is_some() {
            path_state.stop_editing();
        } else {
            manager.deselect();
        }
    }
}
