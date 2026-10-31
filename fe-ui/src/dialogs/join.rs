//! Join-verse-by-invite dialog.
//!
//! D-05: the dialog used to close the instant Join was clicked, so a garbage
//! invite failed silently and a valid one gave no confirmation. It now stays
//! open with a spinner while `ui_mgr.join_request.pending`, shows
//! `join_request.error` inline on a typed `JoinVerseFailed` (see
//! `fe-ui/src/AGENTS.md` §join-flow), and only `hierarchy.rs`'s
//! `VerseJoined`/`JoinVerseFailed` handlers (W1-S2) clear `pending` — this
//! file never clears it itself except on Cancel/close.

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::{send_or_toast, UiManager};
use crate::theme;
use fe_runtime::messages::DbCommand;

pub fn render_join_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    // Read `join_request` before taking the `active_dialog` sub-borrow below
    // (`invite_buf` borrows `ui_mgr.active_dialog`; `ui_mgr` as a whole is
    // only free again once that borrow's last use has passed).
    let pending = ui_mgr.join_request.pending;
    let error = ui_mgr.join_request.error.clone();

    let ActiveDialog::JoinDialog { ref mut invite_buf } = ui_mgr.active_dialog else {
        return;
    };

    let mut close = false;
    let mut invite_to_send: Option<String> = None;

    // No native title-bar close-X while pending: a request in flight must
    // not be abandon-able by accident any more than Cancel/Join/the text
    // edit are (all three are `add_enabled(!pending, ..)` below).
    let mut still_open = true;
    let mut window = egui::Window::new("Join Verse")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        );
    if !pending {
        window = window.open(&mut still_open);
    }
    window.show(ctx, |ui| {
        ui.label("Paste an invite string to join a verse:");
        ui.add_space(4.0);

        ui.add_enabled(
            !pending,
            egui::TextEdit::multiline(invite_buf)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Paste invite string here...")
                .font(egui::TextStyle::Monospace),
        );

        if let Some(err) = error.as_deref() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).color(theme::STATUS_OFFLINE));
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_join = !pending && !invite_buf.trim().is_empty();
            if ui
                .add_enabled(can_join, egui::Button::new("Join"))
                .clicked()
            {
                invite_to_send = Some(invite_buf.trim().to_string());
            }
            if pending {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label("Joining\u{2026}");
            }
            if ui
                .add_enabled(!pending, egui::Button::new("Cancel"))
                .clicked()
            {
                close = true;
            }
        });
    });

    // `invite_buf`'s borrow into `ui_mgr.active_dialog` ended above, so
    // `ui_mgr` as a whole can be mutated again from here down.
    if let Some(invite_string) = invite_to_send {
        let now = ctx.input(|i| i.time);
        if send_or_toast(
            db_tx,
            DbCommand::JoinVerseByInvite { invite_string },
            ui_mgr,
            now,
            "join the verse",
        ) {
            ui_mgr.join_request.pending = true;
            ui_mgr.join_request.error = None;
        }
    }

    if !still_open || close {
        ui_mgr.join_request.pending = false;
        ui_mgr.join_request.error = None;
        ui_mgr.close_dialog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        ui_mgr: &mut UiManager,
        db_tx: &crossbeam::channel::Sender<DbCommand>,
    ) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            render_join_dialog(ctx, ui_mgr, db_tx);
        });
    }

    fn click(
        ctx: &egui::Context,
        time: &mut f64,
        at: egui::Pos2,
        ui_mgr: &mut UiManager,
        db_tx: &crossbeam::channel::Sender<DbCommand>,
    ) {
        for event in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            *time += 0.05;
            run_frame(ctx, *time, vec![event], ui_mgr, db_tx);
        }
    }

    #[test]
    fn join_dialog_stays_open_while_pending() {
        let ctx = egui::Context::default();
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut ui_mgr = UiManager::default();
        ui_mgr.active_dialog = ActiveDialog::JoinDialog {
            invite_buf: "some-invite-string".to_string(),
        };
        ui_mgr.join_request.pending = true;
        let mut time = 0.0;

        // Lay out once; `Window::new("Join Verse")` is `Area::new(Id::new(..))`
        // (egui's own id derivation), so its response — and bounding rect —
        // is readable back by that same id without changing the render fn.
        run_frame(&ctx, time, Vec::new(), &mut ui_mgr, &tx);
        let window_rect = ctx
            .read_response(egui::Id::new("Join Verse"))
            .map(|r| r.rect)
            .expect("the Join Verse window must register an area response");

        // Sweep every control inside the window: while pending, nothing in
        // it can fire a send (Join/Cancel/the text edit are all gated on
        // `!pending`), and the dialog never self-closes either.
        let steps = 10;
        for xi in 0..=steps {
            for yi in 0..=steps {
                let x = window_rect.left() + window_rect.width() * (xi as f32 / steps as f32);
                let y = window_rect.top() + window_rect.height() * (yi as f32 / steps as f32);
                click(&ctx, &mut time, egui::pos2(x, y), &mut ui_mgr, &tx);
            }
        }

        assert!(
            rx.try_recv().is_err(),
            "no DbCommand must be sent while a join is already pending"
        );
        assert!(
            matches!(ui_mgr.active_dialog, ActiveDialog::JoinDialog { .. }),
            "the dialog must stay open for the whole pending window"
        );
        assert!(
            ui_mgr.join_request.pending,
            "only the db-result handlers (W1-S2) may clear pending, never this render"
        );
    }
}
