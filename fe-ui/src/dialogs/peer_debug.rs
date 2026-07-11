//! Peer debug overlay panel.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::theme;

pub fn render_peer_debug_panel(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    sync_status: Option<&fe_sync::SyncStatus>,
) {
    if !matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
        return;
    }

    let mut close = false;

    egui::Window::new("Peer Debug")
        .collapsible(true)
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            if let Some(status) = sync_status {
                ui.label(format!(
                    "Online: {}",
                    if status.online { "Yes" } else { "No" }
                ));
                ui.label(format!("Peer count: {}", status.peer_count));
                if let Some(ref addr) = status.node_addr {
                    ui.add_space(4.0);
                    ui.label("Node address:");
                    ui.monospace(addr);
                }
            } else {
                ui.label("Sync status not available");
            }

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Peer list will be populated when gossip discovery is active.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );

            ui.add_space(4.0);
            if ui.button("Close").clicked() {
                close = true;
            }
        });

    if close {
        ui_mgr.close_dialog();
    }
}
