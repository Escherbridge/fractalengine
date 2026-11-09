//! Extension UI: the slot host, the Extensions panel, consent and the built-in connectors
//! (see `fe-ui/src/extensions/AGENTS.md` and `docs/extensions/extensions-ui.md`).

pub mod browse;
pub mod builtin;
pub mod consent;
pub mod details;
pub mod panel;
pub mod permissions;
pub mod slots;

use bevy::prelude::*;

pub use slots::{
    ExtensionCommandRequest, ExtensionSlotState, SlotContext, details_only, draw_command_slot,
    draw_context_menu_items, draw_inspector_rows, draw_stream_slot,
};

/// Initialises the slot host state; the registry itself is `plugin::UiExtensions`.
pub struct ExtensionSlotsPlugin;

impl Plugin for ExtensionSlotsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExtensionSlotState>();
    }
}
