//! Keyboard shortcuts for tool switching and deselect.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::NodeManager;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;

pub(super) fn handle_tool_shortcuts(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ToolState>,
    mut manager: ResMut<NodeManager>,
    mut egui_ctx: EguiContexts,
) {
    let egui_wants_kb = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.wants_keyboard_input())
        .unwrap_or(false);
    if egui_wants_kb {
        return;
    }

    if keyboard.just_pressed(KeyCode::KeyS) {
        tool.active_tool = Tool::Select;
    } else if keyboard.just_pressed(KeyCode::KeyG) {
        tool.active_tool = Tool::Move;
    } else if keyboard.just_pressed(KeyCode::KeyR) {
        tool.active_tool = Tool::Rotate;
    } else if keyboard.just_pressed(KeyCode::KeyX) {
        tool.active_tool = Tool::Scale;
    } else if keyboard.just_pressed(KeyCode::KeyP) {
        tool.active_tool = Tool::Pen;
    } else if keyboard.just_pressed(KeyCode::Escape) {
        manager.deselect();
    }
}
