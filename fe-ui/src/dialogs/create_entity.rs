//! Create dialog (Verse / Fractal / Petal / Node).
//!
//! D-07: Create used to do nothing visible on an empty name, with no
//! autofocus or Enter-to-submit. Create is now disabled on a blank name,
//! the Name field grabs focus on open, and Enter submits exactly like
//! clicking Create.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::{ActiveDialog, CreateKind};
use crate::actions::{send_or_toast, UiManager};
use crate::theme;
use fe_runtime::messages::DbCommand;

pub fn render_create_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    // L-06: a node made via the tree "+" used to spawn at the petal origin,
    // often off-screen; `Some(focus)` (the camera focus target, threaded by
    // `panels/mod.rs`) places it there instead. `None` keeps the origin.
    spawn_position: Option<[f32; 3]>,
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
    let mut create_request: Option<DbCommand> = None;

    let mut still_open = true;
    crate::ui_shell::layout_policy::bounded_window(ctx, title)
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
        .show_with_targets(ctx, |ui| {
            let name_label = ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            let name_resp = ui
                .add(
                    crate::ui_shell::layout_policy::text_input(ui, name_buf)
                        .hint_text("Enter name\u{2026}")
                        .desired_width(f32::INFINITY),
                )
                .labelled(ui, &name_label);
            // D-07: autofocus — the Name field is the only control worth
            // typing into first, and nothing else in this dialog competes
            // for focus, so "nothing is focused yet" always means this.
            if ctx.memory(|m| m.focused().is_none()) {
                name_resp.request_focus();
            }
            let submit_via_enter = name_resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_create = !name_buf.trim().is_empty();
                let create_clicked = ui
                    .add_enabled(can_create, egui::Button::new("Create").fill(theme::BG_SAVE)).observed(ui)
                    .clicked();
                if can_create && (create_clicked || submit_via_enter) {
                    let name = name_buf.trim().to_string();
                    create_request = Some(build_create_command(
                        current_kind,
                        &current_parent,
                        &name,
                        spawn_position,
                    ));
                    close = true;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON)).observed(ui)
                    .clicked()
                {
                    close = true;
                }
            });
        });

    // The sub-borrow into `ui_mgr.active_dialog` (via `kind`/`parent_id`/
    // `name_buf`) ended above, so `ui_mgr` as a whole can be mutated again
    // from here down — same reasoning as `join.rs`/`gltf_import.rs`.
    //
    // NEEDS-WIRING: a successful `VerseCreated`/`FractalCreated`/
    // `PetalCreated`/`NodeCreated` has no toast today (D-07's "toast on
    // result"); that reply lands in
    // `fe-ui/src/verse_manager/db_results/mod.rs` (owned elsewhere this
    // wave), not here, so only the immediate "channel is dead" failure is
    // toasted by `send_or_toast` below.
    if let Some(cmd) = create_request {
        let now = ctx.input(|i| i.time);
        send_or_toast(db_tx, cmd, ui_mgr, now, "create it");
    }

    if !still_open || close {
        ui_mgr.close_dialog();
    }
}

/// Build the `DbCommand` for creating `kind` under `parent_id` named `name`.
/// Pure and unit-tested so the position/field wiring can't silently drift
/// from what `render_create_dialog` sends.
pub(crate) fn build_create_command(
    kind: CreateKind,
    parent_id: &str,
    name: &str,
    spawn_position: Option<[f32; 3]>,
) -> DbCommand {
    match kind {
        CreateKind::Verse => DbCommand::CreateVerse {
            name: name.to_string(),
        },
        CreateKind::Fractal => DbCommand::CreateFractal {
            verse_id: parent_id.to_string(),
            name: name.to_string(),
        },
        CreateKind::Petal => DbCommand::CreatePetal {
            fractal_id: parent_id.to_string(),
            name: name.to_string(),
        },
        CreateKind::Node => DbCommand::CreateNode {
            petal_id: parent_id.to_string(),
            name: name.to_string(),
            position: spawn_position.unwrap_or([0.0, 0.0, 0.0]),
            correlation_id: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_create_command_table() {
        assert!(matches!(
            build_create_command(CreateKind::Verse, "", "Acme", None),
            DbCommand::CreateVerse { name } if name == "Acme"
        ));
        assert!(matches!(
            build_create_command(CreateKind::Fractal, "verse-1", "Acme", None),
            DbCommand::CreateFractal { verse_id, name }
                if verse_id == "verse-1" && name == "Acme"
        ));
        assert!(matches!(
            build_create_command(CreateKind::Petal, "fractal-1", "Acme", None),
            DbCommand::CreatePetal { fractal_id, name }
                if fractal_id == "fractal-1" && name == "Acme"
        ));
        // L-06: a node with no spawn_position falls back to the origin
        // (unchanged legacy behavior); with one, it spawns there instead.
        assert!(matches!(
            build_create_command(CreateKind::Node, "petal-1", "Acme", None),
            DbCommand::CreateNode { position, .. } if position == [0.0, 0.0, 0.0]
        ));
        assert!(matches!(
            build_create_command(CreateKind::Node, "petal-1", "Acme", Some([1.0, 2.0, 3.0])),
            DbCommand::CreateNode { position, .. } if position == [1.0, 2.0, 3.0]
        ));
    }
}
