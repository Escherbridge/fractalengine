//! Handlers for tree-structure results: full hierarchy loads, entity
//! create/rename/delete, and DB lifecycle events. See ../AGENTS.md §db-results.

use bevy::prelude::*;
use fe_runtime::app::{DbCommandSender, PendingApiRequests};
use fe_runtime::messages::{DbCommand, EntityType, VerseHierarchyData};

use super::super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};
use crate::navigation_manager::NavigationManager;

/// `Seeded`: kick off the initial hierarchy load.
pub(super) fn handle_seeded(db_sender: &DbCommandSender) {
    if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!("db_sender channel closed after Seeded — DB thread may have crashed");
    }
}

/// `VerseJoined`: reload the hierarchy to pick up the joined verse.
pub(super) fn handle_verse_joined(db_sender: &DbCommandSender) {
    if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!(
            "db_sender channel closed after VerseJoined — DB thread may have crashed"
        );
    }
}

/// `DatabaseReset`: clear the in-memory tree and reload.
pub(super) fn handle_database_reset(verse_mgr: &mut VerseManager, db_sender: &DbCommandSender) {
    bevy::log::info!("Database reset — clearing hierarchy");
    verse_mgr.verses.clear();
    verse_mgr.rebuild_node_index();
    if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!(
            "db_sender channel closed after DatabaseReset — DB thread may have crashed"
        );
    }
}

/// `HierarchyLoaded`: rebuild the whole tree, auto-navigate on first load, and
/// spawn scene entities for the active petal's asset nodes.
pub(super) fn handle_hierarchy_loaded(
    verses: &[VerseHierarchyData],
    verse_mgr: &mut VerseManager,
    nav: &mut NavigationManager,
    commands: &mut Commands,
    asset_server: &AssetServer,
    pending_api: &mut PendingApiRequests,
) {
    let first_load = nav.active_verse_id.is_none();

    // Auto-navigate before rebuilding the tree so spawn uses the
    // correct active_petal_id on this same event (single round-trip).
    if first_load {
        if let Some(v) = verses.first() {
            nav.active_verse_id = Some(v.id.clone());
            nav.active_verse_name = v.name.clone();
        }
        'find_petal: for v in verses.iter() {
            for f in &v.fractals {
                for p in &f.petals {
                    if p.nodes.iter().any(|n| n.asset_path.is_some()) {
                        nav.active_fractal_id = Some(f.id.clone());
                        nav.active_fractal_name = f.name.clone();
                        nav.active_petal_id = Some(p.id.clone());
                        bevy::log::info!(
                            "Auto-navigated to first populated petal: {}/{}",
                            f.name,
                            p.name
                        );
                        break 'find_petal;
                    }
                }
            }
        }
    }

    let active_petal = nav.active_petal_id.clone();

    verse_mgr.verses = verses
        .iter()
        .map(|v| VerseEntry {
            id: v.id.clone(),
            name: v.name.clone(),
            namespace_id: v.namespace_id.clone(),
            expanded: true,
            fractals: v
                .fractals
                .iter()
                .map(|f| FractalEntry {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    expanded: true,
                    petals: f
                        .petals
                        .iter()
                        .map(|p| PetalEntry {
                            id: p.id.clone(),
                            name: p.name.clone(),
                            expanded: true,
                            nodes: p
                                .nodes
                                .iter()
                                .map(|n| {
                                    if active_petal.as_deref() == Some(n.petal_id.as_str()) {
                                        if let Some(ref ap) = n.asset_path {
                                            super::super::spawn::spawn_node_entity(
                                                commands,
                                                asset_server,
                                                &n.id,
                                                &n.petal_id,
                                                &n.name,
                                                n.position,
                                                ap,
                                            );
                                        }
                                    }
                                    NodeEntry {
                                        id: n.id.clone(),
                                        name: n.name.clone(),
                                        has_asset: n.has_asset,
                                        position: n.position,
                                        webpage_url: n.webpage_url.clone(),
                                        asset_path: n.asset_path.clone(),
                                    }
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    verse_mgr.rebuild_node_index();

    // Auto-select verse after non-first-load (e.g. after CreateVerse).
    if nav.active_verse_id.is_none() {
        if let Some(v) = verse_mgr.verses.first() {
            nav.active_verse_id = Some(v.id.clone());
            nav.active_verse_name = v.name.clone();
        }
    }

    // Deliver to pending API requests (GET /api/v1/hierarchy).
    pending_api.deliver_hierarchy(verses.to_vec());
}

/// `VerseCreated`: append an empty verse to the tree.
pub(super) fn handle_verse_created(id: &str, name: &str, verse_mgr: &mut VerseManager) {
    verse_mgr.verses.push(VerseEntry {
        id: id.to_string(),
        name: name.to_string(),
        namespace_id: None,
        expanded: true,
        fractals: Vec::new(),
    });
}

/// `FractalCreated`: append an empty fractal under its verse.
pub(super) fn handle_fractal_created(
    id: &str,
    verse_id: &str,
    name: &str,
    verse_mgr: &mut VerseManager,
) {
    if let Some(verse) = verse_mgr.find_verse_mut(verse_id) {
        verse.fractals.push(FractalEntry {
            id: id.to_string(),
            name: name.to_string(),
            expanded: true,
            petals: Vec::new(),
        });
    }
}

/// `PetalCreated`: append an empty petal under its fractal.
pub(super) fn handle_petal_created(
    id: &str,
    fractal_id: &str,
    name: &str,
    verse_mgr: &mut VerseManager,
) {
    for verse in verse_mgr.verses.iter_mut() {
        if let Some(f) = verse.fractals.iter_mut().find(|f| f.id == fractal_id) {
            f.petals.push(PetalEntry {
                id: id.to_string(),
                name: name.to_string(),
                expanded: true,
                nodes: Vec::new(),
            });
        }
    }
}

/// `EntityRenamed`: rename in the tree and sync the active-navigation labels.
pub(super) fn handle_entity_renamed(
    entity_type: &EntityType,
    entity_id: &str,
    new_name: &str,
    verse_mgr: &mut VerseManager,
    nav: &mut NavigationManager,
) {
    match entity_type {
        EntityType::Verse => {
            if let Some(v) = verse_mgr.verses.iter_mut().find(|v| v.id == entity_id) {
                v.name = new_name.to_string();
            }
            if nav.active_verse_id.as_deref() == Some(entity_id) {
                nav.active_verse_name = new_name.to_string();
            }
        }
        EntityType::Fractal => {
            for v in verse_mgr.verses.iter_mut() {
                if let Some(f) = v.fractals.iter_mut().find(|f| f.id == entity_id) {
                    f.name = new_name.to_string();
                }
            }
            if nav.active_fractal_id.as_deref() == Some(entity_id) {
                nav.active_fractal_name = new_name.to_string();
            }
        }
        EntityType::Petal => {
            for v in verse_mgr.verses.iter_mut() {
                for f in v.fractals.iter_mut() {
                    if let Some(p) = f.petals.iter_mut().find(|p| p.id == entity_id) {
                        p.name = new_name.to_string();
                    }
                }
            }
        }
    }
    bevy::log::info!("Renamed {} {} to '{}'", entity_type, entity_id, new_name);
}

/// `EntityDeleted`: prune the tree, back navigation out of the deleted scope,
/// and close any dialog that referenced it.
pub(super) fn handle_entity_deleted(
    entity_type: &EntityType,
    entity_id: &str,
    verse_mgr: &mut VerseManager,
    nav: &mut NavigationManager,
    ui_mgr: &mut crate::actions::UiManager,
) {
    match entity_type {
        EntityType::Verse => {
            verse_mgr.verses.retain(|v| v.id != entity_id);
            if nav.active_verse_id.as_deref() == Some(entity_id) {
                nav.back_from_verse();
            }
        }
        EntityType::Fractal => {
            for v in verse_mgr.verses.iter_mut() {
                v.fractals.retain(|f| f.id != entity_id);
            }
            if nav.active_fractal_id.as_deref() == Some(entity_id) {
                nav.back_from_fractal();
            }
        }
        EntityType::Petal => {
            for v in verse_mgr.verses.iter_mut() {
                for f in v.fractals.iter_mut() {
                    f.petals.retain(|p| p.id != entity_id);
                }
            }
            if nav.active_petal_id.as_deref() == Some(entity_id) {
                nav.back_from_petal();
            }
        }
    }
    // Structural deletes shift tree indices — rebuild the node lookup index.
    verse_mgr.rebuild_node_index();
    ui_mgr.close_dialog();
    bevy::log::info!("Deleted {} {}", entity_type, entity_id);
}
