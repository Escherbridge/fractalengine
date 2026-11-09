//! Extension UI slot host: renders manifest-declared contributions (see `fe-ui/src/extensions/AGENTS.md`).

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
