//! Handlers for tree-structure results: full hierarchy loads, entity
//! create/rename/delete, and DB lifecycle events. See ../AGENTS.md §db-results.

use bevy::prelude::*;
use fe_runtime::app::{DbCommandSender, PendingApiRequests};
use fe_runtime::messages::{DbCommand, EntityType, VerseHierarchyData};

use super::super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};
use crate::dialogs::ActiveDialog;
use crate::navigation_manager::NavigationManager;
use crate::notice::{humanize_db_error, DenialContext, UNRESOLVED_ROLE_LABEL};

/// `Seeded`: kick off the initial hierarchy load.
pub(super) fn handle_seeded(db_sender: &DbCommandSender) {
    if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!("db_sender channel closed after Seeded — DB thread may have crashed");
    }
}

/// `VerseJoined`: if a Join dialog is awaiting this result (OD-2/M-03), close
/// it, navigate into the joined verse and toast success; always reload the
/// hierarchy so the joined verse's full tree materializes regardless of who
/// triggered the join (GUI dialog, or an external path with no pending flag).
pub(super) fn handle_verse_joined(
    verse_id: &str,
    verse_name: &str,
    db_sender: &DbCommandSender,
    nav: &mut NavigationManager,
    ui_mgr: &mut crate::actions::UiManager,
    now: f64,
) {
    if ui_mgr.join_flow.active() {return;}
    if ui_mgr.join_request.finish() {
        if matches!(ui_mgr.active_dialog, ActiveDialog::JoinDialog { .. }) {
            ui_mgr.close_dialog();
        }
        nav.navigate_to_verse(verse_id, verse_name);
        ui_mgr.show_toast(format!("Joined {verse_name}"), now);
    }
    if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!(
            "db_sender channel closed after VerseJoined — DB thread may have crashed"
        );
    }
}

/// `JoinVerseFailed` (OD-2/M-03): a failed Join dialog reports inline while
/// it is still open and pending; a stale/foreign failure (the dialog was
/// already closed, or Join was triggered some other way) surfaces as a
/// general error notice instead of being silently dropped.
pub(super) fn handle_join_verse_failed(
    error: &str,
    ui_mgr: &mut crate::actions::UiManager,
    now: f64,
) {
    // No role exists yet in a verse being joined, so it is unresolved.
    let ctx = DenialContext {
        role_label: UNRESOLVED_ROLE_LABEL,
        verse_name: None,
    };
    if ui_mgr.join_flow.active() {return;}
    if ui_mgr.join_request.finish() {
        ui_mgr.join_request.error = Some(humanize_db_error(error, &ctx));
    } else {
        ui_mgr.show_db_error(error, &ctx, now);
    }
}

/// `ElevatedQueryResult`: an API elevated write changed rows the tree mirrors
/// (it has no typed result of its own) — reload so the desktop reflects it.
pub(super) fn handle_elevated_query_result(changed_rows: usize, db_sender: &DbCommandSender) {
    if changed_rows > 0 && db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!(
            "db_sender channel closed after an elevated write — DB thread may have crashed"
        );
    }
}

/// `DatabaseReset`: clear the in-memory tree and reload.
pub(super) fn handle_database_reset(verse_mgr: &mut VerseManager, db_sender: &DbCommandSender) {
    bevy::log::info!("Database reset — clearing hierarchy");
    verse_mgr.verses.clear();
    verse_mgr.committed_versions.clear();
    verse_mgr.rebuild_node_index();
    if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
        bevy::log::error!(
            "db_sender channel closed after DatabaseReset — DB thread may have crashed"
        );
    }
}

/// `HierarchyLoaded`: rebuild the whole tree, auto-navigate on first load, and
/// spawn scene entities for the active petal's ledger-admitted asset nodes
/// (others are `apply_node_residency`'s — see fe-ui `AGENTS.md`
/// §residency-ledger). `already_spawned` holds node_ids with a live scene
/// entity — repeat loads (API polls, P2P joins) skip them instead of
/// double-materializing; see AGENTS.md §db-results.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_hierarchy_loaded(
    verses: &[VerseHierarchyData],
    verse_mgr: &mut VerseManager,
    nav: &mut NavigationManager,
    commands: &mut Commands,
    asset_server: &AssetServer,
    pending_api: &mut PendingApiRequests,
    already_spawned: &mut std::collections::HashSet<String>,
    budget_exceeded: bool,
    residency: &super::super::ResidencyLedger,
    ts_settings: &mut crate::timeseries_settings::TimeseriesSettingsState,
    allow_initial_navigation: bool,
) {
    let first_load = allow_initial_navigation && nav.initial_auto_navigation_allowed();

    // M2/F6: mirror each verse's persisted timeseries settings into the
    // settings surface state (the verse row's ts_* columns are the truth).
    for v in verses.iter() {
        ts_settings.set_from_hierarchy(&v.id, v.timeseries);
    }

    // Auto-navigate before rebuilding the tree so spawn uses the
    // correct active_petal_id on this same event (single round-trip).
    select_initial_navigation(verses, nav, first_load);

    let active_petal = nav.active_petal_id.clone();

    // Spawn budget: skip nodes already materialized (repeat HierarchyLoaded
    // must be idempotent) and saturate at MAX_PETAL_NODES / the app-wide mesh
    // budget instead of spawning unbounded.
    let mut remaining = if budget_exceeded {
        0
    } else {
        super::super::spawn::spawn_allowance(
            usize::MAX,
            already_spawned.len(),
            super::super::spawn::MAX_PETAL_NODES,
        )
    };
    let mut skipped: usize = 0;

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
                            render_distance: p.render_distance,
                            nodes: p
                                .nodes
                                .iter()
                                .map(|n| {
                                    if active_petal.as_deref() == Some(n.petal_id.as_str()) {
                                        if let Some(ref ap) = n.asset_path {
                                            // Skip already-live nodes (repeat load) —
                                            // the pre-fix double-materialization bug.
                                            if already_spawned.contains(&n.id)
                                                || !residency.admits_node(&n.id)
                                            {
                                                // already live, or not resident —
                                                // `apply_node_residency` owns it
                                            } else if remaining == 0 {
                                                skipped += 1;
                                            } else {
                                                let entity = super::super::spawn::spawn_node_entity(
                                                    commands,
                                                    asset_server,
                                                    &n.id,
                                                    &n.petal_id,
                                                    &n.name,
                                                    n.position,
                                                    ap,
                                                );
                                                let rotation = Quat::from_array(n.rotation);
                                                let (rx, ry, rz) = rotation.to_euler(EulerRot::XYZ);
                                                commands.entity(entity).insert((
                                                    Transform {
                                                        translation: Vec3::from_array(n.position),
                                                        rotation,
                                                        scale: Vec3::from_array(n.scale),
                                                    },
                                                    fe_runtime::messages::DbConfirmedTransform {
                                                        position: n.position,
                                                        rotation: [rx, ry, rz],
                                                        scale: n.scale,
                                                    },
                                                ));
                                                already_spawned.insert(n.id.clone());
                                                remaining -= 1;
                                            }
                                        }
                                    }
                                    NodeEntry {
                                        transform: Transform {
                                            rotation: Quat::from_array(n.rotation),
                                            scale: Vec3::from_array(n.scale),
                                            ..default()
                                        },
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

    if skipped > 0 {
        bevy::log::warn!(
            "hierarchy spawn saturated: {} node(s) not materialized (cap {} / budget gate)",
            skipped,
            super::super::spawn::MAX_PETAL_NODES
        );
    }

    // A later seed may supply the first startup verse, before any cursor intent.
    if nav.initial_auto_navigation_allowed() {
        if let Some(v) = verse_mgr.verses.first() {
            nav.navigate_to_verse(&v.id, &v.name);
        }
    }

    // Deliver to pending API requests (GET /api/v1/hierarchy).
    pending_api.deliver_hierarchy(verses.to_vec());
}

/// Choose one populated petal and its ancestors, or the first empty verse.
pub(super) fn select_initial_navigation(
    verses: &[VerseHierarchyData],
    nav: &mut NavigationManager,
    allowed: bool,
) {
    if !allowed {return;}
    for verse in verses {
        for fractal in &verse.fractals {
            if let Some(petal) = fractal
                .petals
                .iter()
                .find(|petal| petal.nodes.iter().any(|node| node.asset_path.is_some()))
            {
                nav.navigate_to_petal_in_fractal(
                    &verse.id,
                    &verse.name,
                    &fractal.id,
                    &fractal.name,
                    &petal.id,
                );
                bevy::log::info!(
                    "Auto-navigated to first populated petal: {}/{}",
                    fractal.name,
                    petal.name
                );
                return;
            }
        }
    }
    if let Some(verse) = verses.first() {
        nav.navigate_to_verse(&verse.id, &verse.name);
    }
}

/// `VerseCreated`: append the new verse to the tree, carrying its namespace
/// id (F22) so navigation can open the replica immediately — CreateVerse does
/// NOT trigger a hierarchy reload, so this arm is the only place the fresh
/// verse's `namespace_id` lands in the tree. A later `HierarchyLoaded` (an API
/// poll or a P2P join) rebuilds the tree from the DB anyway.
pub(super) fn handle_verse_created(
    id: &str,
    name: &str,
    namespace_id: Option<String>,
    verse_mgr: &mut VerseManager,
) {
    // A hierarchy reload may already have materialized this verse (e.g. an
    // external API poll racing the create). Update it in place rather than
    // appending a duplicate, and only overwrite the id when the result
    // actually carries one.
    if let Some(existing) = verse_mgr.verses.iter_mut().find(|v| v.id == id) {
        if namespace_id.is_some() {
            existing.namespace_id = namespace_id;
        }
        return;
    }
    verse_mgr.verses.push(VerseEntry {
        id: id.to_string(),
        name: name.to_string(),
        namespace_id,
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
                render_distance: None,
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
