//! Join-verse-by-invite dialog.
//!
//! D-05: the dialog used to close the instant Join was clicked, so a garbage
//! invite failed silently and a valid one gave no confirmation. It now stays
//! open with a spinner while `ui_mgr.join_request.pending`, shows
//! `join_request.error` inline on a typed `JoinVerseFailed` (see
//! `dialogs/AGENTS.md` §join-flow), and only `hierarchy.rs`'s
//! `VerseJoined`/`JoinVerseFailed` handlers `finish` the request — this file
//! only `start`s it on send and `reset`s it on Cancel/close.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::{send_or_toast, UiManager};
use crate::theme;
use fe_runtime::messages::DbCommand;

/// How long a sent Join locks the dialog. Every join replies typed
/// (`VerseJoined`/`JoinVerseFailed`), so this only fires when the DB thread
/// died mid-request — without it Cancel and the close-X would never return.
const JOIN_LOCK_TIMEOUT_SECONDS: f64 = 30.0;

pub fn render_join_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    // Read `join_request` before taking the `active_dialog` sub-borrow below
    // (`invite_buf` borrows `ui_mgr.active_dialog`; `ui_mgr` as a whole is
    // only free again once that borrow's last use has passed).
    let now = ctx.input(|i| i.time);
    let pending = ui_mgr
        .join_request
        .waiting_within(now, JOIN_LOCK_TIMEOUT_SECONDS);
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
    let mut window = crate::ui_shell::layout_policy::bounded_window(ctx, "Join Verse")
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
    window.show_with_targets(ctx, |ui| {
        ui.label("Paste an invite string to join a verse:");
        ui.add_space(4.0);

        ui.add_enabled(
            !pending,
            egui::TextEdit::multiline(invite_buf)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Paste invite string here...")
                .font(egui::TextStyle::Monospace),
        )
        .named(ui, "Verse invite string");

        if let Some(err) = error.as_deref() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).color(theme::STATUS_OFFLINE));
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_join = !pending && !invite_buf.trim().is_empty();
            if ui
                .add_enabled(can_join, egui::Button::new("Join")).observed(ui)
                .clicked()
            {
                invite_to_send = Some(invite_buf.trim().to_string());
            }
            if pending {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label("Joining\u{2026}");
            }
            if ui
                .add_enabled(!pending, egui::Button::new("Cancel")).observed(ui)
                .clicked()
            {
                close = true;
            }
        });
    });

    // `invite_buf`'s borrow into `ui_mgr.active_dialog` ended above, so
    // `ui_mgr` as a whole can be mutated again from here down.
    if let Some(invite_string) = invite_to_send {
        if send_or_toast(
            db_tx,
            DbCommand::JoinVerseByInvite { invite_string },
            ui_mgr,
            now,
            "join the verse",
        ) {
            ui_mgr.join_request.start(now);
        }
    }

    if !still_open || close {
        ui_mgr.join_request.reset();
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

    /// A join whose reply never arrives (DB thread gone) stops locking the
    /// dialog after `JOIN_LOCK_TIMEOUT_SECONDS`: Cancel works again.
    #[test]
    fn join_dialog_unlocks_when_no_reply_arrives() {
        let ctx = egui::Context::default();
        let (tx, _rx) = crossbeam::channel::unbounded();
        let mut ui_mgr = UiManager::default();
        ui_mgr.active_dialog = ActiveDialog::JoinDialog {
            invite_buf: String::new(),
        };
        ui_mgr.join_request.start(0.0);
        let mut time = JOIN_LOCK_TIMEOUT_SECONDS + 1.0;

        run_frame(&ctx, time, Vec::new(), &mut ui_mgr, &tx);
        let window_rect = ctx
            .read_response(egui::Id::new("Join Verse"))
            .map(|r| r.rect)
            .expect("the Join Verse window must register an area response");
        // Sweep the window; with the invite empty, the only control that can
        // act is Cancel (or the close-X), and either one closes the dialog.
        let steps = 10;
        'sweep: for xi in 0..=steps {
            for yi in 0..=steps {
                let x = window_rect.left() + window_rect.width() * (xi as f32 / steps as f32);
                let y = window_rect.top() + window_rect.height() * (yi as f32 / steps as f32);
                click(&ctx, &mut time, egui::pos2(x, y), &mut ui_mgr, &tx);
                if matches!(ui_mgr.active_dialog, ActiveDialog::None) {
                    break 'sweep;
                }
            }
        }
        assert!(matches!(ui_mgr.active_dialog, ActiveDialog::None));
        assert!(!ui_mgr.join_request.pending, "closing resets the request");
    }
}
