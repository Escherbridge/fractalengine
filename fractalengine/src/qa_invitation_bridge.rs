//! Owner-thread one-use invitation transfer; see src/AGENTS.md section QA invitation.

use bevy::prelude::*;
use fe_runtime::{
    bridge::BridgeRequest,
    join_invite::InviteSource,
    messages::{DbCommand, DbResult},
    qa_debug::QaFailure,
    qa_invitation::{
        TransferCommand, TransferMetadata, TransferOperation, TransferOutcome, TransferReceiver,
        TransferReply,
    },
};
use fe_ui::{actions::UiManager, navigation_manager::NavigationManager};

#[derive(Resource)]
struct Host {
    receiver: TransferReceiver,
    pending: Option<Pending>,
}
struct Pending {
    request: BridgeRequest<TransferCommand, TransferOutcome>,
    expected: TransferMetadata,
    db_request: String,
    navigation_revision: u64,
}

pub fn install(app: &mut App, enabled: bool, receiver: TransferReceiver) {
    if !enabled {
        return;
    }
    app.insert_resource(Host {
        receiver,
        pending: None,
    });
    app.add_systems(
        PostUpdate,
        serve
            .after(bevy_egui::EguiPostUpdateSet::ProcessOutput)
            .run_if(fe_runtime::maintenance::editor_running),
    );
}
fn denied() -> QaFailure {
    QaFailure::denied()
}
fn scope_covers(caller: &str, target: &str) -> bool {
    let Ok(caller) = fe_database::parse_scope(caller) else {
        return false;
    };
    let Ok(target) = fe_database::parse_scope(target) else {
        return false;
    };
    caller.verse_id == target.verse_id
        && caller
            .fractal_id
            .as_ref()
            .is_none_or(|id| Some(id) == target.fractal_id.as_ref())
        && caller
            .petal_id
            .as_ref()
            .is_none_or(|id| Some(id) == target.petal_id.as_ref())
}
fn current(
    ui: &mut UiManager,
    issuer: &str,
    nav: &NavigationManager,
    scope: &str,
    expected: &TransferMetadata,
) -> bool {
    nav.active_verse_id.as_deref() == Some(expected.verse_id.as_str())
        && scope_covers(scope, &expected.scope)
        && ui
            .qa_invitation_metadata(issuer, nav.intent_revision)
            .as_ref()
            == Some(expected)
}
fn serve(
    mut host: ResMut<Host>,
    mut ui: ResMut<UiManager>,
    nav: Res<NavigationManager>,
    identity: Option<Res<fe_identity::NodeIdentity>>,
    db: Res<fe_runtime::app::DbCommandSender>,
    mut results: MessageReader<DbResult>,
    view: crate::vision_bridge::ViewScope,
) {
    let Some(identity) = identity else {
        host.pending = None;
        for request in host.receiver.try_iter().take(1) {
            request.reply(Err(denied()));
        }
        return;
    };
    let issuer = identity.did.as_str();
    if let Some(pending) = host.pending.as_ref() {
        if pending.request.reply_tx.is_closed()
            || !pending.request.op.is_current()
            || pending.request.op.issuer_did != issuer
            || pending.navigation_revision != nav.intent_revision
            || !view.covers(&pending.request.caller_scope, Some(&nav))
            || !current(
                &mut ui,
                issuer,
                &nav,
                &pending.request.caller_scope,
                &pending.expected,
            )
        {
            if let Some(pending) = host.pending.take() {
                pending.request.reply(Err(denied()));
            }
        }
    }
    for result in results.read() {
        let DbResult::JoinInvitePrepared { request_id, result } = result else {
            continue;
        };
        if !host
            .pending
            .as_ref()
            .is_some_and(|pending| pending.db_request == *request_id)
        {
            continue;
        }
        let Some(pending) = host.pending.take() else {
            continue;
        };
        if pending.request.reply_tx.is_closed()
            || !pending.request.op.is_current()
            || pending.navigation_revision != nav.intent_revision
            || !view.covers(&pending.request.caller_scope, Some(&nav))
            || !current(
                &mut ui,
                issuer,
                &nav,
                &pending.request.caller_scope,
                &pending.expected,
            )
        {
            pending.request.reply(Err(denied()));
            continue;
        }
        let credential = result.as_ref().ok().and_then(|fresh: &InviteSource| {
            ui.consume_qa_invitation(&pending.expected, fresh, issuer, nav.intent_revision)
        });
        let outcome = credential
            .map(|credential| TransferReply::Consumed {
                metadata: pending.expected,
                credential,
            })
            .ok_or_else(denied);
        pending.request.reply(outcome);
    }
    let Ok(request) = host.receiver.try_recv() else {
        return;
    };
    if host.pending.is_some() {
        request.reply(Err(QaFailure {
            code: "invitation_transfer_busy",
            message: "An invitation transfer is already pending",
        }));
        return;
    }
    if request.reply_tx.is_closed()
        || !request.op.is_current()
        || request.op.issuer_did != issuer
        || !view.covers(&request.caller_scope, Some(&nav))
    {
        request.reply(Err(denied()));
        return;
    }
    match &request.op.operation {
        TransferOperation::Observe => {
            let invitation = ui
                .qa_invitation_metadata(issuer, nav.intent_revision)
                .filter(|metadata| {
                    nav.active_verse_id.as_deref() == Some(metadata.verse_id.as_str())
                        && scope_covers(&request.caller_scope, &metadata.scope)
                });
            request.reply(Ok(TransferReply::Observed {
                local_did: issuer.into(),
                invitation,
                join_ready_for_input: ui.qa_join_ready_for_input(),
            }));
        }
        TransferOperation::Consume(expected) => {
            if !expected.validate()
                || !current(&mut ui, issuer, &nav, &request.caller_scope, expected)
            {
                request.reply(Err(denied()));
                return;
            }
            let Some(source) = ui.qa_invitation_source(expected, issuer, nav.intent_revision)
            else {
                request.reply(Err(denied()));
                return;
            };
            let db_request = format!("qa-invitation-{}", ulid::Ulid::new());
            if db
                .0
                .try_send(DbCommand::PrepareJoinInvite {
                    target: source.target,
                    recipient_did: source.recipient_did,
                    role: source.role,
                    expiry_hours: source.expiry_hours,
                    request_id: db_request.clone(),
                })
                .is_err()
            {
                request.reply(Err(QaFailure {
                    code: "invitation_transfer_busy",
                    message: "Current Owner verification could not be queued",
                }));
                return;
            }
            let expected = expected.clone();
            host.pending = Some(Pending {
                request,
                expected,
                db_request,
                navigation_revision: nav.intent_revision,
            });
        }
    }
}
