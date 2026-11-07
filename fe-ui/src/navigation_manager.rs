//! Navigation cursor and replica lifecycle; see src/AGENTS.md §navigation-ancestry.

use bevy::prelude::*;

use crate::verse_manager::VerseManager;

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct NavigationManager {
    pub active_verse_id: Option<String>,
    pub active_verse_name: String,
    pub active_fractal_id: Option<String>,
    pub active_fractal_name: String,
    pub active_petal_id: Option<String>,
    /// Manager-owned revision; public for existing cross-crate struct updates.
    #[doc(hidden)]
    pub intent_revision: u64,
    /// Set through manager methods; public for existing struct-update callers.
    #[doc(hidden)]
    pub initial_auto_navigation_suppressed: bool,
}

impl NavigationManager {
    /// Startup may choose a cursor only before navigation intent or hydration.
    pub fn initial_auto_navigation_allowed(&self) -> bool {
        !self.initial_auto_navigation_suppressed && self.active_verse_id.is_none()
    }

    /// Keep read-only hydration from choosing a cursor; this is not user intent.
    pub fn suppress_initial_auto_navigation(&mut self) {
        self.initial_auto_navigation_suppressed = true;
    }

    pub(crate) fn intent_revision(&self) -> u64 {
        self.intent_revision
    }

    fn record_intent(&mut self) {
        self.intent_revision = self.intent_revision.wrapping_add(1);
    }

    /// Enter a verse. Clears fractal and petal selection.
    pub fn navigate_to_verse(&mut self, id: impl Into<String>, name: impl Into<String>) {
        self.record_intent();
        self.suppress_initial_auto_navigation();
        self.active_verse_id = Some(id.into());
        self.active_verse_name = name.into();
        self.active_fractal_id = None;
        self.active_fractal_name.clear();
        self.active_petal_id = None;
    }

    /// Enter a fractal within the current verse. Clears petal selection.
    pub fn navigate_to_fractal(&mut self, id: impl Into<String>, name: impl Into<String>) {
        self.record_intent();
        self.suppress_initial_auto_navigation();
        self.active_fractal_id = Some(id.into());
        self.active_fractal_name = name.into();
        self.active_petal_id = None;
    }

    /// Enter a petal within the current fractal.
    pub fn navigate_to_petal(&mut self, id: impl Into<String>) {
        self.record_intent();
        self.suppress_initial_auto_navigation();
        self.active_petal_id = Some(id.into());
    }

    /// Enter a fractal with its containing verse.
    pub fn navigate_to_fractal_in_verse(
        &mut self,
        verse_id: impl Into<String>,
        verse_name: impl Into<String>,
        fractal_id: impl Into<String>,
        fractal_name: impl Into<String>,
    ) {
        self.navigate_to_verse(verse_id, verse_name);
        self.navigate_to_fractal(fractal_id, fractal_name);
    }

    /// Enter a petal with its complete ancestry.
    pub fn navigate_to_petal_in_fractal(
        &mut self,
        verse_id: impl Into<String>,
        verse_name: impl Into<String>,
        fractal_id: impl Into<String>,
        fractal_name: impl Into<String>,
        petal_id: impl Into<String>,
    ) {
        self.navigate_to_fractal_in_verse(verse_id, verse_name, fractal_id, fractal_name);
        self.navigate_to_petal(petal_id);
    }

    /// Whether every selected descendant has its required parent.
    pub fn has_valid_ancestry(&self) -> bool {
        (self.active_fractal_id.is_none() || self.active_verse_id.is_some())
            && (self.active_petal_id.is_none()
                || (self.active_fractal_id.is_some() && self.active_verse_id.is_some()))
    }

    /// The current scope, absent for an empty or incomplete cursor.
    pub fn active_scope(&self) -> Option<String> {
        if !self.has_valid_ancestry() {
            return None;
        }
        self.active_verse_id.as_deref().map(|verse_id| {
            fe_database::build_scope(
                verse_id,
                self.active_fractal_id.as_deref(),
                self.active_petal_id.as_deref(),
            )
        })
    }

    pub fn back_from_petal(&mut self) {
        self.record_intent();
        self.suppress_initial_auto_navigation();
        self.active_petal_id = None;
    }

    pub fn back_from_fractal(&mut self) {
        self.record_intent();
        self.suppress_initial_auto_navigation();
        self.active_fractal_id = None;
        self.active_fractal_name.clear();
        self.active_petal_id = None;
    }

    pub fn back_from_verse(&mut self) {
        self.record_intent();
        self.suppress_initial_auto_navigation();
        self.active_verse_id = None;
        self.active_verse_name.clear();
        self.active_fractal_id = None;
        self.active_fractal_name.clear();
        self.active_petal_id = None;
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct NavigationManagerPlugin;

impl Plugin for NavigationManagerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavigationManager>();
        app.add_systems(Update, handle_verse_replica_lifecycle);
    }
}

// ---------------------------------------------------------------------------
// System: open / close P2P verse replica when active verse changes
// ---------------------------------------------------------------------------

pub(crate) fn handle_verse_replica_lifecycle(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    secret_store: Option<Res<fe_database::SecretStoreRes>>,
    mut last_verse: Local<Option<String>>,
    mut joined: Option<ResMut<crate::invitations::ReplicaHandoff>>,
) {
    let Some(ref sync) = sync_sender else {
        return;
    };
    if joined
        .as_mut()
        .is_some_and(|handoff| !handoff.cleanup_superseded(&nav, &verse_mgr, sync))
    {
        return;
    }
    if *last_verse == nav.active_verse_id {
        if let Some(verse) = nav.active_verse_id.as_deref() {
            if let Some(handoff) = joined.as_mut() {
                handoff.take_for(verse, &verse_mgr);
            }
        }
        return;
    }
    if let Some(old) = last_verse.as_ref() {
        match sync.0.try_send(fe_sync::SyncCommand::CloseVerseReplica {
            verse_id: old.clone(),
        }) {
            Ok(()) => *last_verse = None,
            Err(crossbeam::channel::TrySendError::Full(_)) => return,
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                bevy::log::error!(
                    "Sync channel closed while closing the previous navigation replica"
                );
                return;
            }
        }
    }
    if let Some(new_verse) = nav.active_verse_id.as_deref() {
        let adopted = joined
            .as_mut()
            .is_some_and(|handoff| handoff.take_for(new_verse, &verse_mgr));
        if adopted || open_replica(sync, new_verse, &verse_mgr, secret_store.as_deref()) {
            *last_verse = Some(new_verse.into());
        }
    }
}

fn open_replica(
    sync: &fe_sync::SyncCommandSenderRes,
    verse_id: &str,
    verse_mgr: &VerseManager,
    secret_store: Option<&fe_database::SecretStoreRes>,
) -> bool {
    let Some(ns_id) = verse_mgr
        .verses
        .iter()
        .find(|v| v.id == verse_id)
        .and_then(|v| v.namespace_id.clone())
    else {
        // F22: warn instead of silently returning. A verse with no
        // namespace_id in the tree (or absent from it) cannot open a replica,
        // so every write for it this session is retained by the sync thread
        // and dropped at shutdown — the permanent-loss shape F21/F22 exist to
        // prevent. `DbResult::VerseCreated` now populates the tree entry, so
        // this should not fire on the normal create-then-navigate flow.
        bevy::log::warn!(
            "open_replica: verse {verse_id} has no namespace_id in the tree — \
             replica not opened; its writes will not publish this session"
        );
        return true;
    };
    let ns_secret = secret_store
        .and_then(|ss| fe_database::get_namespace_secret(ss.0.as_ref(), verse_id).ok())
        .flatten();
    match sync.0.try_send(fe_sync::SyncCommand::OpenVerseReplica {
        verse_id: verse_id.into(),
        namespace_id: ns_id,
        namespace_secret: ns_secret,
        bootstrap_peers: Vec::new(),
    }) {
        Ok(()) => true,
        Err(crossbeam::channel::TrySendError::Full(_)) => false,
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            bevy::log::error!("Sync channel closed while opening the navigation replica");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigate_to_verse_clears_fractal_and_petal() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.navigate_to_verse("v2", "Verse 2");
        assert_eq!(nav.active_verse_id, Some("v2".to_string()));
        assert_eq!(nav.active_verse_name, "Verse 2");
        assert!(nav.active_fractal_id.is_none(), "fractal should be cleared");
        assert!(
            nav.active_fractal_name.is_empty(),
            "fractal name should be cleared"
        );
        assert!(nav.active_petal_id.is_none(), "petal should be cleared");
    }

    #[test]
    fn navigate_to_fractal_clears_petal() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.navigate_to_fractal("f2", "Fractal 2");
        assert_eq!(nav.active_fractal_id, Some("f2".to_string()));
        assert_eq!(nav.active_fractal_name, "Fractal 2");
        assert!(
            nav.active_petal_id.is_none(),
            "petal should be cleared on fractal change"
        );
        assert_eq!(nav.active_verse_id, Some("v1".to_string()));

        nav.navigate_to_petal_in_fractal("v2", "Verse 2", "f3", "Fractal 3", "p3");
        assert_eq!(nav.active_verse_id.as_deref(), Some("v2"));
        assert_eq!(nav.active_verse_name, "Verse 2");
        assert_eq!(nav.active_fractal_name, "Fractal 3");
        assert_eq!(
            nav.active_scope().as_deref(),
            Some("VERSE#v2-FRACTAL#f3-PETAL#p3")
        );
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_petal_in_fractal("v2", "Verse 2", "f3", "Fractal 3", "p3");
        assert!(
            nav.has_valid_ancestry(),
            "expanded petal restores every ancestor"
        );
        assert_eq!(
            nav.active_scope().as_deref(),
            Some("VERSE#v2-FRACTAL#f3-PETAL#p3")
        );
        nav.navigate_to_fractal_in_verse("v3", "Verse 3", "f4", "Fractal 4");
        assert_eq!(nav.active_scope().as_deref(), Some("VERSE#v3-FRACTAL#f4"));
        assert!(nav.active_petal_id.is_none());
    }

    #[test]
    fn back_from_verse_clears_all_fields() {
        let mut nav = NavigationManager::default();
        assert!(nav.initial_auto_navigation_allowed());
        let revision = nav.intent_revision();
        nav.suppress_initial_auto_navigation();
        assert_eq!(
            nav.intent_revision(),
            revision,
            "Read-only hydration suppression preserves the independent intent revision"
        );
        assert!(!nav.initial_auto_navigation_allowed());
        assert!(
            nav.active_verse_id.is_none()
                && nav.active_fractal_id.is_none()
                && nav.active_petal_id.is_none(),
            "hydration suppression does not fabricate cursor intent"
        );
        nav = NavigationManager::default();
        nav.back_from_verse();
        assert!(
            !nav.initial_auto_navigation_allowed(),
            "Back before startup data prevents automatic re-entry"
        );
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.back_from_verse();
        assert!(!nav.initial_auto_navigation_allowed());
        assert!(nav.active_verse_id.is_none());
        assert!(nav.active_verse_name.is_empty());
        assert!(nav.active_fractal_id.is_none());
        assert!(nav.active_fractal_name.is_empty());
        assert!(nav.active_petal_id.is_none());
    }

    #[test]
    fn back_from_fractal_clears_fractal_and_descendants() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.back_from_fractal();
        assert_eq!(nav.active_verse_id, Some("v1".to_string()));
        assert!(nav.active_fractal_id.is_none());
        assert!(nav.active_fractal_name.is_empty());
        assert!(nav.active_petal_id.is_none());
        assert_eq!(nav.active_scope().as_deref(), Some("VERSE#v1"));
    }

    #[test]
    fn back_from_petal_clears_only_petal() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.back_from_petal();
        assert_eq!(nav.active_verse_id, Some("v1".to_string()));
        assert_eq!(nav.active_fractal_id, Some("f1".to_string()));
        assert!(
            nav.active_petal_id.is_none(),
            "only petal should be cleared"
        );
    }

    /// F22: a verse whose tree entry has no `namespace_id` — the state before
    /// `DbResult::VerseCreated` populates it, or a verse that never received
    /// one — must NOT open a replica. `open_replica` now warns instead of
    /// silently returning; no `OpenVerseReplica` is sent. The positive
    /// create-then-navigate flow (the `VerseCreated` result populates the id,
    /// then navigation opens the replica) is pinned end-to-end in
    /// `verse_manager::db_results::tests::fresh_created_verse_then_navigation_opens_replica`.
    #[test]
    fn navigation_to_verse_without_namespace_id_does_not_open_replica() {
        use crate::verse_manager::VerseEntry;
        use fe_identity::{InMemoryBackend, SecretStore};
        use std::sync::Arc;

        let verse_id = "01F22NOOPENVARSE0000000000";
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());

        let (sync_tx, sync_rx) = crossbeam::channel::bounded(4);
        let mut app = App::new();
        app.init_resource::<NavigationManager>();
        app.insert_resource(crate::verse_manager::VerseManager::from_verses(vec![
            VerseEntry {
                id: verse_id.to_string(),
                name: "No Namespace Verse".to_string(),
                namespace_id: None,
                expanded: true,
                fractals: Vec::new(),
            },
        ]));
        app.insert_resource(fe_sync::SyncCommandSenderRes(sync_tx));
        app.insert_resource(fe_database::SecretStoreRes(store));
        app.add_systems(Update, handle_verse_replica_lifecycle);

        // Frame 1: nothing active — the lifecycle initializes without opening.
        app.update();
        assert!(
            sync_rx.try_recv().is_err(),
            "no open command before navigation"
        );

        // Frame 2: navigating into a verse with no namespace_id must not send
        // an open (the warn branch) — pre-F22 this returned silently, which
        // hid the missing-open hole.
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse(verse_id, "No Namespace Verse");
        app.update();
        assert!(
            sync_rx.try_recv().is_err(),
            "a verse without a namespace_id must not open a replica"
        );
        use crate::invitations::ReplicaHandoff;
        use fe_sync::{join_ticket::JoinedReplicaLease, SyncCommand};
        let old_join = JoinedReplicaLease {
            request_id: "ready-B-generation".into(),
            verse_id: "B".into(),
            namespace_id: "ab".repeat(32),
        };
        let mut handoff = ReplicaHandoff::default();
        assert!(handoff.install(old_join.clone()));
        let mut hierarchy = app.world_mut().resource_mut::<VerseManager>();
        for (id, namespace) in [("B", "ab".repeat(32)), ("C", "cd".repeat(32))] {
            hierarchy.verses.push(VerseEntry {
                id: id.into(),
                name: id.into(),
                namespace_id: Some(namespace),
                expanded: true,
                fractals: Vec::new(),
            });
        }
        assert!(
            !handoff.take_for("C", &hierarchy) && handoff.pending(),
            "A mismatched take cannot discard the ready-B ownership lease"
        );
        app.insert_resource(handoff);
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse("B", "B");
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse("C", "C");
        let sender = app
            .world()
            .resource::<fe_sync::SyncCommandSenderRes>()
            .0
            .clone();
        for i in 0..4 {
            sender
                .try_send(SyncCommand::CloseVerseReplica {
                    verse_id: format!("occupied-{i}"),
                })
                .unwrap();
        }
        app.update();
        assert!(
            app.world().resource::<ReplicaHandoff>().pending(),
            "Full bounded queue retains superseded B while native lifecycle returns"
        );
        for _ in 0..4 {
            assert!(matches!(
                sync_rx.try_recv(),
                Ok(SyncCommand::CloseVerseReplica { .. })
            ));
        }
        app.update();
        assert!(
            matches!(sync_rx.try_recv(),Ok(SyncCommand::CloseJoinedReplica {lease}) if lease==old_join),
            "Exact B generation is cleaned before the unrelated current C is opened"
        );
        assert!(
            matches!(sync_rx.try_recv(),Ok(SyncCommand::CloseVerseReplica {verse_id:v}) if v==verse_id)
        );
        assert!(
            matches!(sync_rx.try_recv(),Ok(SyncCommand::OpenVerseReplica {verse_id,namespace_id,..}) if verse_id=="C" && namespace_id=="cd".repeat(32)),
            "Independently selected C is opened and never receives B's conditional close"
        );
        assert!(!app.world().resource::<ReplicaHandoff>().pending() && sync_rx.try_recv().is_err());
        app.update();
        assert!(
            sync_rx.try_recv().is_err(),
            "Accepted cleanup/open commands are not replayed"
        );
        let mut same_current = crate::invitations::JoinState::default();
        assert!(
            !same_current.check_target_available("C", app.world().resource::<NavigationManager>())
        );
        same_current.cancel();
        same_current.cleanup(Some(
            app.world().resource::<fe_sync::SyncCommandSenderRes>(),
        ));
        app.update();
        assert!(sync_rx.try_recv().is_err() && same_current.error.is_some(),"Joining the already recorded current Verse is refused before admission/native replacement; Cancel leaves its navigation pipeline alone");
        let mut canceled = crate::invitations::JoinState::default();
        let canceled_lease = JoinedReplicaLease {
            request_id: "canceled-B-generation".into(),
            verse_id: "B".into(),
            namespace_id: "ab".repeat(32),
        };
        canceled.retain_cleanup(canceled_lease.clone());
        let sync = app
            .world()
            .resource::<fe_sync::SyncCommandSenderRes>()
            .clone();
        for i in 0..4 {
            sync.0
                .try_send(SyncCommand::CloseVerseReplica {
                    verse_id: format!("occupied-cancel-{i}"),
                })
                .unwrap();
        }
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse("B", "B selected before adoption");
        canceled.cleanup(Some(&sync));
        assert!(
            !canceled
                .queue(fe_runtime::join_invite::JoinCredential::new("next-join".into()).unwrap()),
            "A cursor temporarily pointing to B does not discard its canceled native cleanup lease"
        );
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_verse("C", "Independent C");
        for _ in 0..4 {
            assert!(matches!(
                sync_rx.try_recv(),
                Ok(SyncCommand::CloseVerseReplica { .. })
            ));
        }
        canceled.cleanup(Some(&sync));
        app.update();
        assert!(matches!(sync_rx.try_recv(),Ok(SyncCommand::CloseJoinedReplica {lease}) if lease==canceled_lease),"Canceled B is conditionally closed after transient B then C, even when lifecycle never adopted B");
        assert!(
            sync_rx.try_recv().is_err()
                && app
                    .world()
                    .resource::<NavigationManager>()
                    .active_verse_id
                    .as_deref()
                    == Some("C"),
            "The already owned C pipeline receives no cleanup or reopen"
        );
        assert!(app
            .world_mut()
            .resource_mut::<ReplicaHandoff>()
            .install(old_join));
        crate::maintenance::clear_read_models(app.world_mut());
        assert!(
            !app.world().resource::<ReplicaHandoff>().pending(),
            "Terminal invalidation clears the separately owned native handoff lease"
        );
        assert!(app
            .world()
            .resource::<NavigationManager>()
            .active_verse_id
            .is_none());
    }
}
