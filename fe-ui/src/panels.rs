use bevy_egui::{egui, EguiContexts};

pub fn node_status_panel(mut ctx: EguiContexts) {
    egui::Window::new("Node Status").show(ctx.ctx_mut(), |ui| {
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

pub fn petal_list_panel(mut ctx: EguiContexts) {
    egui::Window::new("Petals").show(ctx.ctx_mut(), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("No petals loaded");
        });
    });
}

pub fn peer_manager_panel(mut ctx: EguiContexts) {
    egui::Window::new("Peer Manager").show(ctx.ctx_mut(), |ui| {
        ui.label("Connected peers: —");
    });
}

pub fn role_editor_panel(mut ctx: EguiContexts) {
    egui::Window::new("Role Editor").show(ctx.ctx_mut(), |ui| {
        ui.label("Custom Roles");
        if ui.button("+ New Role").clicked() {
            // CROSS-CRATE: send DbCommand to create role
        }
    });
}

pub fn asset_browser_panel(mut ctx: EguiContexts) {
    egui::Window::new("Asset Browser").show(ctx.ctx_mut(), |ui| {
        ui.label("Assets (BLAKE3-addressed)");
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("No assets");
        });
    });
}

pub fn session_monitor_panel(mut ctx: EguiContexts) {
    egui::Window::new("Sessions").show(ctx.ctx_mut(), |ui| {
        ui.label("Active sessions");
    });
}
