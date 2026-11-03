//! Desktop invitation coordination; see AGENTS.md.

mod issue;
mod join;
pub(crate) mod join_input;
pub(crate) use issue::IssueState;
pub(crate) use join::JoinState;

use bevy::prelude::*;
use fe_runtime::{app::DbCommandSender, messages::DbResult};

#[derive(Resource, Default)]
pub(crate) struct ReplicaHandoff {
    pending: Option<fe_sync::join_ticket::JoinedReplicaLease>,
    disconnected_reported: bool,
}

impl ReplicaHandoff {
    pub(crate) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn install(&mut self, lease: fe_sync::join_ticket::JoinedReplicaLease) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.pending = Some(lease);
        self.disconnected_reported = false;
        true
    }
    fn matches(&self, verse: &str, hierarchy: &crate::verse_manager::VerseManager) -> bool {
        self.pending.as_ref().is_some_and(|lease| {
            lease.verse_id == verse
                && hierarchy.verses.iter().any(|v| {
                    v.id == lease.verse_id
                        && v.namespace_id.as_deref() == Some(lease.namespace_id.as_str())
                })
        })
    }
    pub(crate) fn take_for(
        &mut self,
        verse: &str,
        hierarchy: &crate::verse_manager::VerseManager,
    ) -> bool {
        if !self.matches(verse, hierarchy) {
            return false;
        }
        self.pending = None;
        true
    }
    pub(crate) fn cleanup_superseded(
        &mut self,
        nav: &crate::navigation_manager::NavigationManager,
        hierarchy: &crate::verse_manager::VerseManager,
        sync: &fe_sync::SyncCommandSenderRes,
    ) -> bool {
        if !self.pending()
            || nav
                .active_verse_id
                .as_deref()
                .is_some_and(|verse| self.matches(verse, hierarchy))
        {
            return true;
        }
        let Some(lease) = self.pending.as_ref() else {
            return true;
        };
        match sync.0.try_send(fe_sync::SyncCommand::CloseJoinedReplica {
            lease: lease.clone(),
        }) {
            Ok(()) => {
                self.pending = None;
                self.disconnected_reported = false;
                true
            }
            Err(crossbeam::channel::TrySendError::Full(_)) => false,
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                if !self.disconnected_reported {
                    bevy::log::warn!(
                        "Superseded joined-replica cleanup is unconfirmed: sync channel closed"
                    );
                    self.disconnected_reported = true;
                }
                false
            }
        }
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
    join.cleanup(sync.as_deref());
    for result in results.read() {
        if let Some((target, state)) =
            crate::dialogs::entity_access::invitation_context(&mut ui.active_dialog)
        {
            state.bind_navigation(nav.intent_revision());
            state.apply(result, &target, did, sync.as_deref());
        }
        join.apply(result);
    }
    if let Some((target, state)) =
        crate::dialogs::entity_access::invitation_context(&mut ui.active_dialog)
    {
        state.bind_navigation(nav.intent_revision());
        state.drive(&target, did, &db.0, sync.as_deref());
    }
    let ready = if handoff.pending() {
        join.wait_for_handoff();
        None
    } else {
        join.drive(&nav, &hierarchy, did, &db.0, sync.as_deref())
    };
    if let Some(ready) = ready {
        let acquired = nav.active_verse_id.as_deref() == Some(ready.scope.verse_id.as_str())
            || handoff.install(ready.lease.clone());
        if !acquired {
            join.retain_cleanup(ready.lease);
            join.error=Some("Native navigation handoff was superseded. Join remains unconfirmed; cleanup is pending.");
            ui.join_request.pending = join.active();
            ui.join_request.error = join.error.map(str::to_owned);
            ui.join_flow = join;
            return;
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
    join.cleanup(sync.as_deref());
    ui.join_request.pending = join.active();
    ui.join_request.error = join.error.map(str::to_owned);
    ui.join_flow = join;
}
