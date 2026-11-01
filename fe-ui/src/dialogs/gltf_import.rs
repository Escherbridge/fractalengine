//! GLTF model import dialog.
//!
//! D-06: Import used to close unconditionally even on empty input, and a
//! failed import was log-only — "succeeding" visibly while doing nothing.
//! Import is now disabled until [`import_enabled`] holds, and a click keeps
//! the dialog open with a spinner ("Importing…") until `db_results`'
//! `GltfImported` arm finishes `ui_mgr.gltf_import_request` and closes it
//! (see `dialogs/AGENTS.md` §gltf-import-flow).

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::{send_or_toast, UiManager};
use crate::navigation_manager::NavigationManager;
use crate::panels::widgets;
use crate::theme;
use fe_runtime::messages::DbCommand;

/// D-06: Import may fire only with a non-blank path AND an active petal —
/// importing into nothing silently did nothing before this.
pub(crate) fn import_enabled(file_path: &str, petal_id: &str) -> bool {
    !file_path.trim().is_empty() && !petal_id.trim().is_empty()
}

/// How long an "Importing…" lock is honored before this dialog re-enables
/// itself. A failed import finishes the request through
/// `db_results/query.rs::handle_error`, so this only fires when no reply
/// arrives at all (the DB thread died mid-request).
const IMPORT_LOCK_TIMEOUT_SECONDS: f64 = 30.0;

pub fn render_gltf_import_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    world_scale: f64,
) {
    let petal_id = nav.active_petal_id.clone().unwrap_or_default();

    // Read before the `active_dialog` sub-borrow below, same reasoning as
    // `join.rs::render_join_dialog` (the borrow of `ui_mgr.active_dialog`
    // through `file_path_buf`/`name_buf`/`position` must end before `ui_mgr`
    // as a whole can be passed to `send_or_toast`/`close_dialog` again).
    let now = ctx.input(|i| i.time);
    let importing = ui_mgr
        .gltf_import_request
        .waiting_within(now, IMPORT_LOCK_TIMEOUT_SECONDS);
    let error = ui_mgr.gltf_import_request.error.clone();

    let ActiveDialog::GltfImport {
        ref mut file_path_buf,
        ref mut name_buf,
        ref mut position,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let can_import = import_enabled(file_path_buf, &petal_id);
    let mut close = false;
    let mut import_request: Option<DbCommand> = None;

    let mut still_open = true;
    let mut window = egui::Window::new("Import GLTF Model")
        .collapsible(false)
        .resizable(false)
        .default_width(340.0)
        .max_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        );
    if !importing {
        window = window.open(&mut still_open);
    }
    window.show(ctx, |ui| {
        ui.add_enabled_ui(!importing, |ui| {
            ui.label(
                egui::RichText::new("File Path:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(file_path_buf)
                        .hint_text("path/to/model.glb")
                        .desired_width(ui.available_width() - 70.0),
                );
                if ui
                    .add(egui::Button::new("Browse...").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("GLTF Models", &["glb", "gltf"])
                        .add_filter("All Files", &["*"])
                        .pick_file()
                    {
                        *file_path_buf = path.display().to_string();
                        if name_buf.is_empty() {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                *name_buf = stem.to_string();
                            }
                        }
                    }
                }
            });

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Display Name:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(name_buf)
                    .hint_text("My Model")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Position:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                for (axis, val) in ["X", "Y", "Z"].iter().zip(position.iter()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.2} m",
                            widgets::world_to_meters(*val, world_scale)
                        ))
                        .monospace()
                        .small(),
                    );
                }
            });
        });

        if importing {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label("Importing\u{2026}");
            });
        } else if let Some(err) = error.as_deref() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).color(theme::STATUS_OFFLINE));
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !importing && can_import,
                    egui::Button::new("Import").fill(theme::BG_SAVE),
                )
                .clicked()
            {
                import_request = Some(DbCommand::ImportGltf {
                    petal_id: petal_id.clone(),
                    name: if name_buf.trim().is_empty() {
                        "Imported Model".to_string()
                    } else {
                        name_buf.trim().to_string()
                    },
                    file_path: file_path_buf.trim().to_string(),
                    position: *position,
                });
            }
            if ui
                .add_enabled(!importing, egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                .clicked()
            {
                close = true;
            }
        });
    });

    // The sub-borrow into `ui_mgr.active_dialog` ended above, so `ui_mgr` as
    // a whole can be mutated again from here down.
    if let Some(cmd) = import_request {
        if send_or_toast(db_tx, cmd, ui_mgr, now, "import the model") {
            ui_mgr.gltf_import_request.start(now);
        }
    }

    if !still_open || close {
        ui_mgr.gltf_import_request.reset();
        ui_mgr.close_dialog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gltf_import_disabled_without_petal_or_path() {
        let cases = [
            ("", "", false, "neither set"),
            ("model.glb", "", false, "no active petal"),
            ("", "petal-1", false, "no path"),
            ("model.glb", "petal-1", true, "both set"),
            ("   ", "petal-1", false, "whitespace-only path is still blank"),
            ("model.glb", "   ", false, "whitespace-only petal id is still blank"),
        ];
        for (file_path, petal_id, expected, why) in cases {
            assert_eq!(
                import_enabled(file_path, petal_id),
                expected,
                "{why}: import_enabled({file_path:?}, {petal_id:?})"
            );
        }
    }
}
