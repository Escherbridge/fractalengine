//! VerseManager — owns the in-memory verse/fractal/petal/node hierarchy and
//! drives scene-entity spawn/despawn when the active petal changes.
//!
//! Replaces the old `VerseHierarchy` resource and the `apply_db_results_to_hierarchy`
//! + `despawn_and_spawn_on_petal_change` systems in plugin.rs.
//!
//! Key improvement over the old code: petal navigation no longer round-trips
//! through `DbCommand::LoadHierarchy`.  Because `NodeEntry` now stores
//! `asset_path`, the manager can directly spawn entities from its in-memory
//! tree when `NavigationManager::active_petal_id` changes.

use bevy::prelude::*;
use fe_runtime::messages::{DbCommand, DbResult, EntityType};

use crate::navigation_manager::NavigationManager;
use crate::plugin::{ActiveDialog, SpawnedNodeMarker, UiManager, UiSet};

// ---------------------------------------------------------------------------
// Hierarchy tree types
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct VerseEntry {
    pub id: String,
    pub name: String,
    /// iroh-docs namespace ID used to open the P2P replica.
    pub namespace_id: Option<String>,
    pub expanded: bool,
    pub fractals: Vec<FractalEntry>,
}

#[derive(Clone, Default)]
pub struct FractalEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub petals: Vec<PetalEntry>,
}

#[derive(Clone, Default)]
pub struct PetalEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub nodes: Vec<NodeEntry>,
}

/// In-memory node record.  `asset_path` is retained (previously dropped) so
/// that the manager can respawn entities on petal switch without a DB round-trip.
#[derive(Clone, Default)]
pub struct NodeEntry {
    pub id: String,
    pub name: String,
    pub has_asset: bool,
    pub position: [f32; 3],
    pub webpage_url: Option<String>,
    /// Path to the GLTF/GLB asset, if any.  Drives scene-entity materialisation.
    pub asset_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// Single source of truth for the verse/fractal/petal/node tree.
#[derive(Resource, Default)]
pub struct VerseManager {
    pub verses: Vec<VerseEntry>,
}

impl VerseManager {
    /// Iterate every node in every petal of every fractal in every verse.
    pub fn all_nodes(&self) -> impl Iterator<Item = &NodeEntry> {
        self.verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .flat_map(|p| &p.nodes)
    }

    /// Find a petal by ID (immutable).
    pub fn find_petal<'a>(&'a self, petal_id: &str) -> Option<&'a PetalEntry> {
        self.verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .find(|p| p.id == petal_id)
    }

    pub fn find_verse_mut(&mut self, id: &str) -> Option<&mut VerseEntry> {
        self.verses.iter_mut().find(|v| v.id == id)
    }

    fn find_petal_mut(&mut self, petal_id: &str) -> Option<&mut PetalEntry> {
        self.verses
            .iter_mut()
            .flat_map(|v| &mut v.fractals)
            .flat_map(|f| &mut f.petals)
            .find(|p| p.id == petal_id)
    }

    /// Update position of a node by its ID across all petals.
    pub fn update_node_position(&mut self, node_id: &str, position: [f32; 3]) {
        for verse in &mut self.verses {
            for fractal in &mut verse.fractals {
                for petal in &mut fractal.petals {
                    if let Some(node) = petal.nodes.iter_mut().find(|n| n.id == node_id) {
                        node.position = position;
                        return;
                    }
                }
            }
        }
    }

    /// Update webpage_url of a node by its ID across all petals.
    pub fn update_node_url(&mut self, node_id: &str, url: Option<String>) {
        for verse in &mut self.verses {
            for fractal in &mut verse.fractals {
                for petal in &mut fractal.petals {
                    if let Some(node) = petal.nodes.iter_mut().find(|n| n.id == node_id) {
                        node.webpage_url = url;
                        return;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct VerseManagerPlugin;

impl Plugin for VerseManagerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VerseManager>();
        app.add_systems(
            Update,
            (
                apply_db_results,
                respawn_on_petal_change,
            )
                .before(UiSet::ProcessActions),
        );
    }
}

// ---------------------------------------------------------------------------
// System: process DbResult messages → update hierarchy + spawn entities
// ---------------------------------------------------------------------------

fn apply_db_results(
    mut reader: MessageReader<DbResult>,
    mut verse_mgr: ResMut<VerseManager>,
    mut nav: ResMut<NavigationManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut ui_mgr: ResMut<UiManager>,
    mut local_role: ResMut<crate::plugin::LocalUserRole>,
    revocation_tx: Option<Res<fe_runtime::app::RevocationBroadcastSender>>,
    mut inspector: ResMut<crate::plugin::InspectorFormState>,
    mut pending_api: ResMut<fe_runtime::app::PendingApiRequests>,
) {
    for result in reader.read() {
        match result {
            DbResult::Seeded { .. } => {
                if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
                    bevy::log::error!("db_sender channel closed after Seeded — DB thread may have crashed");
                }
            }

            DbResult::HierarchyLoaded { verses } => {
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
                                        f.name, p.name
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
                        fractals: v.fractals.iter().map(|f| FractalEntry {
                            id: f.id.clone(),
                            name: f.name.clone(),
                            expanded: true,
                            petals: f.petals.iter().map(|p| PetalEntry {
                                id: p.id.clone(),
                                name: p.name.clone(),
                                expanded: true,
                                nodes: p.nodes.iter().map(|n| {
                                    if active_petal.as_deref() == Some(n.petal_id.as_str()) {
                                        if let Some(ref ap) = n.asset_path {
                                            spawn_node_entity(
                                                &mut commands,
                                                &asset_server,
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
                                }).collect(),
                            }).collect(),
                        }).collect(),
                    })
                    .collect();

                // Auto-select verse after non-first-load (e.g. after CreateVerse).
                if nav.active_verse_id.is_none() {
                    if let Some(v) = verse_mgr.verses.first() {
                        nav.active_verse_id = Some(v.id.clone());
                        nav.active_verse_name = v.name.clone();
                    }
                }

                // Deliver to pending API requests (GET /api/v1/hierarchy).
                pending_api.deliver_hierarchy(verses.clone());
            }

            DbResult::GltfImported { node_id, name, petal_id, asset_path, position, .. } => {
                if let Some(petal) = verse_mgr.find_petal_mut(petal_id) {
                    petal.nodes.push(NodeEntry {
                        id: node_id.clone(),
                        name: name.clone(),
                        has_asset: true,
                        position: *position,
                        webpage_url: None,
                        asset_path: Some(asset_path.clone()),
                    });
                }
                if nav.active_petal_id.as_deref() == Some(petal_id.as_str()) {
                    spawn_node_entity(
                        &mut commands, &asset_server,
                        node_id, petal_id, name, *position, asset_path,
                    );
                }
            }

            DbResult::NodeCreated { id, petal_id, name, has_asset } => {
                if let Some(petal) = verse_mgr.find_petal_mut(petal_id) {
                    petal.nodes.push(NodeEntry {
                        id: id.clone(),
                        name: name.clone(),
                        has_asset: *has_asset,
                        position: [0.0; 3],
                        webpage_url: None,
                        asset_path: None,
                    });
                }
            }

            DbResult::VerseCreated { id, name } => {
                verse_mgr.verses.push(VerseEntry {
                    id: id.clone(),
                    name: name.clone(),
                    namespace_id: None,
                    expanded: true,
                    fractals: Vec::new(),
                });
            }

            DbResult::FractalCreated { id, verse_id, name } => {
                if let Some(verse) = verse_mgr.find_verse_mut(verse_id) {
                    verse.fractals.push(FractalEntry {
                        id: id.clone(),
                        name: name.clone(),
                        expanded: true,
                        petals: Vec::new(),
                    });
                }
            }

            DbResult::PetalCreated { id, fractal_id, name } => {
                for verse in verse_mgr.verses.iter_mut() {
                    if let Some(f) = verse.fractals.iter_mut().find(|f| f.id == *fractal_id) {
                        f.petals.push(PetalEntry {
                            id: id.clone(),
                            name: name.clone(),
                            expanded: true,
                            nodes: Vec::new(),
                        });
                    }
                }
            }

            DbResult::VerseInviteGenerated { invite_string, .. } => {
                ui_mgr.open_dialog(ActiveDialog::InviteDialog {
                    invite_string: invite_string.clone(),
                    include_write_cap: false,
                    expiry_hours: 24,
                });
            }

            DbResult::VerseJoined { .. } => {
                if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
                    bevy::log::error!("db_sender channel closed after VerseJoined — DB thread may have crashed");
                }
            }

            DbResult::DatabaseReset { .. } => {
                bevy::log::info!("Database reset — clearing hierarchy");
                verse_mgr.verses.clear();
                if db_sender.0.send(DbCommand::LoadHierarchy).is_err() {
                    bevy::log::error!("db_sender channel closed after DatabaseReset — DB thread may have crashed");
                }
            }

            DbResult::Error(msg) => {
                bevy::log::error!("DB error: {msg}");
            }

            DbResult::EntityRenamed { entity_type, entity_id, new_name } => {
                match entity_type {
                    EntityType::Verse => {
                        if let Some(v) = verse_mgr.verses.iter_mut().find(|v| v.id == *entity_id) {
                            v.name = new_name.clone();
                        }
                        if nav.active_verse_id.as_deref() == Some(entity_id.as_str()) {
                            nav.active_verse_name = new_name.clone();
                        }
                    }
                    EntityType::Fractal => {
                        for v in verse_mgr.verses.iter_mut() {
                            if let Some(f) = v.fractals.iter_mut().find(|f| f.id == *entity_id) {
                                f.name = new_name.clone();
                            }
                        }
                        if nav.active_fractal_id.as_deref() == Some(entity_id.as_str()) {
                            nav.active_fractal_name = new_name.clone();
                        }
                    }
                    EntityType::Petal => {
                        for v in verse_mgr.verses.iter_mut() {
                            for f in v.fractals.iter_mut() {
                                if let Some(p) = f.petals.iter_mut().find(|p| p.id == *entity_id) {
                                    p.name = new_name.clone();
                                }
                            }
                        }
                    }
                }
                bevy::log::info!("Renamed {} {} to '{}'", entity_type, entity_id, new_name);
            }

            DbResult::VerseDefaultAccessSet { verse_id, default_access } => {
                bevy::log::info!("Set default access for verse {} to '{}'", verse_id, default_access);
            }

            DbResult::FractalDescriptionUpdated { fractal_id, description } => {
                bevy::log::info!("Updated description for fractal {}: '{}'", fractal_id, description);
            }

            DbResult::EntityDeleted { entity_type, entity_id } => {
                match entity_type {
                    EntityType::Verse => {
                        verse_mgr.verses.retain(|v| v.id != *entity_id);
                        if nav.active_verse_id.as_deref() == Some(entity_id.as_str()) {
                            nav.back_from_verse();
                        }
                    }
                    EntityType::Fractal => {
                        for v in verse_mgr.verses.iter_mut() {
                            v.fractals.retain(|f| f.id != *entity_id);
                        }
                        if nav.active_fractal_id.as_deref() == Some(entity_id.as_str()) {
                            nav.back_from_fractal();
                        }
                    }
                    EntityType::Petal => {
                        for v in verse_mgr.verses.iter_mut() {
                            for f in v.fractals.iter_mut() {
                                f.petals.retain(|p| p.id != *entity_id);
                            }
                        }
                        if nav.active_petal_id.as_deref() == Some(entity_id.as_str()) {
                            nav.back_from_petal();
                        }
                    }
                }
                ui_mgr.close_dialog();
                bevy::log::info!("Deleted {} {}", entity_type, entity_id);
            }

            DbResult::PeerRolesResolved { scope, roles } => {
                if let ActiveDialog::EntitySettings { ref mut peer_roles, ref mut roles_loading, .. } = ui_mgr.active_dialog {
                    *peer_roles = roles.iter().map(|(did, role)| {
                        crate::plugin::PeerRoleEntry {
                            peer_did: did.clone(),
                            display_name: String::new(),
                            role: role.clone(),
                            is_online: false,
                        }
                    }).collect();
                    *roles_loading = false;
                }
                bevy::log::debug!("Resolved {} peer roles at scope {}", roles.len(), scope);
            }

            DbResult::RoleAssigned { peer_did, scope, role } => {
                if let ActiveDialog::EntitySettings { ref mut peer_roles, .. } = ui_mgr.active_dialog {
                    if let Some(entry) = peer_roles.iter_mut().find(|p| p.peer_did == *peer_did) {
                        entry.role = role.clone();
                    }
                }
                bevy::log::info!("Assigned role '{}' to {} at scope {}", role, peer_did, scope);
            }

            DbResult::RoleRevoked { peer_did, scope } => {
                if let ActiveDialog::EntitySettings { ref mut peer_roles, .. } = ui_mgr.active_dialog {
                    if let Some(entry) = peer_roles.iter_mut().find(|p| p.peer_did == *peer_did) {
                        entry.role = "none".to_string();
                    }
                }
                bevy::log::info!("Revoked role for {} at scope {}", peer_did, scope);
            }

            DbResult::ScopedInviteGenerated { invite_link } => {
                if let ActiveDialog::EntitySettings { ref mut generated_invite_link, .. } = ui_mgr.active_dialog {
                    *generated_invite_link = Some(invite_link.clone());
                }
                bevy::log::info!("Generated scoped invite link");
            }

            DbResult::LocalRoleResolved { scope, role } => {
                let level = fe_database::RoleLevel::from(role.as_str());
                bevy::log::info!("Local role resolved at {}: {} ({:?})", scope, role, level);
                local_role.role = Some(level);
            }

            DbResult::ApiTokenMinted { token, jti, scope, max_role, expires_at: _, label: _ } => {
                if let ActiveDialog::EntitySettings { ref mut generated_api_token, .. } = ui_mgr.active_dialog {
                    *generated_api_token = Some(token.clone());
                }
                inspector.generated_api_token = Some(token.clone());
                bevy::log::info!("API token minted: jti={} scope={} role={}", jti, scope, max_role);
                // Refresh the scoped token list at current page
                refresh_inspector_tokens(&db_sender, &inspector);
            }

            DbResult::ApiTokenRevoked { jti } => {
                bevy::log::info!("API token revoked: jti={}", jti);
                if let Some(ref tx) = revocation_tx {
                    if tx.0.send(jti.clone()).is_err() {
                        bevy::log::error!("revocation_tx channel closed — API thread may have crashed");
                    }
                }
                // Refresh the scoped token list at current page
                refresh_inspector_tokens(&db_sender, &inspector);
            }

            DbResult::ApiTokensListed { tokens, total } => {
                let entries = tokens_to_entries(tokens);
                if let ActiveDialog::EntitySettings { ref mut api_tokens, ref mut api_tokens_loading, .. } = ui_mgr.active_dialog {
                    *api_tokens = entries.clone();
                    *api_tokens_loading = false;
                }
                inspector.api_tokens = entries;
                inspector.api_tokens_total = *total;
                inspector.api_tokens_loading = false;
            }

            DbResult::ScopedApiTokensListed { tokens, total } => {
                let entries = tokens_to_entries(tokens);
                if let ActiveDialog::EntitySettings { ref mut scoped_api_tokens, ref mut scoped_tokens_loading, .. } = ui_mgr.active_dialog {
                    *scoped_api_tokens = entries.clone();
                    *scoped_tokens_loading = false;
                }
                inspector.api_tokens = entries;
                inspector.api_tokens_total = *total;
                inspector.api_tokens_loading = false;
            }

            _ => {}
        }

        // Also try delivering every result to pending API requests.
        // This covers cases like ScopeResolved, NodeCreated, etc. that
        // the API thread may be waiting on.
        pending_api.try_deliver(result.clone());
    }
}

/// Convert ApiTokenInfo list to UI-displayable ApiTokenEntry list.
fn tokens_to_entries(tokens: &[fe_runtime::messages::ApiTokenInfo]) -> Vec<crate::plugin::ApiTokenEntry> {
    tokens.iter().map(|t| crate::plugin::ApiTokenEntry {
        jti: t.jti.clone(),
        scope: t.scope.clone(),
        max_role: t.max_role.clone(),
        label: t.label.clone(),
        created_at: t.created_at.clone(),
        expires_at: t.expires_at.clone(),
        revoked: t.revoked,
    }).collect()
}

/// Send a scoped, paginated token list refresh using the inspector's current scope and page.
fn refresh_inspector_tokens(
    db_sender: &fe_runtime::app::DbCommandSender,
    inspector: &crate::plugin::InspectorFormState,
) {
    let offset = inspector.api_tokens_page * crate::plugin::API_TOKEN_PAGE_SIZE;
    let limit = crate::plugin::API_TOKEN_PAGE_SIZE;
    let scope = &inspector.api_token_scope_buf;
    if scope.is_empty() {
        if db_sender.0.send(DbCommand::ListApiTokens { offset, limit }).is_err() {
            bevy::log::error!("db_sender channel closed during token list refresh — DB thread may have crashed");
        }
    } else {
        if db_sender.0.send(DbCommand::ListApiTokensByScope {
            scope_prefix: scope.clone(),
            offset,
            limit,
        }).is_err() {
            bevy::log::error!("db_sender channel closed during scoped token list refresh — DB thread may have crashed");
        }
    }
}

// ---------------------------------------------------------------------------
// System: respawn entities when active petal changes (no DB round-trip)
// ---------------------------------------------------------------------------

fn respawn_on_petal_change(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    mut last: Local<Option<String>>,
    mut initialized: Local<bool>,
    spawned: Query<(Entity, &SpawnedNodeMarker)>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    if !*initialized {
        *last = nav.active_petal_id.clone();
        *initialized = true;
        return;
    }
    if *last == nav.active_petal_id {
        return;
    }

    let new_petal = nav.active_petal_id.clone();
    bevy::log::info!(
        "Petal changed {:?} → {:?} — respawning entities",
        *last, new_petal
    );

    // Collect which node_ids are staying alive (kept entities) before issuing any despawns.
    // commands.entity(e).despawn() is deferred — entities just despawned are still visible
    // in the `spawned` Query during the same frame, so we must not rely on the query after
    // despawn commands have been issued.
    let kept_node_ids: std::collections::HashSet<&str> = spawned
        .iter()
        .filter(|(_, m)| new_petal.as_deref().map(|pid| pid == m.petal_id.as_str()).unwrap_or(false))
        .map(|(_, m)| m.node_id.as_str())
        .collect();

    // Despawn entities that don't belong to the new petal.
    for (entity, marker) in spawned.iter() {
        let keep = new_petal
            .as_deref()
            .map(|pid| pid == marker.petal_id.as_str())
            .unwrap_or(false);
        if !keep {
            commands.entity(entity).despawn();
        }
    }

    // Spawn entities for the new petal directly from in-memory data — no DB round-trip.
    if let Some(ref pid) = new_petal {
        for verse in &verse_mgr.verses {
            for fractal in &verse.fractals {
                for petal in &fractal.petals {
                    if petal.id != *pid {
                        continue;
                    }
                    for node in &petal.nodes {
                        let Some(ref ap) = node.asset_path else { continue };
                        // Skip if already spawned and being kept (petal didn't fully change).
                        if kept_node_ids.contains(node.id.as_str()) {
                            continue;
                        }
                        spawn_node_entity(
                            &mut commands,
                            &asset_server,
                            &node.id,
                            pid,
                            &node.name,
                            node.position,
                            ap,
                        );
                    }
                }
            }
        }
    }

    *last = new_petal;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn spawn_node_entity(
    commands: &mut Commands,
    asset_server: &AssetServer,
    node_id: &str,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
    asset_path: &str,
) {
    let handle: Handle<Scene> = asset_server.load(format!("{}#Scene0", asset_path));
    let entity = commands
        .spawn((
            SceneRoot(handle),
            Transform::from_xyz(position[0], position[1], position[2]),
            Name::new(name.to_string()),
            SpawnedNodeMarker {
                node_id: node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
        ))
        .id();
    bevy::log::debug!(
        "Spawned '{}' entity={:?} (petal={})", name, entity, petal_id
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree() -> VerseManager {
        let node = NodeEntry {
            id: "node-1".to_string(),
            name: "Node 1".to_string(),
            has_asset: true,
            position: [1.0, 2.0, 3.0],
            webpage_url: None,
            asset_path: Some("models/cube.glb".to_string()),
        };
        let petal = PetalEntry {
            id: "petal-1".to_string(),
            name: "Petal 1".to_string(),
            expanded: true,
            nodes: vec![node],
        };
        let fractal = FractalEntry {
            id: "fractal-1".to_string(),
            name: "Fractal 1".to_string(),
            expanded: true,
            petals: vec![petal],
        };
        let verse = VerseEntry {
            id: "verse-1".to_string(),
            name: "Verse 1".to_string(),
            namespace_id: None,
            expanded: true,
            fractals: vec![fractal],
        };
        VerseManager { verses: vec![verse] }
    }

    #[test]
    fn update_node_position_finds_correct_node() {
        let mut mgr = make_tree();
        mgr.update_node_position("node-1", [9.0, 8.0, 7.0]);
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert_eq!(node.position, [9.0, 8.0, 7.0]);
    }

    #[test]
    fn update_node_position_noop_on_missing_id() {
        let mut mgr = make_tree();
        mgr.update_node_position("does-not-exist", [0.0, 0.0, 0.0]);
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert_eq!(node.position, [1.0, 2.0, 3.0], "existing node must be unaffected");
    }

    #[test]
    fn update_node_url_sets_and_clears() {
        let mut mgr = make_tree();
        mgr.update_node_url("node-1", Some("https://example.com".to_string()));
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert_eq!(node.webpage_url, Some("https://example.com".to_string()));
        mgr.update_node_url("node-1", None);
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert!(node.webpage_url.is_none());
    }

    #[test]
    fn find_petal_returns_correct_petal() {
        let mgr = make_tree();
        let petal = mgr.find_petal("petal-1");
        assert!(petal.is_some());
        assert_eq!(petal.unwrap().id, "petal-1");
        assert_eq!(petal.unwrap().nodes.len(), 1);
    }

    #[test]
    fn find_petal_returns_none_for_missing_id() {
        let mgr = make_tree();
        assert!(mgr.find_petal("does-not-exist").is_none());
    }

    #[test]
    fn add_verse_fractal_petal_node_chain() {
        let mut mgr = VerseManager::default();
        assert!(mgr.verses.is_empty());

        mgr.verses.push(VerseEntry {
            id: "v1".to_string(),
            name: "V1".to_string(),
            namespace_id: None,
            expanded: true,
            fractals: vec![],
        });
        assert_eq!(mgr.verses.len(), 1);

        mgr.find_verse_mut("v1").unwrap().fractals.push(FractalEntry {
            id: "f1".to_string(),
            name: "F1".to_string(),
            expanded: true,
            petals: vec![],
        });
        assert_eq!(mgr.verses[0].fractals.len(), 1);

        mgr.verses[0].fractals[0].petals.push(PetalEntry {
            id: "p1".to_string(),
            name: "P1".to_string(),
            expanded: true,
            nodes: vec![],
        });

        mgr.verses[0].fractals[0].petals[0].nodes.push(NodeEntry {
            id: "n1".to_string(),
            name: "N1".to_string(),
            has_asset: false,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: None,
        });

        assert!(mgr.find_petal("p1").is_some());
        assert_eq!(mgr.all_nodes().count(), 1);
    }
}
