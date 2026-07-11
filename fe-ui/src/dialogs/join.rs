//! Join-verse-by-invite dialog.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::UiManager;
use fe_runtime::messages::DbCommand;

pub fn render_join_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::JoinDialog { ref mut invite_buf } = ui_mgr.active_dialog else {
        return;
    };

    let mut close = false;

    egui::Window::new("Join Verse")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Paste an invite string to join a verse:");
            ui.add_space(4.0);

            ui.add(
                egui::TextEdit::multiline(invite_buf)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste invite string here...")
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_join = !invite_buf.trim().is_empty();
                if ui
                    .add_enabled(can_join, egui::Button::new("Join"))
                    .clicked()
                {
                    db_tx
                        .send(DbCommand::JoinVerseByInvite {
                            invite_string: invite_buf.trim().to_string(),
                        })
                        .ok();
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if close {
        ui_mgr.close_dialog();
    }
}
