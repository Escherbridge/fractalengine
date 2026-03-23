use bevy_egui::egui;

use crate::atlas::DashboardState;

pub fn gardener_console(ctx: &egui::Context) {
    node_status_panel(ctx);
    petal_list_panel(ctx);
    peer_manager_panel(ctx);
    role_editor_panel(ctx);
    asset_browser_panel(ctx);
    session_monitor_panel(ctx);
    space_overview_panel(ctx, &DashboardState::default(), false);
}

/// Space overview panel: shows aggregate petal/room/model counts, tag filter,
/// and (for admins) a Config URL field.
///
/// `is_admin` gates the config_url field visibility.
pub fn space_overview_panel(ctx: &egui::Context, state: &DashboardState, is_admin: bool) {
    egui::Window::new("Space Overview").show(ctx, |ui| {
        if state.is_online {
            ui.colored_label(egui::Color32::GREEN, "● Online");
        } else {
            ui.colored_label(egui::Color32::RED, "● Offline");
        }
        ui.separator();
        ui.label(format!("Petals: {}", state.petal_count));
        ui.label(format!("Rooms:  {}", state.room_count));
        ui.label(format!("Models: {}", state.model_count));
        ui.label(format!("Peers:  {}", state.peer_count));

        ui.separator();
        ui.label("Tag Filter:");
        // Tag filter chips would be driven by SearchQuery resource in a Bevy system.
        ui.label("(tag filter placeholder)");

        if is_admin {
            ui.separator();
            ui.label("Config URL:");
            // Config URL field is admin-only.
            ui.label("(config url field — admin only)");
        }
    });
}

fn node_status_panel(ectx: &egui::Context) {
    egui::Window::new("Node Status").show(ectx, |ui| {
        ui.label("Thread Status");
        ui.horizontal(|ui| {
            ui.label("Bevy:");
            ui.colored_label(egui::Color32::GREEN, "●");
            ui.label("Running");
        });
        ui.horizontal(|ui| {
            ui.label("Network:");
            ui.colored_label(egui::Color32::GREEN, "●");
            ui.label("Running");
        });
        ui.horizontal(|ui| {
            ui.label("Database:");
            ui.colored_label(egui::Color32::GREEN, "●");
            ui.label("Running");
        });
        ui.separator();
        ui.label("Peers connected: —");
        ui.label("Frame time: —");
    });
}

fn petal_list_panel(ectx: &egui::Context) {
    egui::Window::new("Petals").show(ectx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("No petals loaded");
        });
    });
}

fn peer_manager_panel(ectx: &egui::Context) {
    egui::Window::new("Peer Manager").show(ectx, |ui| {
        ui.label("Connected peers: —");
    });
}

fn role_editor_panel(ectx: &egui::Context) {
    egui::Window::new("Role Editor").show(ectx, |ui| {
        ui.label("Custom Roles");
        if ui.button("+ New Role").clicked() {}
    });
}

fn asset_browser_panel(ectx: &egui::Context) {
    egui::Window::new("Asset Browser").show(ectx, |ui| {
        ui.label("Assets (BLAKE3-addressed)");
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("No assets");
        });
    });
}

fn session_monitor_panel(ectx: &egui::Context) {
    egui::Window::new("Sessions").show(ectx, |ui| {
        ui.label("Active sessions");
    });
}
