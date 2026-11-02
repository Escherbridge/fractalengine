//! Desktop invitation coordination; see AGENTS.md.

mod issue;
mod join;
pub(crate) use issue::IssueState;
pub(crate) use join::JoinState;

use bevy::prelude::*;
use fe_runtime::{app::DbCommandSender, messages::DbResult};

#[derive(Resource, Default)]
pub(crate) struct ReplicaHandoff(Option<(String, String)>);

impl ReplicaHandoff {
    pub(crate) fn take_for(
        &mut self,
        verse: &str,
        hierarchy: &crate::verse_manager::VerseManager,
    ) -> bool {
        self.0.take().is_some_and(|(id, namespace)| {
            id == verse
                && hierarchy
                    .verses
                    .iter()
                    .any(|v| v.id == id && v.namespace_id.as_deref() == Some(namespace.as_str()))
        })
    }
}

pub(crate) fn drive(
    mut results: MessageReader<DbResult>,
    mut ui: ResMut<crate::actions::UiManager>,
    db: Res<DbCommandSender>,
    sync: Option<Res<fe_sync::SyncCommandSenderRes>>,
    identity: Option<Res<fe_identity::NodeIdentity>>,
    mut nav: ResMut<crate::navigation_manager::NavigationManager>,
    hierarchy: Res<crate::verse_manager::VerseManager>,
    mut handoff: ResMut<ReplicaHandoff>,
    clock: Option<Res<crate::notice::NoticeClock>>,
) {
    let did = identity.as_deref().map(|id| id.did.as_str());
    let mut join = std::mem::take(&mut ui.join_flow);
    join.synchronize(&ui.active_dialog, &nav, did);
    join.cleanup(&nav, sync.as_deref());
    for result in results.read() {
        if let Some((target, state)) =
            crate::dialogs::entity_access::invitation_context(&mut ui.active_dialog)
        {
            state.apply(result, &target, did, sync.as_deref());
        }
        join.apply(result);
    }
    if let Some((target, state)) =
        crate::dialogs::entity_access::invitation_context(&mut ui.active_dialog)
    {
        state.drive(&target, did, &db.0, sync.as_deref());
    }
    if let Some(ready) = join.drive(&nav, &hierarchy, did, &db.0, sync.as_deref()) {
        if nav.active_verse_id.as_deref() != Some(ready.scope.verse_id.as_str()) {
            handoff.0 = Some((ready.scope.verse_id.clone(), ready.namespace));
        }
        if let Some(fractal) = ready.scope.fractal_id {
            if let Some(petal) = ready.scope.petal_id {
                nav.navigate_to_petal_in_fractal(
                    &ready.scope.verse_id,
                    &ready.verse_name,
                    &fractal,
                    ready.fractal_name.as_deref().unwrap_or_default(),
                    petal,
                );
            } else {
                nav.navigate_to_fractal_in_verse(
                    &ready.scope.verse_id,
                    &ready.verse_name,
                    fractal,
                    ready.fractal_name.unwrap_or_default(),
                );
            }
        } else {
            nav.navigate_to_verse(&ready.scope.verse_id, &ready.verse_name);
        }
        ui.close_dialog();
        ui.show_toast(
            format!("Joined {}", ready.verse_name),
            clock
                .as_deref()
                .map_or(0.0, |clock| clock.elapsed_secs_f64()),
        );
    }
    join.cleanup(&nav, sync.as_deref());
    ui.join_request.pending = join.active();
    ui.join_request.error = join.error.map(str::to_owned);
    ui.join_flow = join;
}
