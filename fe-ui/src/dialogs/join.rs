//! Masked addressed Join controls; see dialogs/AGENTS.md section addressed-join.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::UiManager;
use crate::theme;
use fe_runtime::join_invite::JoinCredential;
use fe_runtime::messages::DbCommand;

/// Deadline for the dialog's pending projection.
const JOIN_LOCK_TIMEOUT_SECONDS: f64 = 30.0;

const MAX_INVITE_BYTES: usize = 64 * 1024;

struct InviteBuffer<'a> {
    value: &'a mut String,
    rejected: &'a mut bool,
    guarded: Option<(
        &'a egui::Context,
        &'a mut crate::invitations::join_input::JoinInputState,
    )>,
    clip: egui::Rect,
}

impl egui::TextBuffer for InviteBuffer<'_> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<InviteBuffer<'static>>()
    }

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
        if let Some((ctx, guard)) = self.guarded.as_mut() {
            if !guard.insert_allowed(ctx, text, char_index, self.value, self.clip) {
                return 0;
            }
        }
        egui::TextBuffer::insert_text(self.value, text, char_index)
    }
    fn delete_char_range(&mut self, range: std::ops::Range<usize>) {
        if let Some((_, guard)) = self.guarded.as_mut() {
            if !guard.delete_allowed(&range) {
                return;
            }
        }
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

    let join_input = &mut ui_mgr.join_input;
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

        let owned_event = join_input.begin_event(ui.ctx());
        let input_clip = ui.clip_rect();
        let mut buffer = InviteBuffer {
            value: invite_buf,
            rejected: &mut oversized,
            guarded: Some((ctx, &mut *join_input)),
            clip: input_clip,
        };
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
        join_input.end_event(ui.ctx(), owned_event, invite_buf);
        let input_response = response.clone();
        let mut submit_response = None;
        let mut submit_clip = input_clip;
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
            let response = ui
                .push_id("guarded_join_submit", |ui| {
                    ui.add_enabled(can_join, egui::Button::new("Join"))
                })
                .inner
                .observed(ui);
            submit_clip = ui.clip_rect();
            if response.clicked() && join_input.submit_allowed(&response, invite_buf, submit_clip) {
                invite_to_send = Some(invite_buf.trim().to_string());
            }
            submit_response = Some(response);
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
        if let Some(submit) = submit_response {
            join_input.record(&input_response, &submit, input_clip, submit_clip);
        }
        if pending {ui.label("Cancel stops waiting. Accepted metadata, Manager proof and imported capabilities may remain stored.");}
    });

    // `invite_buf`'s borrow into `ui_mgr.active_dialog` ended above, so
    // `ui_mgr` as a whole can be mutated again from here down.
    if input_changed {
        ui_mgr.join_flow.input_changed();
        ui_mgr
            .join_input
            .revision_changed(ui_mgr.join_flow.input_revision());
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
            if ctx.current_pass_index() == 0
                && ctx.data(|data| {
                    data.get_temp::<bool>(egui::Id::new("guard_extra_pass"))
                        .unwrap_or(false)
                })
            {
                ctx.request_discard("guarded Join extra layout pass");
            }
            if ctx.data(|data| {
                data.get_temp::<bool>(egui::Id::new("guard_plain_sink"))
                    .unwrap_or(false)
            }) {
                let id = egui::Id::new("guard_plain_input");
                ctx.memory_mut(|memory| memory.request_focus(id));
                let mut value = ctx.data(|data| data.get_temp::<String>(id).unwrap_or_default());
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut value).id(id));
                });
                ctx.data_mut(|data| data.insert_temp(id, value));
            }
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
            guarded: None,
            clip: egui::Rect::EVERYTHING,
        };
        assert_eq!(
            egui::TextBuffer::type_id(&bounded),
            std::any::TypeId::of::<InviteBuffer<'static>>()
        );
        assert_ne!(
            egui::TextBuffer::type_id(&bounded),
            std::any::TypeId::of::<String>()
        );
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
        ui_mgr
            .join_flow
            .retain_cleanup(fe_sync::join_ticket::JoinedReplicaLease {
                request_id: "canceled-generation".into(),
                verse_id: "canceled-join".into(),
                namespace_id: "ab".repeat(32),
            });
        ui_mgr.join_flow.cleanup(Some(&native));
        assert!(
            !ui_mgr
                .join_flow
                .queue(JoinCredential::new("next-invitation".into()).unwrap()),
            "Full cleanup queue retains ownership and gates a new Join"
        );
        assert!(
            matches!(native_rx.try_recv(),Ok(fe_sync::SyncCommand::CloseVerseReplica {verse_id}) if verse_id=="old-open")
        );
        ui_mgr.join_flow.cleanup(Some(&native));
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
        for mode in [
            "success", "success-multipass", "layout", "focus", "dialog", "stale-pass", "missed-render",
        ] {
            let succeeds = mode.starts_with("success");
            let context = egui::Context::default();
            let mut ui = UiManager::default();
            ui.open_dialog(ActiveDialog::JoinDialog {
                invite_buf: String::new(),
            });
            run_frame(&context, 0.0, vec![], &mut ui, &tx);
            run_frame(&context, 0.01, vec![], &mut ui, &tx);
            let window = fe_runtime::qa_invitation::JoinInputWindow {
                window_id: 1,
                width: 600,
                height: 400,
                scale_bits: 1.0f64.to_bits(),
                focused: true,
            };
            let lease = ui
                .qa_join_input_lease("did:key:recipient", 1, window.clone())
                .expect(
                    "Actual masked Join responses produce an opaque lease without a password name",
                );
            let point = |bounds: [f32; 4]| {
                egui::pos2(bounds[0] + bounds[2] / 2.0, bounds[1] + bounds[3] / 2.0)
            };
            let input_at = point(lease.input_bounds);
            let submit_at = point(lease.join_bounds);
            let input_text = format!("fractalengine://join/v1/{}", "x".repeat(4000));
            let mut spec = fe_runtime::input::InputSpec {
                commands: vec![
                    fe_runtime::input::InputCommand::PointerClick {
                        x: input_at.x,
                        y: input_at.y,
                        button: fe_runtime::input::PointerButton::Left,
                    },
                    fe_runtime::input::InputCommand::TextEntry {
                        text: input_text.clone(),
                    },
                    fe_runtime::input::InputCommand::PointerClick {
                        x: submit_at.x,
                        y: submit_at.y,
                        button: fe_runtime::input::PointerButton::Left,
                    },
                ],
                wait_frames: Some(0),
                capture_after: Some(false),
            }
            .validate()
            .unwrap();
            spec.require_join_lease(&lease.lease_id).unwrap();
            assert!(ui.admit_qa_join_input(
                &lease.lease_id,
                "did:key:recipient",
                1,
                &window,
                &spec
            ));
            if mode == "stale-pass" {
                let _ = context.run(egui::RawInput::default(), |_| {});
                assert!(
                    !ui.prepare_qa_join_input_frame(&lease.lease_id, 0, "did:key:recipient", 1, &window, &context),
                    "A completed foreign pass cannot reuse stale Join widget state"
                );
                assert!(matches!(&ui.active_dialog,ActiveDialog::JoinDialog {invite_buf} if invite_buf.is_empty()));
                assert!(!ui.join_flow.active());
                ui.finish_qa_join_input(&lease.lease_id);
                continue;
            }
            for index in 0..2 {
                assert!(ui.prepare_qa_join_input_frame(
                    &lease.lease_id,
                    index,
                    "did:key:recipient",
                    1,
                    &window,
                    &context
                ));
                assert!(
                    !ui.prepare_qa_join_input_frame(&lease.lease_id, index + 1, "did:key:recipient", 1, &window, &context),
                    "The next input frame must wait for this prepared frame to render and end its pass"
                );
                run_frame(
                    &context,
                    0.1 + index as f64 * 0.1,
                    vec![
                        egui::Event::PointerMoved(input_at),
                        egui::Event::PointerButton {
                            pos: input_at,
                            button: egui::PointerButton::Primary,
                            pressed: index == 0,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    &mut ui,
                    &tx,
                );
            }
            assert!(ui.prepare_qa_join_input_frame(
                &lease.lease_id,
                2,
                "did:key:recipient",
                1,
                &window,
                &context
            ));
            match mode {
                "layout" => {
                    context.style_mut(|style| {
                        style
                            .text_styles
                            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(48.0));
                    });
                }
                "focus" => context
                    .memory_mut(|memory| memory.request_focus(egui::Id::new("guard_plain_input"))),
                "dialog" => ui.open_dialog(ActiveDialog::None),
                "missed-render" => {
                    let _ = context.run(egui::RawInput::default(), |_| {});
                }
                "success-multipass" => context.data_mut(|data| {
                    data.insert_temp(egui::Id::new("guard_extra_pass"), true)
                }),
                _ => {}
            }
            if !succeeds {
                context.data_mut(|data| data.insert_temp(egui::Id::new("guard_plain_sink"), true));
            }
            // No global Text event: the production masked TextEdit owns the private capsule.
            run_frame(&context, 0.3, vec![], &mut ui, &tx);
            context.data_mut(|data| data.insert_temp(egui::Id::new("guard_extra_pass"), false));
            if succeeds {
                assert!(
                    matches!(&ui.active_dialog,ActiveDialog::JoinDialog {invite_buf} if invite_buf==&input_text)
                );
                assert!(
                    !ui.join_flow.active(),
                    "Completed masked insertion enables Join without queuing admission early"
                );
                for index in 3..5 {
                    assert!(ui.prepare_qa_join_input_frame(
                        &lease.lease_id,
                        index,
                        "did:key:recipient",
                        1,
                        &window,
                        &context
                    ));
                    assert!(
                        !ui.prepare_qa_join_input_frame(&lease.lease_id, index + 1, "did:key:recipient", 1, &window, &context),
                        "Join press and release cannot advance without a completed native render"
                    );
                    run_frame(
                        &context,
                        0.4 + (index - 3) as f64 * 0.1,
                        vec![
                            egui::Event::PointerMoved(submit_at),
                            egui::Event::PointerButton {
                                pos: submit_at,
                                button: egui::PointerButton::Primary,
                                pressed: index == 3,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        &mut ui,
                        &tx,
                    );
                }
                assert!(ui.qa_join_input_completed(&lease.lease_id) && ui.join_flow.active(),"Only the actual Join button response queues the ordinary asynchronous Join flow; no Ready is fabricated");
            } else {
                assert!(
                    !ui.qa_join_input_completed(&lease.lease_id)
                        && !ui.prepare_qa_join_input_frame(
                            &lease.lease_id,
                            3,
                            "did:key:recipient",
                            1,
                            &window,
                            &context
                        )
                );
                assert!(context.data(|data|data.get_temp::<String>(egui::Id::new("guard_plain_input")).unwrap_or_default()).is_empty(),"Stale layout/focus/dialog cannot redirect the private credential event into a plain editor");
                assert!(
                    !ui.join_flow.active(),
                    "A stale actual Join response cannot submit the private input"
                );
            }
            ui.finish_qa_join_input(&lease.lease_id);
        }
        let (commands,replies)=crossbeam::channel::bounded(1);
        commands.try_send(DbCommand::LoadHierarchy).unwrap();
        let credential=JoinCredential::new("finalization-correlation-fixture".into()).unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(30);
        let mut gate=crate::invitations::ProofFinalization::default();
        assert!(!gate.submit(&commands,&credential,"generation-A",deadline),"A full bounded queue leaves finalization definitely unsubmitted and never blocks the frame");
        assert!(matches!(replies.try_recv(),Ok(DbCommand::LoadHierarchy)));
        assert!(gate.submit(&commands,&credential,"generation-A",deadline));
        let DbCommand::FinalizeJoinProof {request_id,deadline:actual_deadline,..}=replies.try_recv().unwrap() else {panic!("Only a typed finalization request may be emitted")};
        assert_eq!(actual_deadline,deadline);
        assert!(!gate.submit(&commands,&credential,"generation-A",deadline),"An accepted finalization is never automatically replayed");
        assert!(!gate.confirm("generation-stale-finalize"));
        assert!(gate.confirm(&request_id) && gate.confirmed());
        assert!(!gate.confirm(&request_id),"Duplicate replies cannot complete another phase");
        let mut canceled=crate::invitations::ProofFinalization::default();
        assert!(!canceled.confirm(&request_id),"A canceled/replaced generation cannot accept a late proof receipt");
        assert!(replies.try_recv().is_err());
    }
}
