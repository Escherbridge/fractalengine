use bevy::prelude::*;
use bevy_egui::egui;

pub mod dashboard;
pub mod model_editor;
pub mod petal_wizard;
pub mod room_editor;
pub mod search_bar;
pub mod tag_panel;
pub mod visibility_control;

pub use dashboard::DashboardState;
pub use model_editor::{KvRow, ModelEditorState};
pub use petal_wizard::{PetalWizardState, TagError, WizardStep};
pub use room_editor::RoomEditorState;
pub use search_bar::SearchQuery;
pub use visibility_control::VisibilityExt;

/// Render the petal creation wizard panel.
pub fn petal_wizard_system(
    mut _commands: Commands,
    mut _wizard: Local<PetalWizardState>,
    mut ctx: bevy_egui::EguiContexts,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };
    petal_wizard_panel(ectx);
}

fn petal_wizard_panel(ctx: &egui::Context) {
    egui::Window::new("Create Petal").show(ctx, |ui| {
        ui.label("Petal Wizard — Step 1: Identity");
        ui.separator();
        ui.label("Name:");
        // Input widgets would be wired to PetalWizardState here
        ui.label("(wizard UI placeholder)");
    });
}

/// Render the room metadata editor panel.
pub fn room_editor_system(
    mut _commands: Commands,
    mut _editor: Local<RoomEditorState>,
    mut ctx: bevy_egui::EguiContexts,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };
    room_editor_panel(ectx);
}

fn room_editor_panel(ctx: &egui::Context) {
    egui::Window::new("Room Editor").show(ctx, |ui| {
        ui.label("Room Metadata Editor");
        ui.separator();
        ui.label("(room editor UI placeholder)");
    });
}

/// Render the model metadata editor panel.
pub fn model_editor_system(
    mut _commands: Commands,
    mut _editor: Local<ModelEditorState>,
    mut ctx: bevy_egui::EguiContexts,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };
    model_editor_panel(ectx);
}

fn model_editor_panel(ctx: &egui::Context) {
    egui::Window::new("Model Editor").show(ctx, |ui| {
        ui.label("Model Metadata Editor");
        ui.separator();
        ui.label("(model editor UI placeholder)");
    });
}
