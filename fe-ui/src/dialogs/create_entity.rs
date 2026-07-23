//! Create dialog (Verse / Fractal / Petal / Node).

use bevy_egui::egui;

use super::{ActiveDialog, CreateKind};
use crate::actions::UiManager;
use crate::navigation_manager::NavigationManager;
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

pub fn render_create_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    hierarchy: &mut VerseManager,
    nav: &mut NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::CreateEntity {
        ref mut kind,
        ref mut parent_id,
        ref mut name_buf,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let title = match *kind {
        CreateKind::Verse => "Create Verse",
        CreateKind::Fractal => "Create Fractal",
        CreateKind::Petal => "Create Petal",
        CreateKind::Node => "Create Node",
    };

    let current_kind = *kind;
    let current_parent = parent_id.clone();
    let mut close = false;

    let mut still_open = true;
    egui::Window::new(title)
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(280.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            ui.add(
                egui::TextEdit::singleline(name_buf)
                    .hint_text("Enter name\u{2026}")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Create").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let name = name_buf.trim().to_string();
                    if !name.is_empty() {
                        apply_create(hierarchy, nav, current_kind, &current_parent, &name, db_tx);
                        close = true;
                    }
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    close = true;
                }
            });
        });

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

/// Send a DB command to create the entity. The hierarchy will update when
/// the DbResult comes back -- no local push to avoid duplicates.
pub fn apply_create(
    _hierarchy: &mut VerseManager,
    _nav: &mut NavigationManager,
    kind: CreateKind,
    parent_id: &str,
    name: &str,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    match kind {
        CreateKind::Verse => {
            db_tx
                .send(DbCommand::CreateVerse {
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Fractal => {
            db_tx
                .send(DbCommand::CreateFractal {
                    verse_id: parent_id.to_string(),
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Petal => {
            db_tx
                .send(DbCommand::CreatePetal {
                    fractal_id: parent_id.to_string(),
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Node => {
            db_tx
                .send(DbCommand::CreateNode {
                    petal_id: parent_id.to_string(),
                    name: name.to_string(),
                    position: [0.0, 0.0, 0.0],
                    correlation_id: None,
                })
                .ok();
        }
    }
}
