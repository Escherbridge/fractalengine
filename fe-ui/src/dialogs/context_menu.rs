//! Viewport right-click context menu.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::{UiAction, UiManager};
use crate::theme;

pub fn render_context_menu(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let ActiveDialog::ContextMenu {
        screen_pos,
        world_pos,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let pos = egui::pos2(screen_pos[0], screen_pos[1]);
    let world = world_pos;

    let mut next_dialog: Option<ActiveDialog> = None;
    let mut create_node_at: Option<[f32; 3]> = None;
    let mut close = false;

    let area_response = egui::Area::new(egui::Id::new("viewport_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::BG_CONTEXT_MENU)
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM))
                .show(ui, |ui| {
                    ui.set_min_width(160.0);

                    if ui
                        .add(egui::Button::new("Add GLTF Model").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        next_dialog = Some(ActiveDialog::GltfImport {
                            file_path_buf: String::new(),
                            name_buf: String::new(),
                            position: world,
                        });
                    }

                    if ui
                        .add(egui::Button::new("Add Empty Node").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        create_node_at = Some(world);
                        close = true;
                    }
                });
        });

    // Close on click elsewhere — use the actual rendered rect rather than a
    // hardcoded size so all items are accounted for regardless of content.
    if ctx.input(|i| i.pointer.any_pressed()) {
        let ptr = ctx.input(|i| i.pointer.interact_pos());
        if let Some(ptr_pos) = ptr {
            let menu_rect = area_response.response.rect;
            if !menu_rect.contains(ptr_pos) {
                close = true;
            }
        }
    }

    if let Some(position) = create_node_at {
        ui_mgr.push_action(UiAction::CreateNodeAt { position });
    }
    if let Some(dialog) = next_dialog {
        ui_mgr.open_dialog(dialog);
    } else if close {
        ui_mgr.close_dialog();
    }
}
