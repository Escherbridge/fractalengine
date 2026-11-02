//! Masked addressed Join controls; see dialogs/AGENTS.md section addressed-join.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::theme;
use fe_runtime::join_invite::JoinCredential;
use fe_runtime::messages::DbCommand;

/// How long a sent Join locks the dialog. Every join replies typed
/// (`VerseJoined`/`JoinVerseFailed`), so this only fires when the DB thread
/// died mid-request — without it Cancel and the close-X would never return.
const JOIN_LOCK_TIMEOUT_SECONDS: f64 = 30.0;

const MAX_INVITE_BYTES: usize = 64 * 1024;

struct InviteBuffer<'a> {
    value: &'a mut String,
    rejected: &'a mut bool,
}

impl egui::TextBuffer for InviteBuffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        self.value
    }
    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        if self.value.len().saturating_add(text.len()) > MAX_INVITE_BYTES {
            *self.rejected = true;
            return 0;
        }
        egui::TextBuffer::insert_text(self.value, text, char_index)
    }
    fn delete_char_range(&mut self, range: std::ops::Range<usize>) {
        egui::TextBuffer::delete_char_range(self.value, range);
    }
}

pub fn render_join_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    // Read `join_request` before taking the `active_dialog` sub-borrow below
    // (`invite_buf` borrows `ui_mgr.active_dialog`; `ui_mgr` as a whole is
    // only free again once that borrow's last use has passed).
    let now = ctx.input(|i| i.time);
    let pending = ui_mgr.join_flow.active()
        || ui_mgr
            .join_request
            .waiting_within(now, JOIN_LOCK_TIMEOUT_SECONDS);
    let progress = ui_mgr.join_flow.progress();
    let error = ui_mgr.join_request.error.clone();

    let ActiveDialog::JoinDialog { ref mut invite_buf } = ui_mgr.active_dialog else {
        return;
    };

    let mut oversized = invite_buf.len() > MAX_INVITE_BYTES;
    if oversized {
        invite_buf.clear();
    }
    let mut input_changed = false;
    let mut close = false;
    let mut invite_to_send: Option<String> = None;

    // Cancel stops waiting; admission effects may remain stored.
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
    window = window.open(&mut still_open);
    window.show_with_targets(ctx, |ui| {
        ui.label("Paste your addressed invitation to join its scope:");
        ui.add_space(4.0);

        let mut buffer = InviteBuffer {value: invite_buf, rejected: &mut oversized};
        let response = ui.add_enabled(
            !pending,
            egui::TextEdit::singleline(&mut buffer)
                .id(egui::Id::new("join_credential_input"))
                .min_size(egui::Vec2::splat(crate::object_data::target_size(ui)))
                .password(true)
                .char_limit(64 * 1024)
                .desired_width(f32::INFINITY)
                .hint_text("Paste invite string here...")
                .font(egui::TextStyle::Monospace),
        )
        .named(ui, "Verse invite string");
        input_changed = response.changed() || oversized;

        if oversized {
            ui.colored_label(theme::STATUS_OFFLINE,"Invitation input exceeds the 64 KiB byte limit. Paste a smaller, complete invitation.");
        }
        if let Some(err) = error.as_deref() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).color(theme::STATUS_OFFLINE));
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_join = !pending && !oversized && invite_buf.len() <= MAX_INVITE_BYTES && !invite_buf.trim().is_empty();
            if ui
                .add_enabled(can_join, egui::Button::new("Join"))
                .observed(ui)
                .clicked()
            {
                invite_to_send = Some(invite_buf.trim().to_string());
            }
            if pending {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(progress.unwrap_or("Joining..."));
            }
            if ui
                .add(egui::Button::new("Cancel"))
                .observed(ui)
                .clicked()
            {
                close = true;
            }
        });
        if pending {ui.label("Cancel stops waiting. Accepted metadata and imported capabilities may remain stored.");}
    });

    // `invite_buf`'s borrow into `ui_mgr.active_dialog` ended above, so
    // `ui_mgr` as a whole can be mutated again from here down.
    if input_changed {
        ui_mgr.join_flow.input_changed();
        ui_mgr.join_request.reset();
    }
    if let Some(invite_string) = invite_to_send {
        match JoinCredential::new(invite_string) {
            Ok(credential) => {
                if ui_mgr.join_flow.queue(credential) {
                    ui_mgr.join_request.start(now);
                } else {
                    ui_mgr.join_request.error = Some(
                        "Finish canceling the previous join before submitting another invitation."
                            .into(),
                    );
                }
            }
            _ => {
                ui_mgr.join_request.error = Some(
                    "The invitation was not queued. Finish cancellation or check the input size."
                        .into(),
                )
            }
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
        let input = ctx
            .read_response(egui::Id::new("join_credential_input"))
            .expect("Masked invite editor exposes its actual response");
        assert!(
            input.rect.height() >= 24.0 && input.rect.width() >= 24.0,
            "Single-line Join input keeps the actual logical target floor"
        );

        let window_rect = ctx
            .read_response(egui::Id::new("Join Verse"))
            .map(|r| r.rect)
            .expect("the Join Verse window must register an area response");

        assert!(
            !input.enabled(),
            "Join input remains disabled while a request is pending"
        );
        for _ in 0..10 {
            time += 0.05;
            run_frame(&ctx, time, Vec::new(), &mut ui_mgr, &tx);
        }
        assert!(window_rect.is_positive());

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
        if let ActiveDialog::JoinDialog { invite_buf } = &mut ui_mgr.active_dialog {
            *invite_buf = "\u{1f600}".repeat(MAX_INVITE_BYTES / 4 + 1);
        }
        run_frame(&ctx, 0.0, Vec::new(), &mut ui_mgr, &tx);
        assert!(
            matches!(&ui_mgr.active_dialog,ActiveDialog::JoinDialog {invite_buf} if invite_buf.is_empty()),
            "An oversized non-ASCII prefilled credential is discarded before widget/admission"
        );
        let mut draft = String::new();
        let mut rejected = false;
        let mut bounded = InviteBuffer {
            value: &mut draft,
            rejected: &mut rejected,
        };
        assert_eq!(
            egui::TextBuffer::insert_text(
                &mut bounded,
                &"\u{1f600}".repeat(MAX_INVITE_BYTES / 4 + 1),
                0
            ),
            0
        );
        assert!(
            rejected && draft.is_empty(),
            "Byte-bound edits refuse an oversized Unicode paste without truncating a credential"
        );
        assert!(!format!(
            "{:?}",
            ActiveDialog::JoinDialog {
                invite_buf: "credential-sentinel".into()
            }
        )
        .contains("credential-sentinel"));
        let (native_tx, native_rx) = crossbeam::channel::bounded(1);
        native_tx
            .try_send(fe_sync::SyncCommand::CloseVerseReplica {
                verse_id: "old-open".into(),
            })
            .unwrap();
        let native = fe_sync::SyncCommandSenderRes(native_tx);
        let nav = crate::navigation_manager::NavigationManager::default();
        ui_mgr
            .join_flow
            .retain_cleanup(fe_sync::join_ticket::JoinedReplicaLease {
                request_id: "canceled-generation".into(),
                verse_id: "canceled-join".into(),
                namespace_id: "ab".repeat(32),
            });
        ui_mgr.join_flow.cleanup(&nav, Some(&native));
        assert!(
            !ui_mgr
                .join_flow
                .queue(JoinCredential::new("next-invitation".into()).unwrap()),
            "Full cleanup queue retains ownership and gates a new Join"
        );
        assert!(
            matches!(native_rx.try_recv(),Ok(fe_sync::SyncCommand::CloseVerseReplica {verse_id}) if verse_id=="old-open")
        );
        ui_mgr.join_flow.cleanup(&nav, Some(&native));
        assert!(
            matches!(native_rx.try_recv(),Ok(fe_sync::SyncCommand::CloseJoinedReplica {lease}) if lease.verse_id=="canceled-join" && lease.request_id=="canceled-generation"),
            "Cleanup is ordered after the old accepted command"
        );
        assert!(ui_mgr
            .join_flow
            .queue(JoinCredential::new("next-invitation".into()).unwrap()));
        ui_mgr.join_flow.cancel();
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
