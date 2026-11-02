//! Desktop invitation coordination; see AGENTS.md.

mod issue;
pub(crate) use issue::IssueState;

use bevy::prelude::*;
use fe_runtime::{app::DbCommandSender, messages::DbResult};

pub(crate) fn drive(
    mut results: MessageReader<DbResult>,
    mut ui: ResMut<crate::actions::UiManager>,
    db: Res<DbCommandSender>,
    sync: Option<Res<fe_sync::SyncCommandSenderRes>>,
    identity: Option<Res<fe_identity::NodeIdentity>>,
) {
    let Some((target, state)) =
        crate::dialogs::entity_access::invitation_context(&mut ui.active_dialog)
    else {
        return;
    };
    let did = identity.as_deref().map(|id| id.did.as_str());
    for result in results.read() {
        state.apply(result, &target, did, sync.as_deref());
    }
    state.drive(&target, did, &db.0, sync.as_deref());
}
