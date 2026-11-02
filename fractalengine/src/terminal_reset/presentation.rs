//! Terminal-only maintenance controls; see AGENTS.md.

use super::{Phase, Status};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use fe_ui::accessibility::ResponseExt;

pub(super) fn render_terminal(
    mut contexts: EguiContexts,
    status: Res<Status>,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Database maintenance");
        ui.label(&status.detail);
        ui.label("Identity, preferences, installed packages and cached blobs are retained. Shared peers are not erased.");
        ui.label("Active API sessions stop for this process. Erasing token records does not revoke previously signed credentials.");
        if matches!(status.phase, Phase::Quiescing | Phase::Clearing) {
            ui.spinner();
            ui.label("Do not repeat the reset request.");
        } else if ui.add_sized([220.0, 32.0], egui::Button::new("Close FractalEngine")).named(ui, "Close after terminal database maintenance").clicked() {
            exit.write(bevy::app::AppExit::Success);
        }
    });
}

pub(super) fn close_requests(
    mut requests: MessageReader<bevy::window::WindowCloseRequested>,
    status: Res<Status>,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    let requested = requests.read().next().is_some();
    if requested
        && matches!(
            status.phase,
            Phase::Cleared | Phase::Failed | Phase::Unconfirmed
        )
    {
        exit.write(bevy::app::AppExit::Success);
    }
}
