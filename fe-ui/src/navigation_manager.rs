//! NavigationManager — owns the active verse/fractal/petal cursor and the
//! P2P verse-replica open/close lifecycle.
//!
//! Replaces the old `NavigationState` resource.  All panels that previously
//! wrote directly to `NavigationState` fields now call the navigation methods
//! on this manager, which keeps the mutation semantics identical while giving
//! us a clear owner for the replica side-effects.

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
}

impl NavigationManager {
    /// Enter a verse. Clears fractal and petal selection.
    pub fn navigate_to_verse(&mut self, id: impl Into<String>, name: impl Into<String>) {
        self.active_verse_id = Some(id.into());
        self.active_verse_name = name.into();
        self.active_fractal_id = None;
        self.active_fractal_name.clear();
        self.active_petal_id = None;
    }

    /// Enter a fractal within the current verse. Clears petal selection.
    pub fn navigate_to_fractal(&mut self, id: impl Into<String>, name: impl Into<String>) {
        self.active_fractal_id = Some(id.into());
        self.active_fractal_name = name.into();
        self.active_petal_id = None;
    }

    /// Enter a petal within the current fractal.
    pub fn navigate_to_petal(&mut self, id: impl Into<String>) {
        self.active_petal_id = Some(id.into());
    }

    pub fn back_from_petal(&mut self) {
        self.active_petal_id = None;
    }

    pub fn back_from_fractal(&mut self) {
        self.active_fractal_id = None;
        self.active_fractal_name.clear();
    }

    pub fn back_from_verse(&mut self) {
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

fn handle_verse_replica_lifecycle(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut last_verse: Local<Option<String>>,
    mut initialized: Local<bool>,
) {
    let Some(ref sync) = sync_sender else { return };

    if !*initialized {
        *last_verse = nav.active_verse_id.clone();
        *initialized = true;
        // Open any verse that was already active on startup.
        if let Some(ref vid) = nav.active_verse_id {
            open_replica(sync, vid, &verse_mgr);
        }
        return;
    }

    if *last_verse == nav.active_verse_id {
        return;
    }

    // Close the previous replica.
    if let Some(ref old) = *last_verse {
        sync.0
            .send(fe_sync::SyncCommand::CloseVerseReplica {
                verse_id: old.clone(),
            })
            .ok();
    }

    // Open the new one.
    if let Some(ref new_vid) = nav.active_verse_id {
        open_replica(sync, new_vid, &verse_mgr);
    }

    *last_verse = nav.active_verse_id.clone();
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
        assert!(nav.active_fractal_name.is_empty(), "fractal name should be cleared");
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
        assert!(nav.active_petal_id.is_none(), "petal should be cleared on fractal change");
        assert_eq!(nav.active_verse_id, Some("v1".to_string()));
    }

    #[test]
    fn back_from_verse_clears_all_fields() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.navigate_to_petal("p1");
        nav.back_from_verse();
        assert!(nav.active_verse_id.is_none());
        assert!(nav.active_verse_name.is_empty());
        assert!(nav.active_fractal_id.is_none());
        assert!(nav.active_fractal_name.is_empty());
        assert!(nav.active_petal_id.is_none());
    }

    #[test]
    fn back_from_fractal_clears_only_fractal_and_name() {
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v1", "Verse 1");
        nav.navigate_to_fractal("f1", "Fractal 1");
        nav.back_from_fractal();
        assert_eq!(nav.active_verse_id, Some("v1".to_string()));
        assert!(nav.active_fractal_id.is_none());
        assert!(nav.active_fractal_name.is_empty());
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
        assert!(nav.active_petal_id.is_none(), "only petal should be cleared");
    }
}

fn open_replica(
    sync: &fe_sync::SyncCommandSenderRes,
    verse_id: &str,
    verse_mgr: &VerseManager,
) {
    let Some(ns_id) = verse_mgr
        .verses
        .iter()
        .find(|v| v.id == verse_id)
        .and_then(|v| v.namespace_id.clone())
    else {
        return;
    };
    let ns_secret = fe_database::get_namespace_secret_from_keyring(verse_id)
        .ok()
        .flatten();
    sync.0
        .send(fe_sync::SyncCommand::OpenVerseReplica {
            verse_id: verse_id.to_string(),
            namespace_id: ns_id,
            namespace_secret: ns_secret,
        })
        .ok();
}
