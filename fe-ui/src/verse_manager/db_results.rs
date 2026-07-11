//! Drains `DbResult` messages into the hierarchy, dialogs, and inspector
//! state, and spawns scene entities for freshly-loaded nodes in the active
//! petal.

use bevy::prelude::*;
use fe_runtime::messages::{DbCommand, DbResult, EntityType};

use super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};
use crate::dialogs::{ActiveDialog, ApiTokenEntry};
use crate::navigation_manager::NavigationManager;

pub(super) fn apply_db_results(
    mut reader: MessageReader<DbResult>,
    mut verse_mgr: ResMut<VerseManager>,
    mut nav: ResMut<NavigationManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut local_role: ResMut<crate::plugin::LocalUserRole>,
    revocation_tx: Option<Res<fe_runtime::app::RevocationBroadcastSender>>,
    mut inspector: ResMut<crate::plugin::InspectorFormState>,
    mut pending_api: ResMut<fe_runtime::app::PendingApiRequests>,
    mut petal_map: ResMut<crate::terrain_map::PetalMapState>,
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
                                            super::spawn::spawn_node_entity(
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
                    super::spawn::spawn_node_entity(
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
                // If query tab is waiting, deliver the error there
                if inspector.query_loading {
                    inspector.query_result = Some(format!("Error: {msg}"));
                    inspector.query_loading = false;
                }
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
                        crate::dialogs::PeerRoleEntry {
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

            DbResult::QueryResult { data } => {
                let formatted = serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("Format error: {e}"));
                inspector.query_result = Some(formatted);
                inspector.query_loading = false;
            }

            // --- Property value results ---
            DbResult::NodePropertiesLoaded { node_id: _, ref properties } => {
                inspector.node_properties = properties.clone();
                inspector.node_properties_loading = false;
            }
            DbResult::NodePropertySet { ref node_id, key: _ } => {
                // Re-fetch properties for the node to refresh UI
                inspector.node_properties_loading = true;
                let _ = db_sender.0.send(DbCommand::GetNodeProperties {
                    node_id: node_id.clone(),
                });
            }
            DbResult::NodePropertyDeleted { node_id: _, ref key } => {
                // Remove the key locally for immediate UI feedback
                if let Some(obj) = inspector.node_properties.as_object_mut() {
                    obj.remove(key.as_str());
                }
            }

            // --- Field definition results ---
            DbResult::FieldDefsListed { scope: _, ref field_defs } => {
                inspector.field_defs = field_defs.iter().map(|f| crate::plugin::FieldDefEntry {
                    field_def_id: f.field_def_id.clone(),
                    key: f.key.clone(),
                    value_type: f.value_type.clone(),
                    description: String::new(),
                    required: false,
                    default_val: f.default_val.clone(),
                }).collect();
                inspector.field_defs_loading = false;
            }
            DbResult::FieldDefCreated { .. } | DbResult::FieldDefUpdated { .. } | DbResult::FieldDefDeleted { .. } => {
                // Trigger a refresh of field defs — re-list from current scope
                // (the UI will need to re-send ListFieldDefs; handled by the panel code)
            }

            DbResult::PetalTerrainLoaded { ref petal_id, ref terrain } => {
                // Only the active petal's terrain drives the map picker state.
                if nav.active_petal_id.as_deref() == Some(petal_id.as_str()) {
                    petal_map.petal_id = Some(petal_id.clone());
                    petal_map.tileset_ids = terrain
                        .as_ref()
                        .and_then(|t| t.get("tileset_hexon_uris"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    petal_map.loaded = true;
                }
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
fn tokens_to_entries(tokens: &[fe_runtime::messages::ApiTokenInfo]) -> Vec<ApiTokenEntry> {
    tokens.iter().map(|t| ApiTokenEntry {
        jti: t.jti.clone(),
        scope: t.scope.clone(),
        max_role: t.max_role.clone(),
        label: t.label.clone(),
        created_at: t.created_at.clone(),
        expires_at: t.expires_at.clone(),
        revoked: t.revoked,
        sub: t.sub.clone(),
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
